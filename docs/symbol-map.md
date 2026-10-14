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
| Driver | `compiler::Compiler::{compile_path, compile_source, execute, with_std_package}`, `compiler::CompiledProgram::{mir, elaborated_mir, drop_elaborated_mir, concrete_mir, emit_mir, imported_package}` | The only whole-program stage ordering. Holds the MIR phases apart, each computed once: parametric MIR (`mir`), the program's own drop-elaborated MIR (`elaborated_mir`, the elaborator's input), the whole drop-elaborated program with every imported package body joined (`drop_elaborated_mir`, what `emit_mir` serializes), and concrete MIR (`concrete_mir`, the elaborator's cached output every backend consumes, refused when the program's own bodies carry verifier findings). |
| Bundled library package | `package::{bundled_std_package, precompile, installed_package, ImportedPackage, BUILD_ID, NO_LIBRARY_SOURCE}`, `module::{installed_prefix, ModuleError::InstalledPackage}`, `checked_package::LibraryRoot`, root `build.rs` (`MOJITO_BUILD_ID`, over the compiler's and the bundled library's sources) | The bundled library compiled once per compiler build into a `std` package, and read once per process: an installed compiler (`module::installed_prefix`) maps `<prefix>/lib/mojito/std.mojoc`, which `mojito precompile` writes (`package::precompile`), and fails with `ModuleError::InstalledPackage` without it; a development build caches its own in `target/mojito-cache`. The package names every library file relative to the library's root (`LibraryRoot`: `CheckedUnit::{encode, decode}`'s file table, `DeclarationRecords::encode`/`PackageDeclarations::read`'s module table), and the build refuses one naming its root (`package::refuse_install_paths`). `Compiler::{compile_path, compile_source, compile_unlinked}` link once, with its module records (`Compiler::compile_linking`), and import it, a link error being the compile's error; `MOJITO_STD_PACKAGE=0` compiles the library from source in a development build and is refused by an installed one (`package::NO_LIBRARY_SOURCE`), which ships no library source; `Compiler::compile_linked`, given a source link, compiles the library from source. The check looks a package name up in the declaration directory, read in place (`ImportedPackage::checked`, `checker::DeclarationInterfaces::new` over `ImportedPackage::declarations`); each compilation marks the modules the link read from the package, by their positions in its module table (`module::LinkedProgram::packaged`) and decodes no record (`checker::InstalledInterfaces::reaching`), and prepares, elaborates, and checks the program's own statements alone, with the records as context, each decoded on first lookup (`Compiler::compile_importing`, `comptime::ImportedScope`); no module has a checker record. A package build exports its check's rows (`Compiler::compile_for_package`), and makes the declaration records of them once the library is lowered (`checker::InterfaceExport::records`); it slices the compiled program's checked tables into one checked record per lowering unit (`package::packaged_units`, `CheckedProgram::units`, `CheckedTables::extract`), each placing the functions the whole library's lowering attributes to it first (`mir::lower_checked_program_units`), and refuses the package unless every unit, read back and lowered alone, gives each function it places as the library's lowering did (`package::refuse_unequal_units`, `mir::text::package::same_body`). |
| Package format | `mir::text::package::{write_package, read_package, same_body, Package, PackageHeader, PackageBodies, BodyFinish}`; `checked_unit::{LoweringUnit, UnitKind, CheckedUnit, UnitProgram}` (a unit names nodes, never syntax: every unit names its statement node, `root`: a `def`'s or constant's own, a method's or a wrapper's struct's, a trait member's own `def`'s; a wrapper also names its `index` among the default wrappers) and `CheckedProgram::{lowering_units, installed}`, `checked::InstalledSites`, `CheckedProgram::units`, `checked::encoding` (the `masked_node!` encodings of `CheckedExpr`, `CheckedStmt`, and `CheckedDeclaration`: an encoding mask, the node's parts, then only the attributes the mask names) (crate `mojito-checked`); `token::{encoding_occurrences, decoding_occurrences, node_syntax}` (crate `mojito-common`); `checked::package::{PackageBytes, Payload, PackageDeclarations, DeclarationDirectory, ModuleEntry, PackagedModule, DeclarationEntry, PackagedBody, LoweredDeclaration, DeclarationRecords, DeclarationRecord, DeclaringModules, InterfaceTable, PackageScope, ProgramOverloads}` (crate `mojito-checked`, re-exported as `mojito::checked_package`); `checked::declaration::{StructDeclaration, FunctionDeclaration, TraitDeclaration, TraitRequirement, ExplicitDestroyObligation}` (a `TraitRequirement` carries its requirement's signature and dispatch symbol; MIR's `MirStructDeclaration`, `MirFunctionDeclaration`); `origin::encoding_packaged_owners` (crate `mojito-types`) | The container: a header (the package's identity and the byte ranges of the interface and the declaration directory) followed by bytecode records. The declaration directory is read in place: one string section holds every name once, and fixed-width tables hold the one module table, each declaration record's range, the struct table, one function row per library function in lowering order (name, declaration, runtime parameter count, `ref_params` mask, the range of the unit it lowers from; `PackageDeclarations::{bodies, body_row, body_range, ref_param}`) beside a by-name permutation, which answers a name prefix as one range (`PackageDeclarations::functions_prefixed`, `PackageBodies::{functions_prefixed, row}`), and the checker's key, method, and binding indexes (`DeclarationDirectory::{key_declaration, table_keys, methods_declaring, binding_position}`). The payload holds the interface (the package's declared names), one record per library declaration (its lowered part, the one exact value of the integer constant the declaration is included, naming each function by its directory row (`LoweredDeclaration::function_rows`), its struct, trait, and constant by the declaration's row, each effect summary by its function's index in the record (`LoweredDeclaration::function_index`), and each `ref_params` by its conventions (`FunctionDeclaration::conventions_mask`), all joined back on decode; then its checker residue as opaque bytecode, whose keys and projected names the checker restores from the declaration's directory keys), each module's link record as opaque bytecode (`PackageDeclarations::{modules, link_record}`; `PackageDeclarations::compiles` answers whether a statement comes from a package module), and one checked record per lowering unit (`CheckedUnit`: a module `def`, a struct method with its struct's name and lowered symbol, an inherited default's wrapper, a trait default, or a module constant; its `CheckedTables` (its checked expression and statement nodes, each holding its own op, type, and attributes, its declarations, its roots, and the locations of spelled sugars beside their constructions' nodes; a node's identity is its position, given back on install), beside the rows of its own declaration sites, a member's struct's parameter list left to the struct's declaration and given back by `CheckedUnit::program`'s `struct_parameters`; written under `encoding_occurrences`, which numbers each `SyntaxId` densely within the unit and names each source file by an index in the unit's own file table, and decoded under `decoding_occurrences`, which draws one fresh identity per number and keeps each derivation; its binding owners encoded under `encoding_packaged_owners`, R561, R611, R616). `PackageBytes` is the file mapped (`memmap2`) or owned when just built; `PackageDeclarations::{function, structure, lowered_owning}` decode a declaration record's lowered part on first lookup, once per process, keeping its verdict in the record's own cell (`PackageDeclarations::decode`; `failures` is the whole package's, for the artifact alone), and `residue` hands the check its bytes; `PackageDeclarations::{struct_entry, function_entry, trait_entry, owning}` name a lookup's record decoding nothing, and `ReachedFailures` is one compilation's record of the failed records its view reached (`PackageScope`, `InstalledInterfaces`, `mir::link::ImportedDeclarations`, whose `failures` `mir::verify` reports); `PackageScope` implements `OverloadLookup` by looking a name up: its overload set is the reached function declarations under the name and its `$ov$` run (`FunctionDeclaration::overload_member`), or, for a reached trait's `Trait.method`, its trait declaration's requirements (`TraitDeclaration::requirements`), a fieldwise constructor its struct declaration's (`StructDeclaration::fieldwise_parameters`), and a constant its record's (`LoweredDeclaration::constant`), under the program's own sets (`ProgramOverloads`); `PackageBodies::get` lowers a body's unit on first use, once per process (`PackageBodies::lowered`: `CheckedUnit::decode`, `CheckedUnit::program` over a view of the package's own `ProgramOverloads`/`ProgramConstants` and struct declarations of its own, as the body's finish and verify are, each noting the records it reaches that do not decode into the body's verdict, `PackageBodies::reached`, which installs the unit's tables as its checked program with no arena or syntax rebuilt (`CheckedProgram::installed`, whose program holds no statements), then `mir::lower_package_unit`), and finishes the body (the driver's `analysis::finish_package_body`, drop elaboration then the pre-elaboration pipeline, or `finish_package_body_without_pipeline` with the pipeline off; then `verify::verify_function_in`, its findings joined with the hook's own); `PackageBodies::findings` is that body's verdict (why its unit does not lower, or its finish's and verifier's findings), which the elaborator refuses when it materializes the body, and `PackageBodies::failures` every finished body's findings in the process, not one compilation's. `DeclarationRecords::{place_struct, place_function, place_transfer_effects, place_call_through_effects, place_constant}` put a lowered declaration or summary in the record of the declaration its name belongs to, refusing a constant not its declaration's own, a summary of no function of its record, and a function placed after a summary (`DeclaringModules::declaration_of`; `DeclaringModules::declare` adds a module constant the elaboration folded away). |
| Lex | `lexer::Lexer`, crate-level `lex` | Spanned token stream. |
| Parse | `parser::Parser::{parse_program, parse_program_diagnostic}`, crate-level `parse` | Spanned AST; diagnostic partial AST is quarantined. |
| Link | `module::{link_with_options, link_source_with_options, LinkOptions, ModuleError}`; with a package, `module::{link_with_package, link_source_with_package, inject_prelude_with_package}` (each a `module::LinkedProgram`: the statements and the modules read from records), and for a package build `module::link_source_recording`; `module::package::{PackageModules, PackageModule, ModuleIdentity}` (`PackageModules::entry`, `Linker::{resolve_module, find_at_root, load_packaged}`, `ResolvedModule`) | Dependency-first flat program with `SourceSpan` module identity, explicit-binding collision checks, an entry declaration of an explicitly imported name refused (`ModuleError::Redefinition`), an entry declaration whose name a loaded module links bare, a builtin identity, or a prelude name qualified by the entry module (`module::entry_item_name`, in `Linker::resolve_entry`), `std.*` resolved at the bundled root alone (`Linker::find_module`), canonical self-import checks, and provisional exports for mutual cycles. `builtin_module_exports` gives the docstring-only `std.traits`/`std.origin` homes and the `std.hashlib` `Hashable`/`Hasher` homes their builtin identity exports. An import resolves once to a `ResolvedModule` (path, canonical identity, dotted name) that every later step reads; at the package's library root the package's module table, keyed by dotted name and built from the declaration directory's, answers with no filesystem probe. A module a package records links from its record (loads by dotted name and export tables, bytecode; no declaration), with no source read; a package build records each module as it links it from source. |
| Comptime | `comptime::elaborate`, `ct::CtValue` | Ordinary AST with compile-time control resolved. |
| Check | `checker::{check_program, Checker}`, `checked::CheckedProgram` | Authoritative semantic handoff and side tables. A package callable's effect summaries are read from its declaration record on the first call naming it and kept in the pass's effect maps (`Checker::{callee_transfer_effects, callee_call_through_effects}`, `InstalledInterfaces::{transfer_effects, call_through_effects}`); `CheckContext::install` takes the reached modules' `InstalledInterfaces`, which every pass attaches (`Checker::attach_interfaces`) in place of the package's declarations: the declaration-keyed tables (`checker::declaration_table::{DeclarationTable, DeclarationSet}`) answer a key their own entries miss from its declaration's record, whose residue decodes on first lookup and joins its lowered part (`InstalledInterfaces::{declaration, has_key, keys, structs_declaring}`, `DeclarationInterface::join`), and a package module binding resolves below the module scope with a reserved owner (`Checker::installed_binding`, `InstalledInterfaces::binding`); a record that does not decode is reported by `InstalledInterfaces::failures`, which joins the records its package scope and effect lookups reached (`ReachedFailures`). `CheckContext::{record_interfaces, record_elaborated, take_interfaces}` give a package build its library export (`Checker::export_interfaces`, `checker::InterfaceExport`): one `checker::interface::DeclarationInterface` per library declaration, which `InterfaceExport::records` encodes as its record's residue less what the record's lowered part says (`DeclarationInterface::project`: struct facts, method signatures beside a `MethodResidue` naming its function by its index in the record, whose name `symbol::method_of_symbol` splits back into the method's name and overload qualifier, and a `def`'s type beside a `ProjectedBinding`, each compared byte for byte first), the check's effect summaries, and its integer constants as its check folded them (`InterfaceExport::checked_constants`, placed by `DeclarationRecords::place_constant`; each constant's integer row and elaborated value are left out by `DeclarationInterface::project` and joined back from `LoweredDeclaration::constant` by `DeclarationInterface::join`), after refusing a library whose lowered declarations imply other overload sets than its check scanned (`checked::declaration::declared_overloads`, `OverloadSets::differing_name`); a residue also carries its module constant as the build's elaboration resolved it (`checker::{ElaboratedConstant, ElaboratedConstants}`, read back by `InstalledInterfaces::elaborated(name)`) and its module-scope compile-time facts, which a scope lookup the checker's own scopes miss reads (`Checker::{installed_comptime_dtype, installed_comptime_parameter, installed_comptime_value, installed_type_alias, is_simd_length_binder}`). The checker's overload sets are a shared `checked::package::ProgramOverloads`, which an importer attaches over its `PackageScope` (`InstalledInterfaces::package_scope`) and `CheckedProgram::overloads` carries to MIR. Its module integer constants are a shared `checked::package::ProgramConstants`, folded once before anything spells a symbol (`Checker::fold_module_constants`), answered for a package name by `PackageScope::constant`, and carried to MIR by `CheckedProgram::constants`. |
| HIR | `hir::Cfg::{build_checked_fn, build_checked_fn_with_captures, build_checked_thunk, build_seeded_checked_region}` | Statement CFG over checked nodes: `HirExpr`/`HirStmt` name a `CheckedNodeId`/`CheckedStmtId`, `Cfg::slots` records the slot each local-name node resolves to, its checked binding's slot (`Cfg::bindings`), a CFG types no slot (MIR types each from its declaring op, a frame's parameters and named result from `lower_cfg_nested`'s `declared` map), and `Cfg::body` holds the statement nodes of the body the CFG lowers. `build_seeded_checked_region` lowers a `try` region's statement ids directly from its frame's `hir::RegionSeed` (slot spellings, binding slots, and the `except` binding whose slot the region declares); `build_checked_thunk` lowers an expression thunk's prologue, then returns its value, handed as its checked node (`FunctionLowering::returned`: a lifted request's `ThunkRequest::node`, a default's `CheckedFnParam::default`); every builder takes its body's statement ids (`FunctionLowering::statements`, a `def`'s or method's op's body, `CheckedTables::{roots, wrapper_roots}`, a trait member's `def` node, which its trait's `CheckedTraitMethod::default` names, a nested `def`'s `CheckedDef::body`). |
| MIR | `mir::{lower_checked_program, lower_checked_program_importing, lower_checked_program_units, lower_unit, lower_package_unit}`, `mir::link::{link_declarations, ImportedDeclarations}`, `mir::MirDeclarations::{function, structure, declares_trait, trait_declaration, trait_refines, materialized}`, `mir::MirProgram` | Fully register-typed A-normal IR, places, declaration metadata, source table. Lowering reads the checker's overload sets (`CheckedProgram::overloads`, one `Arc<ProgramOverloads>` every lowered function shares, through `symbol::OverloadLookup`). A struct's spelled methods and its trait-default wrappers (`CheckedProgram::default_wrappers`) lower through one `lower_struct_member` over a `StructMember` view, whose parameters are any `ast::DeclaredParam` (a method's `CheckedFnParam`s, read off its struct's op at `LoweringUnit::Method::member`, or a wrapper's `SignatureParam`s); a module `def` and a trait's member (`CheckedProgram::trait_members`) lower through one `lower_function`, which reads the `def`'s op, a trait member's its `def` node, which its trait's op names (`CheckedTraitMethod::default`). MIR reads no declaration from syntax: `struct_declaration` reads each root's `CheckedStruct`, and a default folds from its node (`CheckedConst::from_node`). A program lowers unit by unit (`CheckedProgram::lowering_units`, `lower_unit`), each struct's MIR declaration first (`struct_declaration`), a module constant's lifted applications after the module initializer's. An importer lowers its own declarations and attaches a package's MIR declarations, adding nothing to them, before closing and verifying them; every phase looks a declaration up by name through `MirDeclarations::{function, structure}`, own first, then the package's, decoded on first lookup. |
| MIR text | `mir::text::{disassemble, disassemble_functions, parse_artifact, verify_artifact, load_artifact, ParsedArtifact, ArtifactSourceMap, ArtifactReport}` | Canonical serialization, source-located Mojo-independent artifact parsing, and the parse-then-verify loading gate that maps canonical `mir::verify` findings to artifact spans. |
| Verify | `mir::verify::{verify, verify_concrete, concrete_function_findings, instruction_named_types}`; `mir::{instruction_slots, for_each_instruction_slot_mut, terminator_targets}` | `verify_concrete` is the mode for elaborated MIR: `verify` plus the rules of `verify/concrete.rs`, which reject symbolic types, compile-time parameters, layout queries, and any compile-time argument slot (`native::mono`'s `drop_parameter_slots` leaves a concrete call none, as Mojo's concrete call has no parameter list); `verify/scope.rs` is the parametric mode's binder-scope and kind rule for every type a body names (`docs/notes/param-expr-attributes.md` §Register types), and `instruction_named_types` the types an instruction names beside its places; `concrete_function_findings` is the same rules for one body, which `native::mono` asks while materializing an instance. `verify` is semantic verification of typed MIR: register/place types, concrete and inline abstract call contracts, variadic ABI conventions, CFG edges, effects, reference capabilities, loans, and interior origins. `verify/instr.rs` owns `verify_instruction`: the place and register checks every instruction gets, then a dispatch on the instruction's family over one `InstrCx`. Each family's checks sit with the rules they use: loans and interior invalidation in `loans.rs`, the iterator protocol in `iteration.rs`, subscripts in `subscripts.rs`, calls in `calls.rs`, and references, value construction, private storage, SIMD, and raise/`try` effects in `instr.rs`. `verify/drops.rs` (`verify_slot_ends`) is the one dataflow rule: a must-analysis of slots ended on every path, over the function's blocks, its `try` regions, and their escape edges, reporting a `drop.var` or `var.consume` of a slot every path reaching it already dropped, consumed, or moved; `mir::instruction_slots` is the slot inventory it revives by, and `mir::terminator_targets` the one successor function (the ownership analyses use it too). |
| Ownership | `analysis::check_ownership_program` (checked wrapper `check_ownership_checked`) | Move/init and loan validation over lowered MIR; a call into an imported package reads the callee's `ref_params` mask from the package's function table on lookup (`PackageDeclarations::ref_param`); `analysis/moves.rs` walks `try` regions per channel (`walk_region`/`walk_try`). |
| Drops | `analysis::{elaborate_drops_program, elaborate_drops_function}` | MIR with explicit `DropVar`/`ConsumeVar` operations, plus `DropPlace`s before writes that overwrite an initialized droppable place — a field store, and a whole place written through a reference (a `mut` parameter's or `mut self` receiver's `WriteRef`, a place pointer's or `ref` binding's `Store`) — from `analysis/store_drops.rs` (`elaborate_store_drops`, whose `out self` receiver test is `mojito_symbol::symbol::is_initializer_symbol`) dying named-struct copy constructions turned into their source's move by `analysis/copy_elision.rs` (`elide_dying_copies`, between `elaborate_drops` and `refine_deinit_fields`), or into its forwarding when bound to a `$fwd_arg_r` slot (`forwarding_use`, `forward`, `strip_forwarded_slots`), or lent to a `$tmp_arg_r` slot whose deaths become the source's (`lending_use`, `lend`, `rename_lent_slots`), and per-field `DropPlace`s for `deinit` parameters from `analysis/field_drops.rs` (`refine_deinit_fields`); re-verified before execution. Its dataflows (move states, register loans, drop liveness) visit blocks through `analysis/moves.rs`'s `Worklist` in reverse postorder, and a block's deaths come from one backward walk (`analysis/drops.rs`'s `BlockReplay`, shared by the body and every `try` region, where `scan.rs`'s `var_def` counts a `GetIter` as its iterator's def and `teardown` makes an input `DropVar`/`ConsumeVar` the slot's death, so the pass is idempotent, and `collect_try_defs` counts a handler's error binding as a def site at the handler's entry, so a `try`'s region locals stay out of the enclosing liveness) over `analysis/scan.rs`'s `DenseSet`; a `try`'s scope-exit cleanup holds its body's locals (`scan.rs`'s `region_cleanup_vars`, leaving out what `consumed_in_place` reports) and the outer values its body rebinds or uses that nothing observes after it (`drops.rs`'s `seed_raise_cleanup`, at a top-level and a nested `try`); an in-place `DefVar` (set by MIR lowering's `produced_in_place` for a non-raising call's discarded or declared result) takes its producer's deaths after it, its own slot first (`InPlaceDeaths`). Which operands an instruction loads by value, so their owners drop before it, is `analysis/register_loans.rs`'s `ArgumentPassing`, read from the callee's declaration, or for a call through a trait bound from the requirement the bound declares (`MirDeclarations::bound_requirement`, by the call's `__trait_dispatch.*` symbol; the compiler-known `Equatable`/`Comparable`, which the checker also names as a bound comparison's target, from `checked::declaration::compiler_trait_declaration`); the per-instruction result is `RegisterUses`, which the block-level drop-liveness fixpoints (`transfer_drop_liveness`, `transfer_region_drop_liveness`) count as the replay does. |
| Pre-elaboration pipeline | `analysis::{PIPELINE, BodyPass, run_pipeline, pre_elaborate_program, finish_package_body, finish_package_body_without_pipeline, ELABORATE_DROPS, pass_named, pass_names}` (`analysis/pre_elaboration.rs`); `analysis::{opt, OptError}` (`analysis/opt.rs`), the `mojito opt` CLI command, and `tests/mir_pass_test.rs` over `tests/mir_passes/<pass>/`; `Compiler::with_pre_elaboration`, `MOJITO_PRE_ELABORATION` | The ordered per-body passes over drop-elaborated parametric MIR, before the elaborator (empty today). `CompiledProgram::elaborated_mir` runs `pre_elaborate_program` over the program's own bodies but `__toplevel__`; an imported package's `BodyFinish` hook runs `finish_package_body` once per body; `run_pipeline` verifies after each pass in debug builds. The switch picks the hook, and `package::bundled_std_package(pre_elaboration)` imports the package once per setting. `pass_named` is the pass registry (every `PIPELINE` pass plus `elaborate-drops`); `opt` loads a MIR artifact, runs named passes in order, and verifies after each in every build, and the pass tests pin each case's `functions:` section. |
| Execute | `compiler::Compiler::execute`, `backend::Backend::run_concrete`, `backend::vm::VmBackend`, `artifact::run_artifact` | Production execution runs the cached `CompiledProgram::concrete_mir` through `run_concrete`, the one executable entry (`VmBackend::call_concrete` is its CTFE twin); a loaded artifact is elaborated the same way first. |
| Artifacts | `artifact::{run_artifact, ArtifactRunError}` | Load-then-execute composition for textual MIR artifacts: the `load_artifact` gate, then `native::mono` and `Backend::run_concrete`, shared by the CLI `exec` subcommand and tests. |

## Cross-Phase Contracts

| Concern | Sole owner | Consumers |
|---|---|---|
| Structural call binding | `call::{match_call_slots, match_fieldwise_slots, ArgSlot, CallSlots}` | Checker and VM call adapters; `match_fieldwise_slots` binds a `@fieldwise_init` construction's keywords to fields for the checker and MIR, which lists the registers in field order from `OverloadSets::fieldwise_parameters`. |
| Parser-to-call marker normalization | `call::{regular_marker_index, effective_keyword_only_index}` | Checker and MIR declaration lowering. |
| Callable identity, overload/dispatch, and native instance names | `symbol::{SignatureKey, VariadicKey, InstanceArg, OverloadSets, OverloadLookup, OverloadFamily, overload_family, CallableLookup, resolve_callable_symbol, resolve_method_symbol, instance_symbol, canonical_specialization_type, unqualified_instance_name, lowered_def_name, lowered_method_name, static_method_symbol, qualified_method_symbol, method_of_symbol}`; lifted functions' names `symbol::{lifted_constant_name, lifted_body_name, lifted_check_name, lifted_default_name, lifted_owner, LiftedOwner}` | Checker, MIR, VM, native monomorphization, symbol tests, and the package build, which places a lifted function in the record of the declaration `lifted_owner` names and refuses one whose owner is no function it packages. Runtime and native method retargeting share one declaration-view policy, which asks a `CallableLookup` table for an exact name and a base's `$ov$` family rather than taking a candidate list: the VM's `Prog` answers from its concrete functions, the elaborator's `Bodies` by name. A receiver-overloaded static (`Tuple.__len__()` beside the instance `__len__`) keys as `static_method_symbol`; a minted Tuple instance spells bare in overload keys and with its elements in instance names (`KeyMode`). An `instance_symbol` whose argument spelling passes 96 characters names its arguments by a SHA-256 digest (`W$mono$H…`), and a nested instance argument spells as its own symbol, so a symbol's length is bounded at any nesting depth. `lowered_def_name`, `lowered_method_name` (over a `MethodShape`), and the lifecycle-constructor name (`lifecycle_name` over a `LifecycleShape`; `lifecycle_method_name` for syntax) take any `ast::AnnotatedParam`; the checked arena's builder spells each declaration op's symbol and registered name with them once (`CheckedDef::symbol`, `CheckedMethod::{symbol, registered}`), and every later reader reads the op. `lowered_def_name`, `lowered_method_name`, and `MethodBinderOwners::scan` spell a module integer constant as the value their caller's constant reader answers, the check's (`checked::package::ProgramConstants`); `OverloadSets` holds no constant. A struct's variadic runtime-pack constructor beside a same-arity nullary one is selected by the VM's `constructor_name`, the monomorphizer's `runtime_pack_constructor`, and the native `constructor_init` alike. |
| Checked semantic facts | `checked::{CheckedProgram, CheckedTables, Walked, WalkResult, CheckedExpr, CheckedOp, CheckedStmt, CheckedStmtId, CheckedStmtOp, CheckedDef, CheckedSignature, CheckedFnParam, CheckedStruct, CheckedField, CheckedStructComptime, CheckedMethod, CheckedTrait, CheckedTraitMethod, CheckedTraitComptime, CheckedWithItem, CheckedParamArg, CheckedKwArg, CheckedSubscriptArg, CheckedComprehensionClause, CheckedTStringPart, CheckedConst, AnnotationSite, CheckedCallContract, CheckedIteratorCall, CheckedResultAdapter}` | MIR, ownership driver, backends. `CheckedProgram::tables` is the one `Arc<CheckedTables>` (expressions, statements, and declarations) that every `hir::Cfg` and MIR `Flatten` shares — lowering never copies program-wide tables. A `CheckedExpr` holds its own `CheckedOp`, whose `operands()` are its child nodes (an annotation's lifted applications are named apart, in `TypeValue`/`CheckedParamArg::Attribute`/`VarDecl`'s `applications`); a type operand is what the check resolved, never annotation syntax: `CheckedParamArg::Attribute` holds the `TyArg` of a type-shaped bracket argument or of a value-shaped one the check bound to a type or an origin (`NodeFacts::parameter_attributes`, recorded once by `Checker::record_bracket_attributes` from what the binder of the bracket list returned: the `BracketAttributes` beside `resolve_use_params`'s `BoundParameters`, `applied_type`/`applied_type_value`, `simd_dims`, and the builtin type constructors in `checker/type_resolution.rs`, scattered back to source positions through `ForwardedBrackets` where the struct origin partition (`PartitionedStructArgs::brackets`) or an explicit specialization (`split_callable_specialization`) kept origins back; a value-shaped one binding bound to a type or an origin also records `NodeFacts::argument_attribute`, in `resolve_param_arg`, `accept_origin_argument`, and generic-callee origin binding in `checker/generics.rs`) and `CheckedOp::TypeValue` its `Ty` (`NodeFacts::type_operand`, `Checker::type_value`), which MIR's `param_arg_reg`/`attribute_value_reg`, `param_arg_has_no_runtime_form`, `forwarded_value`, and `EnclosingBinders::named_by` read (the last also a `CheckedOp::Lambda` node's checked function type, in place of its hidden `def`'s annotations); `CheckedTables::{walk, walk_expression}` visit a body's statement and expression nodes in source preorder, a `Walked` visitor answering a `WalkResult` (`SkipBodies` leaves a statement's `CheckedStmtOp::bodies` or a lambda's `def` unwalked), as upstream's `Operation::walk` does; MIR rebuilds no body syntax: it lowers each expression through its handle (`Flatten::handle`, a `mir/node.rs` `Node`), which carries its node and HIR tree, spelling a name as its `Cfg::slots` slot through `name_spelling` (no checked expression is looked up by its source location), and literals and names lower from the node's op (`Flatten::literal_or_name`, `identifier_read`, `read_slot`, `name_place_root`, `value_binder_node` in `mir/lower_expr/expr.rs` and `mir.rs`). A `CheckedStmt` holds its own `CheckedStmtOp` (bodies as statement-id lists), binding, binding type, and declaration; a declaration op holds no signature syntax: a `CheckedDef` or `CheckedMethod` holds its symbol (a method also its registered name) and its `CheckedSignature` (compile-time `ParamDecl`s, each declared parameter's `Ty`, result `Ty`, `DeclarationEffect`), which `ArenaBuilder::stmt_op` reads from the check's declaration sites through `Signatures` (a package method unit takes its symbol and member index from its `LoweringUnit`, since its struct shell spells neither), `CheckedDefaultWrapper::signature` is filled the same way in `CheckedProgram::new`, and a `CheckedStruct`'s fields are `CheckedField`s holding their checked types; MIR's `lower_function`, `lower_struct_member`, `struct_declaration`, and `nested.rs` read these and look up no `AnnotationSite` but a nested closure's `FunctionType`; `CheckedTables::roots`/`wrapper_roots` are the module's and each default wrapper's root lists, and a trait's op names each default's member `def` node (`CheckedTraitMethod::default`), built once, where the trait is. MIR lowers an opaque statement by matching its `CheckedStmtOp` alone (`Flatten::lower_stmt`, `mir/lower_stmt.rs`), its operands the handles of `HirStmt::expressions` in the op's operand order, a `ref` declaration binding `HirStmt::declared`, and reads a `try`'s bodies as statement ids (`TryRegions::of`); `reassigned_names` walks the ops. No checked statement is found by its source location: nested discovery walks statement ids (`find_nested_defs`, `NestedSite`), each nested `def` and lambda and enclosing `comptime for` its node, a lambda found among its statement's own value expressions (`statement_lambda_operands`). Nothing rebuilds syntax from the nodes. A lifted thunk's request (`ComptimeThunks::{request, request_value, request_sequence, application}`) carries its returned node and no syntax, which `lower_fn_nested` hands HIR after the prologue (`Cfg::build_checked_thunk`). |
| Source annotation syntax | `ast::SourceType` (alias of the AST `Type` node) | Parser, checker input, HIR/MIR source metadata. |
| Source location/provenance | `token::{Span, SourceSpan}` | AST, checker side tables, MIR diagnostics. |
| Compile-time values | `ct::CtValue` | Elaborator, specialization, checked constants. `CtValue::Expr` is a residual parameter expression, `CtValue::Deferred` the slot of a binder (named by `ParamRef`) outside generic identity, and `CtValue::Marker` an elaborator-private `CtMarker` classification of a name; none is a constant. |
| Parameter expressions | `param_expr::{ParamContext, ParamExpr, ParamKind, ParamOp, MetaTy, ParamId, ParamRef, ParamBindings, ParamEval, ParamError, ConstraintVerdict, ParamConstraint, identity_eq, SIZE_OF_FUNCTION, builtin_application_value}` and `param_expr::fold::{fold_infix, fold_neg, fold_invert, compare}` (crate `mojito-types`) | The typed, canonical, per-compilation-interned form of a value argument, a value default, a callable default's condition, and a dependent type (`types::DependentType::Parameter`). `ParamContext` owns construction, canonical form, `rebuild` (an operator rebuilt from its parts as the constructors kept it, for the A1 payload's export and the MIR text reader), `replace` (type identity) and `evaluate`/`fold` (a required value); `fold` is the one implementation of compile-time scalar operators. `ParamKind::Apply` (`ParamContext::{apply, size_of, dtype_float_query}`) is a compile-time application, never folded by `replace` and equal by structure (`evaluate` answers a builtin one, `answer_builtin_applications`), carrying the value the compile-time route established when one has; the checker builds one for a module constant whose initializer applies a function (`Checker::applied_constant_expr`), and the elaborator keeps such a constant's name in every type argument (`CtMarker::Applied`). Users: the checker (`constraints.rs`), the elaborator (`eval.rs`), MIR text and the verifier, native monomorphization (`mono/symbolic.rs`), the VM's default resolution, and `symbol::mangle`. `types::{TyRewrite, rewrite_ty, replace_parameters, referenced_parameters}` is the one type traversal behind replacement. `TyKind::Simd`'s slots are `types::{SimdDtype, SimdWidth}`; a lane gather's mask is `types::LaneMask`, known or a template's `shuffle`/`slice`/`join` form, and `LaneMask::{close_with, resolve}` are the one closing and constraint rule the elaborator applies (`close_lane_masks`); `simd_ty_from_slots` is the only constructor of a symbolic vector type, and `simd_shape`, `simd_slots`, `is_scalar_simd`, `scalar_simd_dtype`, and `simd_lane` read the slots. A type binder is the same identity: `TyKind::Param { binder: ParamRef, .. }` and `ParamDecl::{Type, Value}.id` carry the declaration's `ParamId`, `types::TySubst` (`HashMap<ParamId, Ty>`) is every type substitution's key, and `types::CONTRACT_BINDER_OWNER` owns a canonicalized contract's slots. The identity crosses the waist on `MirInstr::ConstructTypeParam.param` and on a call's instantiated arguments (`TyKind::Param`), and native monomorphization's `Bindings.types` and `Bindings.values` are keyed by `ParamRef`. Design: `docs/notes/param-expr-attributes.md`. |
| Semantic types | `types::{Ty, TyKind, TyArg, ParamDecl}`; a callable type's signature is `types::FuncTy` (`TyKind::Func`), and `types::GenericFuncTy` (`TyKind::GenericFunc`) adds its declarations; `Ty::callable` reads either | Checker, checked data, MIR declarations, VM coercion. A `Ty` is a handle to a shared `TyNode` (private to `types.rs`), which holds its `TyKind` and caches its digest and closedness (`Ty::is_closed`); `Ty::kind` reads the variant, `Ty::new` (and `From<TyKind>`) builds one, sharing one node per dataless variant (`Ty::int()`, `Ty::none()`, …), `Ty::update` is the only in-place write, and `Ty::reusing` keeps a handle a rewrite left equal. `ParamContext::intern_ty` uniques the types the check records (`body_carry`'s `interned` and `uniqued`), refusing a type `Ty::carries_decorations` reports. Design: `docs/architecture.md` (Types). |
| Package type section | `type_section::{TypeSection, encoding_types, decoding_types}` (crate `mojito-types`) | Every type a package's records hold, once; inside `encoding_types` a `Ty` encodes as its entry's index and inside `decoding_types` decodes from it, its entry on first reference. `mir::text::package::write_package` runs every record encode (its `records` closure, which encodes the checker residues, included) in one scope and appends the section; `checked::package::Payload::{with_types, decoding}` holds the read section and runs every record decode (`PackageBodies`' units, `PackageDeclarations::lowered`, the checker's residues through `PackageDeclarations::decoding`) inside it. |
| Runtime values/operations | `runtime::{Value, coerce_checked, apply_infix, apply_prefix}` | VM and VM-backed CTFE. |
| Backend contract | `backend::{Backend, BackendKind}` | Compiler driver and CLI. |
| Phase timing (`--timings`) | `timing::{enable, enabled, span, round, count, report}` (crate `mojito-common`) | Every phase crate records spans; the CLI enables collection and prints the report; `scripts/bench-compile`, `scripts/instantiation-report`, and `tools/bench` parse it. Disabled, a span is one relaxed atomic load. |
| `print` keywords | `infer_print` (`checker/builtins.rs`) | The VM's `print` arm (`dispatch.rs`; `file=` writes through `host_write_bytes`), pliron's `lower_print` (`lower/print.rs`; a `print_sink` descriptor makes `write_stdout` call libc `write`), `mojito_types::types::is_stdlib_file_descriptor_struct`. |
| Constructed defaults (`dir: Optional[String] = None`) | `CheckedConst::Construct` in the callee's declaration | The VM's `bind_for_call`; the native monomorphizer's `instantiate_constructed_defaults` (enqueues the constructor instance for the parameter type and respells the default's target), pliron's `reachable_set` (follows the default's target) and `bind_call_slots` (runs the instance over fresh storage). |
| Evaluated defaults (`s: String = String("a")`) | `mir::lower_default` lowers the default as the zero-parameter function `$default$<owner>$<parameter>` recorded as `CheckedConst::Evaluate`; the checker's `dynamic_default_reference` rejects one naming runtime storage and the elaborator below MIR runs one like any function it demands | The VM's `bind_for_call` runs the function per call; pliron calls it from the caller (`evaluated_default_value` in `lower/calls.rs`), releasing a borrowed slot's value through `default_temps`. `native::mono` (`instantiate_constructed_defaults`) and pliron's `reachable_set` carry the edge no call instruction spells; a default function declaring the binders it reads is instantiated under its owner instance's arguments (`default_function_bindings`). |
| Keyword-pack collectors (`var **kwargs: T`) | The callee declaration's `kw_variadic`, collected into the `StringDict` instance `instance_symbol("StringDict", [T])` | The VM's `make_kwargs_dict`; `native::mono`'s `enqueue_keyword_collector` keeps the instance's empty constructor and `__setitem__`, pliron's `reachable_set` follows them, and `build_keyword_collector` (`lower/calls.rs`) runs them from the direct (`bind_call_slots`) and indirect (`bind_contract_slots`) caller. |
| Narrow float lanes (`Float16`, `Float32`) | `ast::Dtype::{is_narrow_float, round_lane, float_literal_lane}` over `common::float::{f16, round_f16, f16_bits}` and `literal::FloatLiteral::{to_f16, to_f32}` (crate `mojito-common`) | The VM's lane rounding, literal materialization, `to_bits`, and `__fma__` (`runtime.rs`); `CtLane::from_value` (`mojito-types/ct.rs`); pliron's `widen_float_lane`/`round_float_lane` (`lower/arith.rs`), `lower_narrow_float_binop` (`lower/binops.rs`), `narrow_float_constant` (`lower/emit.rs`), and `simd_round_float_vector` (`lower/simd.rs`). |
| `DType` values | `ast::Dtype::{code, is_integral, is_floating_point, is_signed, is_unsigned, is_numeric, is_float8, is_half_float, predicate, float_query, repr_alias}` with `DTYPE_PREDICATES`, `DTYPE_FLOAT_QUERIES`, the `DTYPE_*_MASK` bits, and `UPSTREAM_ONLY_DTYPE_NAMES` (crate `mojito-ast`) | The checker's `dtype_constant` (`checker/indexing.rs`, recording `SemanticAdjustment::DtypeConstant` for `DType.<name>` and a `SIMD` type operand's `dtype` via `member_type_operand`; `infer_member` records it for a value's `dtype` beside `length`), its `dtype_float_query` (recording `SemanticAdjustment::ParamValue`: the answer at a known dtype, `ParamContext::dtype_float_query`'s application over a binder), and its `TyKind::Dtype` method arm (`infer_dtype_predicate` in `method_calls/simd_receivers.rs`); MIR's `Const::Dtype` and `CheckedConst::Dtype`; the elaborator's predicate and float-query folds (`comptime/eval.rs`), a `SIMD` type's `dtype` in `associated_value` (`comptime/elab.rs`), and dtype equality (`compare_numeric_values`); the VM's `Value::Dtype` (display, `scalar_repr`, `hash_leaf_value`, the receiver arm in `invoke.rs`); pliron's `ScalarTy::Dtype` (`dtype_constant`, `dtype_text`, `lower_dtype_predicate`). A `DType` hashes as its `UInt8` code through `types::hash_leaf_ty`. |
| Scalar splat into a vector | `types::scalar_splats_into` (crate `mojito-types`) and `symbol::SCALAR_SPLAT_CONVERSION` (crate `mojito-symbol`) | The checker's `implicit_conversion_constructor` selects the splat as an implicit conversion; `checked.rs` turns its target into `SemanticAdjustment::SplatScalar` and `checked_call_boundary` (`method_calls/selection.rs`) into `CheckedCallValueAdjustment::SplatScalar`; MIR's `Flatten::splat_scalar` (`lower_expr/calls.rs`) emits the one-element `MakeSimd`. |
| Host call allowlist (`external_call`) | `mojito_types::ffi::{CType, FfiCallee, CALLEES, callee, accepts_arg, accepts_ret}` | The checker's `infer_external_call` (rejects any other callee, checks arguments and the declared return type), the VM's `backend/vm/libc.rs` table (std-only execution: descriptor table, `errno` slot, environment overlay, glibc `dirent` byte images, `_c_stat` fills by field name), and pliron's `lower/externs.rs` (on-demand `llvm.func` declarations, `open` variadic, real C calls). The callee travels as the call's first instantiated argument (`CtValue::Str`), which the VM's `call_named` (`CallTypes::instantiated`) and pliron's `lower_external_call` read; the result type is the destination register's checked type. |
| Compiler-called Mojo bodies | `stdlib/std/_intrinsics.mojo` (`_pow_int`, `_int_digits`, `_uint_digits`), named by `mojito_symbol::symbol::{POW_INT_SYMBOL, INT_DIGITS_SYMBOL, UINT_DIGITS_SYMBOL, DIGITS_BUFFER_BYTES, calls_pow_int}` | The Mojo implementations behind `**` on `Int`/`UInt` and integer display, which the builtin has no struct to hang a method on. `std.prelude` imports the module so every linked program carries them, and they stay out of `PRELUDE_EXPORTS`. Callers: the VM's `apply_binop`/`format_value`, the native monomorphizer's roots, and pliron's `reachable_set` plus `lower_pow`/`format_scalar`. |
| Generated string tables | `stdlib/std/_string_tables.mojo` (written by `scripts/gen-string-tables` from the pinned upstream `_unicode_lookups.mojo` and `_parsing_numbers/constants.mojo`) | Fixed-width hex records in string literals: the Unicode 16 case-mapping tables behind `StringSpan.upper`/`lower`/`isupper`/`islower` and the Eisel-Lemire power-of-five table behind `atof`; `string.mojo`'s `_hex_at`/`_hex_u64_at`/`_table_find` decode and binary-search them. Regenerate at every re-pin; never edit by hand. |
| String formatting | `stdlib/std/string.mojo` (`_FormatUtils`, `_FormatCurlyEntry`, `_PrecompiledEntriesRuntime`, and the `format` methods of `String` and `StringSpan`), the literal stand-in named by `mojito_symbol::symbol::{STDLIB_FORMAT_UTILS_STRUCT, FORMAT_LITERAL_METHOD}` | Upstream's `collections/string/format.mojo`, kept beside `String` because `format` builds a `String`. A string literal's `format` reaches `_FormatUtils.format_literal` through `infer_literal_format` (`checker/method_calls/intrinsic_receivers.rs`) and `SemanticAdjustment::LiteralFormat`; nothing in the VM or the native backend formats a template. |
| Native compile (`backend-pliron`) | `backend::pliron::{compile, compile_mir, CompileOptions, NativeModule, EmitKind, OptLevel, NativeTarget, JitValue, TrapCategory, PlironError, runtime_declarations}` | CLI `compile`/`run --backend pliron` and the capability-manifest differential harness. `compile` takes the driver's cached concrete graph and elaborates nothing; `compile_mir` elaborates drop-elaborated MIR from the entries a caller names. |
| Shared native ABI | `native::target::{Triple, CpuFeatures, NativeTarget, BuildConfig, OptLevel, EmitKind}`, `native::layout::{LayoutCx, StructFieldIndex, LayoutError, compose}`, `native::mangle::mangle`, `native::rt_abi` | Every native backend, the elaborator's answer to a layout query (`native::mono`, under the compilation's target; `LayoutError::Symbolic` is the one refusal of a type that still names a parameter), the CLI, `crates/mojito-runtime` agreement tests, and the LLVM cross checks. |
| The elaborator | `native::mono::{specialize, specialize_importing, entry_roots, SpecializedProgram, ElaborationCounts, GeneratorCount, MonoError, MonoErrorKind}` (`MonoErrorKind::Verify`: `Specializer::materialize_body` refuses a package body whose `Bodies::findings` are non-empty; `MonoErrorKind::Decode`: `Specializer::run` refuses an elaboration whose lookups reached a package declaration that does not decode, and `Specializer::materialize_body` a package body whose `Bodies::reached` is non-empty), `mono/bodies.rs`: `Bodies` (the program's own bodies and an imported package's, each decoded on first use; a by-name table answering a name, a name prefix, and a base's overload family, as upstream's symbol table finds an overload set, and each function's output position), `mono/catalog.rs`: `Catalog` (declarations by name; `prefixed` for functions), `mir::ConcreteMir`, `mono/specializer.rs`: `InstanceState`, `Specializer::{demand_application, materialize_closure, resolve_applications, select_comptime_branches, demand_layout}`, `LayoutOracle`; `mono/rebind.rs`: `Specializer::discharge_rebinds`; `mono/failure.rs`: `Specializer::discharge_instantiation_failures` (a reached method whose body failed to elaborate above MIR), `reached_failure` (a parameter constant an unrolled iteration could not compute, reported once the instance reaches it); `mono/gather.rs`: `Specializer::close_lane_masks`; `mir::prune_unreachable_blocks` | The driver, for the VM and the native backend alike, and artifact execution. Clones an entry-rooted concrete MIR graph from drop-elaborated MIR, which it leaves unchanged. `entry_roots` is the one definition of a whole program's roots (`main`, `__toplevel__`). Its output is a `ConcreteMir`, whose only constructor runs `mir::verify::verify_concrete`, which owns the concreteness rules. `mono/promote.rs` owns the one shape whose instance signature differs from its template's: a callable parameter bound to a closure that captures. A non-generic closure becomes the instance's last runtime parameter (`promote_to_runtime_parameter`); a generic one is folded, and its environment becomes trailing reference parameters (`append_environment_parameters`, `declare_environment_parameter`, recorded in `Bindings::callable_environments`) that the instance's devirtualized calls pass as the lifted body's captures. `Specializer::bind_capturing_callables` decides between them at the call site. Both renumber the variable slots they displace (`mir::verify::instruction_places_mut` is the shared place inventory, and `mir::for_each_instruction_slot_mut` the slot inventory, they walk). `mono/availability.rs` owns the verdict of a member's `where` clause (`MirFunctionDeclaration.availability`) under an instance's bindings, read from `MirStructDeclaration::{declared, conformances}` (a struct's declared conformances and synthesized lifecycle rows) and `MirDeclarations::{declares_trait, trait_refines}` when a clause asks it; the checker states those facts in `Checker::conformance_facts` (`checker/traits.rs`), handed over as `CheckedProgram::conformances`, for the program's own structs and traits only, and answers no struct for a trait ahead of use. `mojito_types::conformance::{struct_trait_rule, refines}` is the rule the checker and the elaborator share for a struct's conformance and trait refinement. `mojito_types::conformance::leaf_conforms` is the one rule for a compiler-known type's conformance to a built-in trait, shared by the checker's `conforms_to` and the elaborator. `INSTANCE_BUDGET` (`mono.rs`) is the one elaboration bound, checked in `Specializer::enqueue`; `specialize` runs the elaborator on its own deep-stack thread, so its pass statistics (`ElaborationCounts`, built by `mono/specializer.rs::elaboration_counts` from the instance list and the concrete output) travel back in `SpecializedProgram::counts`, and `CompiledProgram::concrete_mir` reports them on the driver thread, whose timing log is the one reported. |

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
  (`StructInfo`, `MethodSig` with its `SignatureDefault` attribute — `Constant`, `Parameter`, or `Evaluated` — overload helpers, `ConformanceOracle`), and the
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
  A trait member (`checking_trait_member`) may take a `deinit` receiver,
  and a parameter named `self` is writable by its own convention.
- `checker/with_stmt.rs` owns the `with` statement: `check_with` classifies
  the manager into a `WithForm` (`context_manager_form`: consuming
  `__enter__`, plain and error-taking `__exit__` overloads, upstream's
  rejections), synthesizes the desugar (`Synth` node factory, whose node
  identities are `SyntaxId::derived` from the statement's, hidden
  `$with<id>_*` names) and checks it in a block scope, recording a
  `WithDesugar` (its statements); and
  `splice_with_desugars` replaces every checked `with` in the final tree
  before `explicit_destroy`; `KEEP_ALIVE_BUILTIN` names the
  `_mojito_keep_alive` liveness anchor the call inference accepts and MIR's
  `lower_stmt.rs` lowers to `KeepAlive`.
- `checker/bound_defaults.rs` owns the requirement defaults bound at a call
  through a trait bound: `record_bound_default_arguments` (called from the
  bound branch, `resolve_bound_method` in `method_calls/resolution.rs`)
  records, by the call's
  origin, each omitted requirement default some conformer's witness
  declares otherwise (`witnesses_default_alike`), the compilation's
  `CheckContext` keeps them across passes
  (`CheckContext::bound_default_arguments`, `BoundDefaultArguments`), and
  `bind_bound_default_arguments` spells them as keyword arguments of the
  call, before a pass and again, with another pass, when a pass found new
  ones (`check_program_carrying`). `requirement_default` in
  `checker/traits.rs` makes each requirement default its signature's
  attribute when the trait is declared: a literal constant, module
  constants folded (`comptimes`, `comptime_literals`, both registered
  before the trait pass), or, for one reading the method's value parameters
  (`reads_value_parameter`), a `ParamExpr` over them
  (`compile_dependent_ct_expr`). `spelled_parameter_default` evaluates that
  expression with the call's solved binding of the parameters and
  `spelled_parameter_value` spells the value in the caller's scope; a
  constant is spelled by `checked_literal`. A generic call (`ExprKind::Invoke`) binds
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
  on a `TyKind::Variant` receiver (the bundled `Variant`'s `_storage` field)
  through `infer_method_call`, which the parameterized `Invoke(Member)`
  spelling and the ordinary method call both dispatch to after the static
  receiver arms (a type-name receiver is never inferred as a value); the
  public API is
  `stdlib/std/utils/variant.mojo`, whose type-keyed methods `native::mono`
  instantiates at each call's type argument, and `type_keyed_projection`
  routes a `v[T]` subscript to the struct's type-keyed
  `__getitem_param__[T]` accessor in value and place positions (the
  `type_keyed_accessor_call` lowering in MIR, which reads the subscript's
  index node). `check_lambda` runs a lambda
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
  selects a signature per receiver family (struct, bound, builtin),
  `receiver_effects.rs` checks the receiver
  and argument conventions and builds the reference result, and
  `call_contract.rs` records the `CheckedCallContract`. `selection.rs`,
  `statics.rs`, and `builtin_types.rs` hold the scoring, static, and
  pointer/List helpers, and `mlir_op.rs` the one `__mlir_op` statement
  the bundled modules spell. `mc_infer.rs:infer_selected_method_call` is the
  tail every selected signature takes (overload target, effects, receiver
  and argument contracts, result). A member of `Tuple` or `TString` is an
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
  the sugar's location; the sugar's node operand is that construction, so
  HIR lowers it in the sugar's place. Together they own method-call inference (including the inverted write:
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
  `mut`/`ref` argument places and solving a reference result's origin over
  the argument slots via `solve_call_origins_with_bool_bindings`, as a free
  call does, and alias-checking;
  both method paths and the constructor paths record
  `checked::MethodInstantiation`s, which name the method's template
  (`method_calls/resolution.rs:record_method_call_instantiation`); the
  method-call, static, and constructor paths also record every closed
  generic-struct application reached from a non-bundled source
  (`record_struct_instantiation` → `checked::StructInstantiation`), and
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
- `checker/trait_defaults.rs` owns trait defaults as upstream's
  `StructEmitter` emits them: each default is its trait's member
  `Trait.m` (`member_symbol`), whose declaration
  `trait_member_declaration` builds from the trait statement (first
  compile-time parameter `_Self`, receiver first) for the elaborator, the
  check, and the handoff alike. `Checker::check_trait_members` checks every
  member after the module walk and `checked_trait_members` hands it over
  as `checked::CheckedTraitMember` (`CheckedProgram::trait_members`). A trait's
  check records its defaults in its `TraitInfo` (a `DefaultShape` from
  `default_shapes`, naming its requirement row in `TraitInfo::methods` by
  overload index), which a package exports. The check builds each conformer's
  wrapper per inherited overload (`inherited_defaults`, `default_wrappers`,
  `DefaultWrapper`): `Checker::plan_default_wrappers` counts it in the
  overload prescan, `register_default_wrappers` registers its signature
  from the default's, its binders reowned
  (`types::reown_generic_signature`), after the struct's spelled methods,
  `check_default_wrappers` checks its one statement after every trait
  member, which calls the trait's member with `Self` bound first and every argument forwarded, and
  `checked_default_wrappers` hands it over as
  `checked::CheckedDefaultWrapper` (`CheckedProgram::default_wrappers`).
  A wrapper declares no default of its own: MIR's `adopt_member_defaults`
  gives its declaration its member's, an evaluated one applied to the
  wrapper's instantiation of the member (`CheckedConst::Evaluate`'s
  `arguments`), which `native::mono`'s `default_function_bindings`
  substitutes.
  `inherited_destructors` names the inherited `deinit`-receiver wrappers
  the struct shell records. It reports the pin's conflicting default and
  struct-parameter name-conflict rejections. A method body's check binds
  and walks a spelled method and a wrapper alike
  (`declarations::{MethodBody, BodyParam}`, `Checker::{bind_receiver,
  bind_parameter, check_method_body}`).
- `checker/traits.rs` owns trait/struct declaration checking, conformance
  (nominal and built-in), and type-capability queries (`is_deinitable`,
  `is_movable`, `is_copyable`, …, and the three-valued
  `register_passability`, which `conforms_to(_, RegisterPassable)` and a
  result origin read). A type parameter is movable or
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
  `lower_ref_sig_resolved`; `origins/sig.rs`'s `check_result_origins` is the
  one rule for which parameter or receiver a result may name the origin
  of, called once per struct method, trait requirement, and `def`
  signature), and cross-call transfer effects (`abstract_body_origin`,
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
  of the range it had (`owner_range_split`, which `body_carry.rs` reads).
- `checker/body_carry.rs` owns carrying a body's facts from one checker pass
  to the next. `PassCarry` is what `checker.rs:check_program_carrying`
  returns (the `HandoffStores` that `HandoffStores::into_discovery` moves
  into the `DiscoveryResult`: the occurrence records and the declaration-keyed
  stores; the checker-internal stores; one
  `BodyRecord` per site, the identity watermark); `carried_stores!` lists
  every store a site is measured and copied over, and is also the store
  list `checker/probe.rs` rolls back; `Checker::carry_body`
  serves a site whose record is clean and whose effect reads (recorded by
  `note_body_effect_read` beside each `effect_observations` write) are still
  current, and `enter_body_site`/`leave_body_site` bracket an inferred one
  (from `statements.rs:check_def` at module level and
  `declarations.rs:check_method_inner`); the carry is private to the
  passes of one check (`checker.rs:check_program_carrying`).
- `checker/node_facts.rs` owns the checker's access to its one
  per-occurrence store (`Checker::node_facts`, a logged `FactMap` of
  `NodeFacts`): `record_at` is the write path (the record, created if
  absent), `fact`/`has_fact` read one fact, and `clear_fact`/`clear_flag`
  take one out without creating a record. A guard on the store is never
  held across a call that writes a fact.
- `checker/probe.rs` owns `Checker::probe`: it opens a `ProbeMark` on every
  carried store, the deletability sets, and `call_bindings` (`ProbeMarks`),
  runs a speculative check, and rolls every store back. Its callers are
  `call_inference.rs`'s `TyKind::Overload` loop and `generics.rs`'s
  overloaded callable-value branch; each re-runs the selected candidate
  outside the probe.
- `checker/comptime_validation.rs` owns the check of compile-time control
  flow in the one check, over every function body with its declaration's
  parameters symbolic: `check_comptime_condition` types a `comptime if`
  condition (a generic constraint, a concrete `conforms_to`, or a `Bool`
  value; `fold_closed_constraint` folds a closed one to its verdict and
  `reject_dynamic_condition` is the pin's rejection of a runtime local),
  `check_comptime_for` checks a loop body under its element type,
  `bind_template_comptime` binds a local `comptime` binding over the
  body's binders (at module scope, a constant's lifted initializer as
  `$comptime$<name>$module`, `binderless_comptime_expression` over
  `module_lifted_application`, which `statements.rs:check_module_block`
  also runs ahead of the first body after which the initializer types,
  through `predeclare_lifted_constant`), and
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
  symbolic (a shuffle, slice, or join only at a known width). The same page
  owns a pack that is still a parameter:
  `pack_reference`/`unbound_pack_named` resolve it (scopes in
  `Checker::pack_params`, `annotations::pack_scope`), `pack_element_type`
  builds the dependent element (`ParamContext::list_get`,
  `DependentType::pack_element` in `mojito-types`), `opaque_element` is its
  bounded `TyKind::Param` view and `restore_pack_elements` the way back,
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
  unroller's per-copy call) writes as the element's default construction (an
  alias substitution above the check spells it through
  `comptime/rewrite.rs:pack_element_construction`; `Tuple.__init__(out self)`
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
  Pliron's `lower/drops.rs:clear_presence` clears the leaf flag for),
  `forwarded_pack`/`forwarded_pack_argument` recognize a spread of it as one
  call argument (its placement through `call.rs:spread_position` and
  `bind_spread`) and `bind_forwarded_pack` binds it whole to a callee's pack
  collector (bounds, ownership, and the recorded `owned_packs`/
  `owned_collectors`), at the overflow loops of
  `call_inference.rs:infer_generic_call`, `generics.rs:instantiate_method_generics`,
  `method_calls/selection.rs:score_method_call`, and `builtins.rs:infer_print`,
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
  `native::mono`'s `substitute_ty` closes per instance, and
  `comptime/eval.rs` reverses and concatenates a closed `TypeList` above
  the check.
- A `def` body and a struct method's body are each checked once, with
  their binders symbolic, by `check_block` (`checker/statements.rs`; a
  method's from `declarations.rs:bind_and_check_method`, under
  `Checker::method_site`); `count_body_inference` (`checker.rs`) keeps the
  `body_inference.plain`/`body_inference.template` timing counters, and
  `remove_occurrence_facts` (`checker/statements.rs`) clears an
  occurrence's facts before an augmented store re-infers it. What an
  instance still owes below the waist is `docs/notes/generator-contract.md`.
- `operators.rs:vector_alias` types a vector alias's call (`U256(...)`) as
  the `SIMD` it spells; `operators.rs:struct_infix_dispatch` is the
  type-level half of `infer_infix` and `struct_reflected_dispatch` its
  reflected half; `operators.rs:borrow_operator_operand` marks a place
  operand a read dunder takes (a named struct or a by-reference element)
  `BorrowReadArgument`, for infix, reflected, and prefix operators alike.
- `checked.rs`'s `DiscoveryResult` is `CheckedProgram::new`'s internal
  input, built by the check before its arena is: the check's facts as one
  `NodeFacts` record per occurrence (`node_facts`; a record encodes as the
  list of the facts it holds) beside the declaration-site tables, with
  `finalize` for the arena (`check_program_in`, the driver's entry, in the
  compilation's `CheckContext`). The declarations are made by a walk over
  the built statement nodes (`CheckedTables::declare`, `Declarer`), each
  declaring node naming its own; a `def`'s explicit captures are its op's
  (`CheckedDef::captures`). The arena builder (`build_checked_expressions`,
  `ArenaBuilder`, `node_adjustments`) reads a node's facts from its
  occurrence's record, and runs once per program: a package unit's
  tables are the program's subtree (`CheckedTables::extract`, with
  `StructShell` for a member's struct and the `NodeMap` that
  `CheckedOp::map_operands`/`CheckedStmtOp::map_operands` renumber
  operands through, in build order).
- `fact_store.rs` owns `FactMap`, `FactSet`, and `FactVec`: a `HashMap`,
  `HashSet`, or `Vec` that logs every key written, read through `Deref`,
  with `mark`/`logged` for a range of the log, and `ProbeMark` with
  `open_probe`/`roll_back`, which journal every write while a probe is open
  and undo it. Every checker fact store is one, so `checker/body_carry.rs`
  can copy exactly what a body wrote and `checker/probe.rs` can undo what
  an overload candidate wrote.
- `explicit_destroy.rs` owns the explicit-destruction analysis, run from
  `checker.rs`'s `run_explicit_destroy` once over the elaborated program,
  where `check_comptime_if` joins the arms of a condition naming a parameter as
  branches (`join_parametric`) and the arms of any other condition as
  alternatives (`join_comptime`). `LINEAR_TYPE_PARAMETER` (`$linear`) keys
  the obligation of a value typed by a non-`Deinitable` type parameter;
  `Env.linear_temporaries` carries the call results the checker recorded as
  owned-but-unconsumed (`places.rs`'s `record_linear_temporary` and
  `record_unconsumed_temporary`, intersected in `run_explicit_destroy`),
  reported as `'(expression temporary)'`. `SpanFacts::owned_temporaries`
  (the rest of the recorded linear temporaries) and `SpanFacts::raise_points`
  (raising call effects, `conversion_raises`, and the checker's
  `raising_iterations`) give the walk its raise edges: `check_expr` consumes
  an operation's in-flight operands and pending transfers
  (`consume_operands`) before `raise_edge` checks what is still owned and
  records the state on the innermost `RaiseFrame`, and `check_try` starts an
  `except` arm or `finally` body from their join (`join_parametric`), then
  `join_try` joins the arm with the normal exits. A write through a reference (a
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
  rejects writing through a by-value (`TrivialRegisterPassable`) rebind.
  The eraser turns `rebind[Dest](x) = value`
  into the plain `Assign` of `x`, whose retyping `rebind_assignment_target`
  applies from the `Assign` arm and `record_assignment_rebind` records,
  reversed, on the assigned value. MIR lowering reads a rebound operand
  through `Flatten::rebind_value` (`MirInstr::Rebind`) or
  `Flatten::rebound_place` (a `Proj::Rebind` step), both in
  `lower_expr/entry.rs`; every pass above the elaborator reads a place
  through `MirPlace::storage`, `steps`, or `is_whole_root` (`mir/ir.rs`),
  which see through the step, and `native::mono`'s `discharge_rebinds`
  (`mono/rebind.rs`) judges and erases both forms per instance, each
  place step against the type it applies to (`verify::place_base_ty` for
  the first). The parser admits the call as an
  assignment and augmented-assignment target (`parser/stmts.rs`).
  Every body is checked once with the operand symbolic, so no body is
  keyed on a `rebind`.
- `checker/constraints.rs` owns compile-time evaluation and generic-constraint
  compilation/evaluation. `compile_where_clause` compiles a clause,
  `bind_constraint`/`bind_declared_constraints`/`compile_condition` bind its
  operands to their binders (`GenericConstraint::bind`), and
  `ConstraintEnvironment` is the identity-keyed argument environment every
  verdict reads. Its evaluators resolve names and delegate operator
  semantics to `param_expr::fold`; `compile_dependent_ct_expr` builds a
  `ParamExpr` through the compilation's `ParamContext`
  (`Checker::param_context`, the compilation's `CheckContext::param_context`),
  and `value_parameter_in_scope`/`push_param_scope` resolve a bare value
  parameter to its owned reference (`annotations.rs`: `binder_owner`,
  `Checker::method_binder_owner` over `symbol::MethodBinderOwners`, which
  the elaborator's `elaborated_binder` shares,
  `value_parameter`, `params_as_args` over `ParamDecl::own_argument`).
  The same table's `call_qualifier` names the overload a method-call
  record selects (the qualifier of the symbol `lowered_method_name`
  declares, which a call records verbatim).
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
  source alive and loaned. It also records `IterationProtocol::iterator_ty`,
  the `__iter__` result those two `GetIter` sites type a split iterator slot
  with: a conformer's from `struct_iteration_protocol`, a bounded
  parameter's from its bound's own `__iter__` (`generic_iterator_ty`), with
  `origin_of(self)` resolved to the named source by
  `resolve_iterator_origin`. A borrowed loop over a named homogeneous
  runtime pack yields element references (`pack_iteration_reference`),
  which `IterationProtocol::indexes_pack` names for MIR. Both MIR loop sites
  normalize through `Flatten::normalize_iterator` and advance through
  `emit_has_next`/`emit_next` (`lower_stmt.rs`), which index such a pack in
  place through a `PackCursor` (`mir.rs`) opened by `open_pack_cursor` over
  the place `borrow_iteration_source` records in `Flatten::pack_sources`.
- `checker/calls.rs` adapts neutral call matching to `TypeError` and validates
  checker-only signature rules.
- `checker/places.rs` owns call-site place classification and alias rejection
  (the pointer keyword subscript `p[unsafe_offset=i]` is a place:
  `pointer_offset_keyword_subscript`), and how a selected call's read
  parameters bind their arguments (`record_argument_borrows`: the borrowable
  place reads and the owned temporaries the caller destroys after the call,
  `read_temporary_arguments`; the arguments a read `*args` collector gathers
  bind the same way, a collector's positions and convention spelled
  `Gathered` by `Checker::gathered_arguments`,
  `MethodCallResolution::gathered_arguments`, and
  `ConstructorBinding::of_signature`; the arguments and receiver a regular
  `var`/`deinit` parameter owns, and the elements an owning collector
  gathers, are marked `ForwardedArgument`, `forwarded_arguments`, and
  so is an operator operand an owned dunder parameter takes,
  `record_forwarded_operand` from `checker/operators.rs`; a construction binds through it from
  `Checker::record_constructor_call` in `checker/declarations.rs`, which
  also makes a raising `__init__` a raising call and alias-checks the
  arguments as an initializer call through `check_free_call_aliasing`).
  `Checker::designates_place` is the semantic place test: a syntactic place
  other than a subscript whose selected getter returns by value.
- `checker/generics.rs` owns unification, substitution, and callable/method
  specialization; `solve_value_args` solves value binders from argument
  types, a vector's width slot solving only a binder declared `SIMDLength`
  (`Checker::simd_length_binders`, recorded by `classify_params`);
  `substitute_variadic_at` types a member's struct-pack collector
  (`*b: *Self.Ts`) at a receiver as the runtime pack of its elements, a
  forwarded caller pack retyped with the struct pack's bounds.
  `solving_argument_ty` types an argument before its callee's binders are
  solved: a display against `List[K]`/`Set[T]`/`Dict[K, V]` takes its open
  element binder from its own elements, the bracket bindings
  (`declarations.rs:explicit_type_subst`) substituted first; every generic
  call path then checks each argument against its solved parameter with
  `infer_with_expected`, last, so a display records that collection.
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
  responsibility across `impl<'a> Flatten<'a>` blocks in the submodules below.
- `mir/node.rs` owns `Node`, the handle MIR lowering reads a checked
  expression through (HIR's tree with its checked node), and its operand
  accessors (`operands`, `operand`, `operand_of`, `object`, `index`, `args`,
  `kwargs`, `param_args`, `param_value`, `location`).
  `mir/facts.rs` owns the node's attribute readers (`Node::{ty, place_ty,
  owner, raises, adjustments, call_contract, instantiated_args,
  reference_places, interior_references, element_receiver, reference_result,
  implicit_conversion, index_normalization, recorded_value,
  intrinsic_index_dispatch, pointer_offset_subscript, rebound_place,
  is_fresh_temporary, …}`; a fresh temporary is a call's, a dispatched
  operator's, or a by-value subscript's result) and
  the `Flatten` readers that also need the function being lowered
  (`name_spelled`, `expression_var`, `nested_info`, `aggregate_borrows`,
  `subscript_call_contract`, `call_capture_accesses`, `local_callee`).
  `Flatten::expr` and its adjustments (`lower_expr/entry.rs`), the place
  family (`place`, `try_place`, `simple_place`, `pure_field_place` in
  `lower_stmt.rs`, `name_place_root` in `mir.rs`), `reference_handle`, the
  compile-time argument slots (`param_arg_regs` over the op's
  `CheckedParamArg`s), and `mir/calls.rs`'s argument, receiver, anchor, and
  invalidation lowering take a `Node`, and so do statement lowering
  (`lower_stmt`, the subscript-store and augmented-subscript family,
  `lower_bind`, `bind_comptime_value`) and `mir/calls.rs`'s
  augmented-subscript sources; `Flatten::with_statement_loans` brackets each
  statement and expression root. No syntax bridge remains.
- `mir/calls.rs` owns call-site lowering (arguments, keywords, receiver,
  reference results, checked-call boundaries, interior-origin invalidations)
  and call setup: `CallSetup` is one open call's frame of deferred by-value
  place reads and owned-operand transfers (`forwards`), opened by
  `open_call_setup` (or `lower_setup_receiver` /
  `lower_setup_receiver_with`, which lower the receiver into it) and emitted
  in parameter order, reads before transfers, by `close_call_setup`; `lower_setup_operand` lowers one
  operator operand into the frame (its `bound` flag set for a call argument
  or display element, clear for a receiver or operator operand),
  `display_elements` lowers a display's elements through a frame of their
  own and, for a bound display that `setup_display` selects (a list or brace
  display), hands its element reads and construction to the enclosing frame,
  `outside_call_setup` keeps non-operand lowering out of it, and
  `setup_read` decides which places defer. `bind_read_temporary` binds an
  operand the checker marked `ReadTemporaryArgument` (a read call argument,
  a method's read receiver through `lower_method_receiver`, a struct or
  prefix operator's read operand) to its `$tmp_arg_r` slot
  (`mir::READ_TEMPORARY_SLOT`).
  `bind_forwarded_temporary` binds a fresh temporary marked
  `ForwardedArgument` (an owned argument, or an owned receiver through
  `lower_method_receiver`) to a `$fwd_arg_r` slot
  (`mir::FORWARDED_ARGUMENT_SLOT`), in place when a non-raising call made
  it, that the call takes through `UseVar Forward` at its setup
  (`defer_to_setup`); `Flatten::transfer`
  (`lower_expr/expr.rs`) lowers a forwarded whole-slot `x^` to `UseVar
  Forward` (`UseMode::Forward`, `UseMode::moves`), and `Flatten::ternary`
  (`lower_expr/ctrl.rs`) forwards a conditional expression's result slot
  to its consumer the same way.
  `setup_reference` decides which reference-result operands defer their
  read: `defer_to_setup` marks the operand in `Flatten::setup_reference`
  (`SetupReference`), `materialize_reference_result_place` gives its handle
  slot setup loans (`MirLoan::setup`) and starts the read there, and
  `awaits_setup_read` routes `reference_result_value` to that slot. Both
  decisions walk the operand's accessor chain with `place_chain`, which
  stops at a static call's type receiver (`type_receiver_name`).
  In `mojito-analysis`, `moves.rs`'s `MoveContext::setup_owners` reads a
  setup slot's owners at each read through it, and `loans.rs` exempts setup
  loans from access and loan exclusivity.
- `mir/lower_expr.rs` (split across
  `lower_expr/{entry,ctrl,expr,expr_call,expr_method,expr_access,calls}.rs`)
  owns expression lowering (the `expr_unconverted`
  dispatcher, collections/comprehensions, nested closures, the
  field-invocation indirect-call branch). `expr_unconverted` in `expr.rs`
  first dispatches on the node's `CheckedOp` for literals and names
  (`literal_or_name`), for calls, invocations, method calls, and callable
  type applications (`call_family`, `call_op`), and for member reads,
  subscripts, slices, multi-subscripts, and `Variant` projections
  (`access_family`), and for list, brace, and tuple displays and
  comprehensions (`display_family`, reading the construction through
  `ctrl.rs`'s `collection_plan`/`array_literal_plan`/`dict_literal_plan`; a non-empty dict display is `Dict`'s `__dict_literal__` call over two `List` literal calls), whose entry points take the node and read its
  attributes and adjustments, and for prefix and infix operators,
  comparison chains, ternaries, transfers, and lambdas (`operator_family`,
  reading the operator from the op; a spread, type value, or t-string
  reaching it is an explicit `Unsupported` boundary). Every family takes the
  `Node` and reads each operand off the handle by the id its op names
  (`Node::{object, index, args, param_value, operand_of}`); the call
  family reads the callee (`Node::callee`), receiver, arguments, keywords,
  and compile-time arguments off the handle too. An expression none of
  the five families takes is one explicit `Unsupported` boundary. `expr_call.rs` owns direct calls (`call_expr`,
  `checked_construction`, `direct_call`) and callable-value invocations (`invoke_expr`,
  `variant_operation`, `parameterized_method_call`); `expr_method.rs` owns
  method calls (`method_call_expr`, its value and storage special forms,
  `ordinary_method_call`, `type_receiver_name`); `expr_access.rs` owns
  member, subscript, slice, and variant-projection reads (both projection
  spellings share `variant_payload_read`, and both pointer subscript
  spellings share `pointer_subscript_read`, a borrowed pointee read
  followed by the checked `copy_boundary`). The place builders
  (`lower_stmt.rs`), `lower_projected_reference_place`, and
  `Flatten::pointer_keyword_subscript` (`mir.rs`) decide by the op, and
  `Node::{member_object, index_operands, transfer_operand,
  spread_operand, named_value}` (`facts.rs`) bind a member, subscript,
  transfer, spread, or walrus node's operands for the predicates. The module
  installs merged caller-side
  `EstablishLoans` — domain-keyed for interior-precise destinations — for
  checked call-transfer records (`install_call_transfers`, reading the call
  node's `call_transfers`) after free, method, indirect, and nested calls.
- `mir/lower_stmt.rs` owns statement, place, subscript-assignment, `try`-region,
  and terminator lowering (with `region_crosses_control`), plus the
  borrowed-iteration source binding and loan re-establishment helpers shared
  with comprehension lowering.
- `mir/nested.rs` owns capture analysis and nested-function lifting; each
  lifted declaration names the declaration it is nested in
  (`MirFunctionDeclaration::enclosing`, set by `lower_nested_node`), and the
  nested frame declares its enclosing callable and type binders' locals
  (`Enclosing`). `verify/scope.rs` (`Scope::of_function`) walks that chain
  for the binders a body may name; `native::mono`
  (`Specializer::instantiate_nested_bodies`, `nested_bindings`,
  `binder_scope`) instantiates a nested body under its enclosing instance. A
  declaration inside a kept `comptime for` also declares the loop indices
  (`MirFunctionDeclaration::region_binders`, from `find_nested_defs`'s stack
  of loop statement ids and `nested.rs::loop_index`, which reads the
  iterable through the statement's `CheckedStmtOp::ComptimeFor`); `mono/unroll.rs`
  (`instantiate_region_references`, `region_generator_slots`) points each
  region copy's closure at its instance, a generic one at the copy's
  generator (`Specializer::region_generators`).

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
  dispatch loop (`call_frame`/`make_frame`/`prepare_direct_call`).
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
  declarations without selecting or cloning, leaving each trait default in
  its trait, whose body the elaborator's trait arm elaborates as its
  member's (`checker::trait_member_declaration`);
  `prepare_importing` prepares an importer's own statements beside the
  package's elaborated SIMD aliases, reading no package declaration; `elaborate_prepared` and
  `elaborate_importing` (an importer's own statements, `ImportedScope`
  under its module environment: a name the own frame misses is the
  installed package's constant, decoded on first lookup through
  `ConstEnv`, `Elab::{module_constant, is_generic_alias, generic_alias}`,
  and the module block's `elab::TypeAliases`; a package
  struct or `def` read through `Elab::{ct_struct, declares_struct,
  declares_function}` from the oracle's
  `ConformanceOracle::{package_struct, package_associated_value,
  package_field_types, package_function_params}`, `checker::PackageStruct`)
  are the
  route the driver enters once per compilation, `elaborate_exporting` the
  package build's, which also returns the module constants it resolved;
  `elaborate` composes prepare → elaborate for the CLI's
  `check` and the phase tests), the `Elab` elaboration driver
  (`block`/`stmt`; its
  `keep_template_comptime_if` keeps every `comptime if` of a function body and `keep_template_comptime_for` every loop, `def_body` pushing a binder set for every body, empty for a plain one; a module constant that applies a callable, asks a layout, or reads one is marked `CtMarker::Applied` and kept as written, `applied_initializer`; its
  own binders (`Elab::template_binders`) for the check, where every other
  one is selected), and type resolution; nothing is cloned or specialized
  above the check. `Elab`'s remaining
  methods are split across `impl<'a> Elab<'a>` blocks in the submodules
  below, and the root's helper clusters live in `comptime/{synth,params}.rs`.
- `comptime/elab.rs` holds the root driver's own cluster, and owns
  `Elab::freeze_struct_value_arguments` (a whole-program `MutVisitor` walk
  folding a computed struct-typed value argument, `Tagged[Extent.square(4)]`,
  to its fieldwise construction) and `instantiation_failure_stub` (below).
- `comptime/pack_qualification.rs` owns upstream's spelling rule for a
  variadic struct's own pack (`qualify_struct_packs`, the first step of
  `prepare`): a member naming the pack bare reports
  `ComptimeError::UnqualifiedStructParam`, the header keeps the bare name,
  and the parser's qualified spread `Type::SelfParam("*Ts")` is folded onto
  the bare `Named("*Ts")` node every later fold expands. It walks members
  with `mojito_ast::visit` (`Visitor` + `walk_*`), the crate's read-only AST
  traversal with a scope hook for shadowing binders.
- `comptime/eval.rs` owns compile-time expression evaluation (the `eval`
  dispatcher, set/dictionary displays and the explicit literal constructors,
  the structural collection folds `len`/`in`/`keys`/`values`, reflection
  methods, infix/iteration folding).
- `checker/comptime_requests.rs` owns the line every compile-time request
  is drawn by: `applies_callable`, over a phase's `CalleeOracle` (the
  elaborator's `Elab`, `comptime/elab.rs`, and the `Checker`), so the two
  phases agree on which bindings are requests by construction, and
  `lifts_comptime_value`, which adds a collection display and a
  `Dict[...]`/`Set[...]` constructor call, module or local. The checker
  folds every other closed module constant into `Checker.module_values`
  (`Checker::module_value`, `Checker::module_constant_value`); a local
  binding of an application is read the same way
  (`Checker::local_comptime_application`). The
  elaborator keeps every collection-valued `comptime` binding for the
  check — a display or constructor (`Elab::keep_template_comptime_binding`),
  a closed field-name list (`crossing::lists_field_names`), a method call of
  a value binding (`calls_value_method`) — and spells none of them into a
  read; a binding that still folds to a collection is an error
  (`Elab::stmt`). `comptime/eval.rs` folds closed values only
  and refuses every call (`not_evaluated_here`): no call is evaluated
  above the check.
- `comptime/crossing.rs` owns the compile-time → runtime crossing fold
  (`fold_runtime_crossings`: `materialize[X]()` and `comptime(e)` become
  literals, and so does a reflection query over a closed handle
  (`reflection_query`: `r.field_count()`, `reflect[T].field_index["x"]()`);
  a `comptime(e)` operand that evaluates to a collection is rejected with
  upstream's `ImplicitlyCopyable` text; runtime locals shadow). The check
  owns every other crossing rejection (`Checker::reject_display_crossing`).
- No method or `def` is cloned, per call or per instance: a `comptime
  if`/`comptime for` over a method's own binders or its struct's stays in
  its template (`Elab::def_body`), and a body that still fails over them
  becomes `instantiation_failure_stub`'s body there (`comptime/elab.rs`),
  the compiler-private `_mojito_instantiation_failed("…")` the checker
  types like `_mojito_abort` (`call_inference.rs`, diverging in
  `declarations.rs:is_diverging_intrinsic`), so `native::mono` reports it
  where it is reached (`mono/failure.rs`). Every generic `def` keeps its
  template at every call; a `DType`- or lane-keyed `def`, overloaded or
  not, is an ordinary generic `def`. An associated type its body names is
  solved below the waist, from `MirStructDeclaration.associated_types`, or
  for a parameterized member (`C.IteratorType[o]`) from
  `parameterized_associated_types` (`ParameterizedAssociatedType`,
  `AssociatedParameter` in `mojito-checked/src/declaration.rs`, built by
  `ParameterizedMember::checked_fact`) (`declared_associated_type` over
  `struct_instance`, `native/mono/substitute.rs`; the template's origins
  bind through `mojito_types::types::substitute_origin_params`). `checker/origins/transfer.rs` owns the
  carried source (`SigOrigin::Carried`, recorded by
  `record_transfer_effect` and closed by `replay_transfer_effects` with the
  receiver's arguments and the call's own bindings, `call_closed`).
- `comptime/rewrite.rs` owns alias substitution over the AST
  (`rewrite_stmt_cloned`, `rewrite_expr`), type-binding substitution
  (`substitute_type_bindings_in_{block,type}`), `mark_struct_names`, and
  `pack_element_construction`.
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
  VM (`Prog::hash_leaf_update`) and Pliron (`hash_leaf_update_instance`)
  select it by
  the lane shape of its value parameter.
- `crates/mojito-symbol/src/symbol.rs` owns the value-specialization demangler
  (`demangle_specialization`, which rebuilds every key but a type or
  reflected one, and `unqualified_instance_name`) behind
  `_unqualified_type_name`'s spelling of a minted clone.
- `crates/mojito-symbol/src/symbol.rs` also owns the string identities
  (`is_stdlib_string_struct`/`is_stdlib_string_span_struct`, re-exported from
  `mojito-types`; `nominal_string_literal_ctor_symbol`, the literal→`String`
  wrap MIR emits), the list-literal initializer predicate
  `is_list_literal_constructor` (the checker's display construction and
  mono's `Array` parameter-value construction), its dict counterpart
  `is_dict_literal_constructor` (the checker's
  `record_dict_literal_construction`, which records a `DictLiteral` on the
  display's `ConstructCollection`), and
  `materialized_instantiation_argument`, the spelling an instance's
  argument takes (the native monomorphizer's `enqueue_display_instance` in
  `native/mono/instances.rs` instantiates a printed instance's `write_to`,
  and `dispatched_overload_target` selects the receiver's
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
  types as the struct at its own binders), and `infer_call` binds a
  `def`'s own type parameters from the arguments the checker solved, which
  `SemanticAdjustment::InstantiatedArguments` carries into
  `MirInstr::Call::instantiated_args`, and a method's own from a
  `checked::MethodInstantiation` into `MirInstr::MethodCall::instantiated_args`,
  inferred value parameters included, which take no slot; a construction's
  record, `Checker::record_construction_instantiation`
  (`checker/declarations.rs`), holds the struct's arguments then the
  constructor's own, and `infer_call` binds a construction's from its
  declaration's first parameter); the checker closes a method's own value
  binders in its result (`Checker::close_method_values`) and types a spelled
  `DType` argument (`record_dtype_parameter_arguments`); `split_method_symbol` splits a lowered method
  symbol from its receiver at the last `.` outside brackets (an instance's
  baked `SIMD[DType.float32, 2]` keeps its `.`; the MIR verifier and the
  monomorphizer parse method symbols through it), and
  `lifecycle_constructor` recognizes a
  construction through an overload.
- A closed bracket argument is compile-time data on its call: the checker's
  `folded_parameter_arguments` (`checker/call_inference.rs`, over
  `bracket_argument_slots`, the one bracket-to-declaration binding) fills
  `GenericInstantiation::folded_arguments` (a construction's and
  `external_call`'s included) and
  `MethodInstantiation::folded_arguments` with the positions, among the
  call's bracket arguments as written, of arguments solved
  to `CtValue::is_parameter_attribute` values (a string, or a closed
  scalar, vector, or aggregate), carried to MIR as
  `SemanticAdjustment::FoldedParameterArguments`; `Flatten::param_arg_regs`
  (`Flatten::param_arg_has_no_runtime_form`, `mir.rs`) lowers no code for
  them; `verify_param_arguments` (`verify/subscripts.rs`) accepts the absent
  register against the call's `instantiated_args`.

## Change Routing

| If you change… | Start at… | Also inspect… |
|---|---|---|
| Syntax or AST shape | [`grammar.md`](grammar.md), `parser.rs`, `ast.rs` | Parser tests, `frontend.md`, feature matrix. |
| Argument binding | `call.rs` | Checker/VM adapters and call-parity tests. |
| Which declarations use a parameter as a lane width | `mojito-ast/simd_width.rs` | `comptime/synth.rs` (`desugar_simd_wildcard_parameters`), `checker/comptime_validation.rs`, and `native::mono`'s lane closing (`mono/gather.rs`). |
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
  displaces on the display's span and `display_adjustments`,
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
  `ComptimeSource::Bound` (`record_bound_iteration`), a sequence a
  compile-time application builds records `ComptimeSource::Applied`
  (`record_applied_iteration`), which MIR lifts as `Evaluated`'s is, a
  name bound to one records its application as the sequence
  (`requested_name`) and has no runtime form
  (`lower_comptime_binding`, over the statement's
  `CheckedStmtOp::Comptime::denotes`; a runtime read of a collection is
  rejected by `reject_display_crossing`), a `range` bound the
  check does not close records `ComptimeSource::EvaluatedRange`
  (`record_comptime_iteration`, admitted by `reads_compile_time_alone` over
  `comptime_binding_owners`), a condition naming an evaluated binding is
  left uncompiled (`names_evaluated_binding`), and a runtime read is
  rejected (`reject_display_crossing`, from `inference.rs`). An iterable
  `comptime_iteration_element` does not serve (it gives `None`) is
  iterated by upstream's iterator protocol:
  `Checker::check_comptime_iterator_for` (`checker/comptime_iteration.rs`)
  types the spelled `iterable.__iter__()` call and the three stub calls
  (`comptime_iterator_protocol`, the stubs in a `Checker::probe`, named by
  `paramfor_stub` over `std.builtin._stubs`, the bundled
  `stdlib/std/builtin/_stubs.mojo` the prelude links for its bodies),
  records the call as the iterable's `SpelledConstruction` and
  `ComptimeSource::Iterator` in the iterable's `comptime_iteration` fact
  (`CheckedArena::spelled_construction` pushes it onto the call's node),
  and binds the variable as a local alias of the `paramfor_next_value`
  application. `lower_term` makes the header's
  `ComptimeSequence::Iterator`, its `initial` the thunk of the spelled
  call (`ComptimeThunks::request_value`), and `Specializer::trip_elements`
  (`native/mono/unroll.rs`) steps it, binding each copy to its iterator.
  `Flatten::lower_comptime_binding` (`mir/lower_stmt.rs`) records the
  binding with its value's node handle and sequence (`DisplayBinding`, in
  the scoped `Flatten::displays`; `ComptimeThunks::bind_display`) and gives
  it no runtime form, lifting the
  display under the name and over the binders the check's sequence spells;
  `lower_term` reads a header's sequence back off the binding
  `ComptimeSource::Bound` names (`Flatten::display`), and lifts an evaluated bound
  (`ComptimeThunks::request_value`). A bracket argument that reads a
  display is lowered by `Flatten::param_arg_reg` through `display_read`, or
  for the capitalized spelling as the element its attribute holds
  (`attribute_value_reg`), and an inferred
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
  `check_program` and `ConformanceOracle::{from_program, importing}`) collects every
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
  `ComptimeThunks::request_applications` (`mir.rs`, from `lower_fn_nested`,
  the nested-`def` lowering, and the top level once the body is lowered,
  over the body's statement ids `Cfg::body`, and from a module constant's
  `LoweringUnit::Constant::root`) walks the body's checked nodes
  (`CheckedTables::walk`, skipping a nested `def`'s, struct's, or trait's
  bodies) and lifts each named function over the narrowest recorded scope (`enter_scope`) that declares
  its binders, `ComptimeThunks::lower` skipping a name another owner
  already lifted, and `Flatten::comptime_application` reads the expression
  back as its application in `display_read` and `value_binder_node`.
  `Flatten::display_read` (`mir.rs`)
  lowers an expression the check lifted as the parameter constant of its
  application, and a compile-time expression that reads a display
  (`displays_read`): an `Int` or `Bool` as a parameter constant over its
  thunk's application, any other value in place after `build_display`. A
  local `comptime` binding or a `comptime(e)` operand that applies a
  callable at a type no parameter expression spells (a `String`, a tuple, a
  struct) is lifted by name at its own type
  (`Checker::requested_binding`, `checker/comptime_validation.rs`), in any
  body, and each read of the binding materializes that application (a
  `ParamValue`, `Checker::local_comptime_application`) where no slot holds it, a projection of a temporary it builds copied out as the thunk's
  `return` operand (`Checker::copy_compile_time_result`);
  `Flatten::crossing_operand` (`mir/lower_expr/expr_access.rs`) is the
  operand of a kept `materialize[X]()` or `comptime(e)`
  (`Checker::infer_template_materialize`, `infer_template_comptime`, which
  rejects an operand that is not `ImplicitlyCopyable` as a crossing), a
  whole display through `crossing_display`. Every thunk begins with the
  local `comptime` bindings its expression reads that denote no parameter
  expression (`ComptimeThunks::bind_evaluated`, `thunk_prologue`,
  `bindings_read` over `CheckedTables::walk_expression` from the request's
  node, an `EvaluatedBinding` naming its statement node; lowered by `lower_expression_thunk`, which
  hands HIR the request's node as the returned value, under
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
  `comptime` value (`comptime k = n + 1`, or an application the statement
  op `denotes`) as its parameter expression:
  `lower_comptime_binding` records it (`ComptimeThunks::bind_value`), each
  request carries the bindings so far (`EnclosingBinders::comptime_bindings`),
  and `Flatten::identifier_read` (`mir/lower_expr/expr.rs`) answers the read.
  The same holds for such a binding as a method receiver
  (`lane.is_floating_point()` over `comptime lane = dt`):
  `Flatten::lower_call_receiver` (`mir/calls.rs`) reads it as a value, not
  a place, and `type_receiver_name` takes a receiver for a type only when
  its node is a name the check bound to nothing and typed as no value. The header's `source` is the
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
  (`mir/lower_expr/expr.rs`) lowers as its operand, a whole field-name list
  (`materialize[names]()`) being the `Array` `materialized_reflection_list`
  (`checker/reflection.rs`) types and records as a parameter value that
  mono constructs per instance; `check_ct_bool` compiles
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
  result `VmBackend::freeze` (`freeze_value`, `backend/vm.rs`) freezes as
  nominal fields and `CtValue::Pointer` memory. `CtValue::comptime_iteration_elements`
  reads the frozen `Array`, `List`, `Set`, or `Dict` storage; a nominal
  `Tuple` (`is_nominal_tuple`) freezes to its elements — and
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
  runtime promotion (`mono/promote.rs`) and unrolling. A type parameter
  has no frame slot (`callable_parameter_locals`, `mir.rs`), as Mojo gives
  it no storage; a non-callable value binder's slot, interned at its first
  read by `Flatten::var`, is typed by the binder's declared type. The comptime
  elaborator keeps a `comptime for`'s index symbolic
  (`Elab::keep_template_comptime_for`, `comptime/elab.rs`), so a struct
  applied over it (`Lanes[i]`) reaches the check as a parameter expression.
- Every `comptime for` of every function body is kept to MIR
  (`Elab::keep_template_comptime_for`, `comptime/elab.rs`); no top-level
  `def` is keyed on one and nothing is unrolled above the check. The
  crossing pass leaves a named collection in a kept header or condition
  for the check, which reads it as its application, and leaves a
  `comptime(e)` it cannot evaluate for the check (`in_template_body`, true
  in every function body).
- Every type pack is template-served: a whole-pack spread is
  `MirInstr::Call::spread` or `MirInstr::MethodCall::spread`
  (`lower_pack_spread_argument`, `mir/calls.rs`; `verify_pack_spread`,
  `verify/calls.rs`; `refuse_pack_spread`, `backend/vm/calls.rs`, since
  concrete MIR carries none) and `native::mono::spread` (`mono/spread.rs`:
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
  pack's length.

## Struct generators (P3d, 2026-10-05)

- No struct is cloned above the check: every struct — keyed on a
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
  subscript call (whose compile-time operand register it reads by bracket
  position, `CheckedCallParameterArgument::operand`), `mono/substitute.rs:substitute_subscript_call` folds it in
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
  specializer's rewrite), each typed by the read's register;
  `CtValue::is_materialized_constant` (`mojito-types/src/ct.rs`, called
  from `verify/instr.rs`) admits it, a bare string only at the nominal
  `String` (`types::is_nominal_string`). A value with a `String` leaf
  (`CtValue::is_constructed_parameter_value`) is the same constant, its
  slot holding nothing: `reject_parameter_storage_reads`
  (`mono/substitute.rs`) rejects a place of it that survives the
  substitution, and `seed_captured_parameter_slots` stores a captured
  folded slot's constant at entry, never one owning a buffer. Only a list
  read at an `Array` is constructed at each read
  (`construct_parameter_reads`, through `array_display_construction`). The VM materializes a folded one through
  `ct_value_as_runtime` (`backend/vm.rs`) and, a tuple at any depth as the nominal `Tuple` its
  checked type names, `thaw`
  (`backend/vm/adapters.rs`), and Pliron through `lower_parameter_value`
  (a top-level string included) and `store_parameter_value`
  (`lower/consts.rs`). A `comptime for` element is the same constant at
  each read of its binder, and an element read at an `Array` is
  constructed (`construct_parameter_reads`, from `Specializer::copy_body`,
  `mono/unroll.rs`). A
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
  and an alias substitution folds one off a substituted binder
  (`substituted_field`, `comptime/rewrite.rs`). A compile-time parameter's
  slot is no
  drop root: a `comptime for` binder's (`comptime_binder_slots`,
  `analysis/scan.rs`) and a value parameter's
  (`materialized_parameter_slots`, `mojito-mir/src/mir.rs`, over the
  declarations `binder_scope` walks, which `Specializer::binder_scope`
  shares), both excluded by `elaborate_drops` and
  `RegionDropCtx::droppable`.
- A type's compile-time value member (`T.K`, `Self.T.K`, `A.K`, `B[4].K`,
  `Self.K`) is `Checker::type_member_value` (`checker/constraints.rs`, over
  `member_subject` and `struct_member_at`): a `ParamKind::TypeMember`
  node (`ParamContext::type_member`, text `param_type_member`) through a
  type parameter, typed by the bound's requirement, and the struct's member
  at the application's arguments on a struct; a member no bound requires is
  `TypeError::NoTraitAttribute`. `infer_member` (`indexing.rs`) records a
  runtime read through `read_type_member` as `SemanticAdjustment::ParamValue`,
  and `eval_associated_ct` reads one in a compile-time position. Each
  struct's value members cross the waist as
  `ConformanceFacts::associated_values` → `MirStructDeclaration::associated_values`
  (`Checker::conformance_facts`, `traits.rs`), and `native::mono` answers
  the node per instance (`type_member_answer`, `mono/symbolic.rs`, over
  `StructShape::values`).
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
  `Elab::freeze_struct_value_arguments` (`comptime/elab.rs`) rewrites any
  other struct-typed argument of a struct or a uniquely named `def` to that
  construction, evaluated by CTFE.
- `bind_ty_args` (`mono/unify.rs`) binds a struct's pack element by element,
  whole (`CtValue::Tuple`), or forwarded (`TyKind::RuntimePack`) through
  `bind_pack`.

## The parameter constant (2026-10-04)

- `Const::Param(ParamExpr)` (`mir/ir.rs`) is a parameter expression read as a
  runtime value, the text form `param(...)` (schema 1.20). The checker
  records it as `SemanticAdjustment::ParamValue { value }`
  (`mojito-checked`, `checked.rs`) through `Checker::record_pack_query_value`
  and `typelist_proposition_query` (`checker/comptime_validation.rs`), from
  `infer_member` (`Ts.length`, `indexing.rs`), `infer_len` (`len(Ts)`,
  `builtins.rs`), and the `Invoke` arm of `infer` (`Ts.contains[X]()`,
  `Ts.all_conforms_to[T]()`, `inference.rs`). A value pack that is still a
  parameter (`*values: Int`) is a `ParamList` of its element
  (`annotations::value_parameter_expr`, `verify::declared_kind`):
  `Checker::value_pack_named` resolves it — a struct's own `Self.values`
  through `self_value_pack`, which `infer_member` types as the pack. Only a
  parameter position reads the pack itself: `compile_dependent_ct_expr`'s
  `Index` arm makes `values[i]` a `ListGet` (upstream's
  `__getitem_param__`, roadmap R800), and `comptime_iteration_element`
  types a `comptime for` over it. Its length is the bundled
  `ParameterList.size` member, read through the value (`values.size`) by
  the instance arm of `infer_member` (`indexing.rs`, any struct's value
  member) or, at run time, by `ParameterList.__len__`; a parameter-position
  `len(values)` is a lifted application of that call. Inside the bundled
  struct alone `len(Self.values)` is the raw list's length
  (`Checker::reads_raw_parameter_list`, `eval_associated_ct`), the stand-in
  for upstream's `#kgen.param_list.size`. Any other read
  (`for v in values`, any runtime index, a whole binding) is the bundled
  `ParameterList[*values]()` (`stdlib/std/builtin/variadics.mojo`, with
  `_ParameterListIter`): `Checker::infer_value_pack_read` (top of
  `infer_impl`) spells that construction as a `SpelledConstruction` whose
  identity is derived at `ast::VALUE_PACK_READ_ORDINAL`, the checked arena
  and HIR (the read's operand is that construction's node) lower it in the
  read's place,
  and `origin_place` / `materialize_borrow_owner` treat the read as that
  temporary. `Checker::spells_parameter_list` and
  `admit_parameter_list_brackets` admit `ParameterList`'s brackets only
  there and in its bundled module, since upstream's list is one parameter.
  The bundled struct conforms to `Writable` where its element type does
  (`_write_elements`, `write_to`, `write_repr_to`, upstream's members).
  `ParameterList.get_span` takes the elements' static address from the
  module-private `__param_list_address[*values]()`
  (`infer_param_list_address`, `call_inference.rs`;
  `SemanticAdjustment::ParamListAddress`), which MIR lowers to
  `MirInstr::ParamListAddress` (schema 1.34), mono closes in
  `answer_param_constants`, the VM allocates once per list
  (`VmBackend::static_param_list`), and the native backend interns as a
  private constant global (`ModuleShared::intern_param_list`,
  `lower_param_list_address`). A `ref[ImmStaticOrigin]` result the VM
  reaches through such a temporary span is forwarded as its heap pointer
  (`VmBackend::static_reference_result`). A pack spread
  whole into brackets (`Pack[*vs]`, `total[*vs]()`) binds the callee's pack
  to it (`Checker::value_pack_spread`, read by the bracket binding in
  `checker/declarations.rs`); MIR
  passes it as the list's parameter constant in a `VariadicPack` register
  (`Flatten::attribute_value_reg`, from the argument's recorded attribute),
  `verify_param_arguments` accepts it in the pack's slot, mono folds it and
  retires it once the call names its instance (`retire_forwarded_packs`,
  `mono/spread.rs`); and mono binds the pack from the call
  (`bind_instantiated_arguments`, `mono/infer.rs`;
  `bind_explicit_value_arguments`, `mono/unify.rs`) and folds the constants
  in each unrolled copy too (`copy_body`, `mono/unroll.rs`).
  `Flatten::param_value` and
  `param_value_register` (`mir/lower_expr/expr_access.rs`) lower it; the
  pack operand is never lowered. `verify/scope.rs` checks its binders and
  `verify/concrete.rs` rejects a survivor.
- `Specializer::answer_param_constants` and `param_constant`
  (`mono/specializer.rs`) fold it per instance to the constant typed by
  its register, a list read at an `Array` constructed instead
  (`constructed_parameter_constant`, `array_display_construction`,
  `mono/substitute.rs`, through the list-literal initializer
  `ConstructionDecls::list_literal_constructor` finds, the call MIR lowers a
  display to): a length through `eval_ct`,
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
  `ParamContext::evaluate` answers them too;
  `ParamContext::replace` does not, since an application stays symbolic in
  a type. The checker's producer is
  `record_reflection_value` (`checker/reflection.rs`).

## The compile-time branch and the request path (2026-10-03)

- `MirTerm::ComptimeBranch { cond: GenericConstraint, .. }` (`mir/ir.rs`) is
  the `comptime if`: HIR's `Terminator::ComptimeBranch`, lowered by
  `Flatten::comptime_condition` (`mir/lower_expr/entry.rs`) from the
  checker's `SemanticAdjustment::ComptimeCondition` (recorded by
  `Checker::check_comptime_condition`, `checker/comptime_validation.rs`, per
  leaf under `not`/`and`/`or`) or, for a leaf with no constraint, from the
  thunk `ComptimeThunks::request` registers with the binders in scope at
  the condition and `lower_expression_thunk` lowers over them (`mir.rs`,
  shared with `lower_default`, which reads a default's node off its
  declaration's op through the unit's `root` (`CheckedFnParam::default`); a nested
  `def`'s through `NestedNode::defaults`); the thunk reads an enclosing loop's index
  as a parameter reference (`Flatten::parameter_reads`, filled from
  `EnclosingBinders::loop_bindings`, read in `Flatten::identifier_read`). `verify/scope.rs`
  (`ScopeCx::constraint`) checks its binders; `verify/concrete.rs` rejects a
  survivor; the text form is `comptime_branch` (schema 1.15).
- `mojito_vm::crossing` owns `ct_to_vm`/`vm_to_ct` and `CTFE_FUEL`;
  `VmBackend::{call_concrete, freeze}` run a verified fragment for the
  elaborator. `call_concrete` accepts compile-time arguments; typed `thaw`
  (`backend/vm/adapters.rs`) materializes them and `Const::Value` into fresh
  heap storage. `freeze_value` rejects dead and cyclic pointer allocations.
  Pliron's `store_parameter_value` (`lower/consts.rs`) writes frozen pointer
  memory using checked element layouts and reconstructs nested strings.
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
  `FunctionFrame` around a nested materialization. No `def` is keyed on
  its compile-time control flow; MIR types a value parameter's slot at
  its declared type, which native `mono::substitute_function` substitutes
  under the application's bindings; `CtMarker::Applied` keeps
  an applied constant, a layout constant among them, symbolic through elaboration.
