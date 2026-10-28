//! Static semantic checker: the authoritative handoff between elaborated AST
//! and compiler lowering.
//!
//! It resolves annotations, calls, traits, and conventions into
//! [`CheckedProgram`](mojito_checked::checked::CheckedProgram). It is a
//! *sound* approximation: if [`check`] succeeds, compiled execution will not
//! raise `UndefinedVariable`, `TypeError`, `NotCallable`, `ArityMismatch`, or
//! `ClosureEscape`. It is deliberately not *complete* — see the
//! forward-reference note below — so a few valid Mojo programs are rejected.
//!
//! ## Scoping
//! A stack of scopes (`Vec<HashMap<String, Ty>>`) models lexical name lookup.
//! Names are bound *sequentially* in source order, and a nested `def` body is checked at its definition
//! site with the enclosing scopes still on the stack (so capture is lexical).
//! One consequence: a function body may not forward-reference a sibling `def`
//! declared later in the same block (mutual recursion). Choosing soundness over completeness here keeps
//! the checker simple; hoisting `def` signatures per block is future work.

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use annotations::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use builtins::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use calls::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use comptime_validation::*;
use constraints::ConstraintEnvironment;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use declarations::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use generics::*;
use mojito_checked::fact_store::{FactMap, FactSet, FactVec};
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy, TransferSet, VariantIndex};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use origins::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use overload_support::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use places::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rebind::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use traits_support::*;

use mojito_common::timing;
use mojito_types::conformance::COMPILER_TRAITS;

use bound_defaults::BoundDefaultArguments;
use mojito_ast::ast::{
    ArgConvention, Dtype, Expr, ExprKind, FnParam, InfixOp, Method, PrefixOp, Stmt, StmtKind,
    StructComptime, SubscriptArg, TStringPart, TraitComptime, Type as SourceType,
};
use mojito_ast::call::{
    ArgSlot, CallVariadics, effective_keyword_only_index, match_call_slots, regular_marker_index,
};
use mojito_common::error::TypeError;
use mojito_common::token::{SourceSpan, SyntaxId};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{
    ConstraintVerdict, HoleKind, MetaTy, ParamContext, ParamError, ParamExpr, ParamId, ParamKind,
    ParamOp, ParamRef, UNBOUND_BINDER_PREFIX,
};
use mojito_types::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, ParamDecl, SimdDtype,
    SimdWidth, Ty, TyArg, TySubst, array_element, array_parts, array_type, canonical_simd_ty,
    contains_infer, dict_elements, dict_type, is_scalar_simd, list_element, list_type, range_type,
    scalar_simd_dtype, set_element, set_type, simd_lane, simd_slots, tuple_elements,
    tuple_type as nominal_tuple_type,
};

/// Type-check a whole program. Convenience wrapper over [`Checker`].
pub fn check(stmts: &[Stmt]) -> Result<(), TypeError> {
    check_program(stmts).map(|_| ())
}

/// Type-check and retain the semantic facts consumed by lowering/backends.
pub fn check_program(stmts: &[Stmt]) -> Result<mojito_checked::checked::CheckedProgram, TypeError> {
    check_program_in(stmts, &mut CheckContext::default())
}

/// [`check_program`] in a compilation's [`CheckContext`].
pub fn check_program_in(
    stmts: &[Stmt],
    context: &mut CheckContext,
) -> Result<mojito_checked::checked::CheckedProgram, TypeError> {
    let discovery = check_program_carrying(stmts, context).map(PassCarry::into_result)?;
    let _arena = timing::span("arena");
    timing::count("arena_builds", 1);
    Ok(discovery.finalize())
}

/// The per-compilation state one check runs in.
///
/// It lives for one `Compiler::compile_linked`: the driver lends it to the
/// check and reads it back; every other entry point checks in a default one.
#[derive(Debug)]
pub struct CheckContext {
    /// The compilation's parameter-expression context, shared by every
    /// checker pass. A defaulted context holds a detached one.
    param_context: ParamContext,
    /// Carry an unchanged body's raw facts from one checker pass to the
    /// next instead of inferring it again (`checker/body_carry.rs`).
    body_fact_reuse: bool,
    /// Per call through a bound, by the syntax identity the call keeps across
    /// passes, the requirement defaults it binds (`bound_defaults.rs`).
    bound_default_arguments: HashMap<SyntaxId, BoundDefaultArguments>,
    /// The declaration interfaces of the package modules the program's link
    /// reached, attached to every pass in place of their statements.
    installed: Option<InstalledInterfaces>,
    /// Whether the check exports each library module's declaration
    /// interface, as a package build's does.
    record_interfaces: bool,
    /// The interfaces the last check exported, encoded with their
    /// directory.
    interfaces: Option<Result<InterfaceExport, String>>,
    /// The module constants the program's comptime elaboration resolved,
    /// which an exported interface carries.
    elaborated: ElaboratedConstants,
}

impl CheckContext {
    pub fn new() -> Self {
        Self {
            param_context: ParamContext::new(),
            body_fact_reuse: true,
            bound_default_arguments: HashMap::new(),
            installed: None,
            record_interfaces: false,
            interfaces: None,
            elaborated: Vec::new(),
        }
    }

    /// Check against the declarations `installed` holds, which the program
    /// given to the check does not declare again.
    pub fn install(&mut self, installed: InstalledInterfaces) {
        self.installed = Some(installed);
    }

    /// Export each library module's declaration interface when the check
    /// ends ([`Self::take_interfaces`]).
    pub const fn record_interfaces(&mut self) {
        self.record_interfaces = true;
    }

    /// Export `elaborated`, the module constants the program's comptime
    /// elaboration resolved, each in its declaring module's interface.
    pub fn record_elaborated(&mut self, elaborated: ElaboratedConstants) {
        self.elaborated = elaborated;
    }

    /// The library interface the last check exported, which the package
    /// build makes the declaration records of once it has lowered the
    /// library ([`InterfaceExport::records`]).
    ///
    /// # Errors
    ///
    /// What the check could not export, or that it exported nothing.
    pub fn take_interfaces(&mut self) -> Result<InterfaceExport, String> {
        self.interfaces
            .take()
            .unwrap_or_else(|| Err("the check exported no interface".to_string()))
    }

    pub const fn param_context(&self) -> &ParamContext {
        &self.param_context
    }

    /// Whether a body whose inputs are unchanged since the previous checker
    /// pass takes its facts from that pass.
    pub const fn body_fact_reuse(&self) -> bool {
        self.body_fact_reuse
    }

    pub const fn set_body_fact_reuse(&mut self, reuse: bool) {
        self.body_fact_reuse = reuse;
    }

    const fn bound_default_arguments(&self) -> &HashMap<SyntaxId, BoundDefaultArguments> {
        &self.bound_default_arguments
    }

    /// Keep the requirement defaults a pass found calls through a bound
    /// leaving out; whether any call was new.
    fn record_bound_default_arguments(
        &mut self,
        found: HashMap<SyntaxId, BoundDefaultArguments>,
    ) -> bool {
        let before = self.bound_default_arguments.len();
        for (call, arguments) in found {
            self.bound_default_arguments
                .entry(call)
                .or_insert(arguments);
        }
        self.bound_default_arguments.len() != before
    }
}

impl Default for CheckContext {
    fn default() -> Self {
        Self::new()
    }
}

/// [`check_program_in`]'s passes, returning the last pass's
/// carry.
///
/// Each transfer pass after the first starts from the previous pass's
/// committed effect maps and serves every body site whose effect reads are
/// still current from that pass instead of inferring it (`body_carry`).
fn check_program_carrying(
    stmts: &[Stmt],
    context: &mut CheckContext,
) -> Result<PassCarry, TypeError> {
    // Two-phase transfer effects: a call site checked before its callee's
    // body only sees effects already committed, so the check reruns — seeded
    // with the prior round's committed map — whenever some call site
    // observed a stale (since-grown) callee entry. Effects grow
    // monotonically over a finite lattice, so the fixpoint is small; the cap
    // guards checker defects, not user programs.
    const TRANSFER_EFFECT_ROUNDS: usize = 4;

    fn first_stale<E: PartialEq + Clone>(
        committed: &HashMap<String, Vec<E>>,
        observations: &HashMap<String, Vec<E>>,
    ) -> Option<String> {
        observations
            .iter()
            .find(|(name, seen)| {
                let now = committed.get(*name).cloned().unwrap_or_default();
                now.len() != seen.len() || now.iter().any(|effect| !seen.contains(effect))
            })
            .map(|(name, _)| name.clone())
    }

    let mut expanded = stmts.to_vec();
    // Source locations survive elaboration clones and therefore cannot identify
    // semantic occurrences. Re-key the final checked tree after the last
    // checker-side cloning transform, before any fact table is populated.
    let syntax_origins = {
        let _rekey = timing::span("syntax_rekey");
        mojito_ast::ast::rekey_syntax(&mut expanded)
    };
    bound_defaults::bind_bound_default_arguments(
        &mut expanded,
        &syntax_origins,
        context.bound_default_arguments(),
    );
    let rebind_targets = erase_rebinds(&mut expanded);
    // The previous pass: none for the first, then each stale pass for the
    // next. Its committed maps seed the pass and its records serve the
    // unchanged bodies.
    let mut previous: Option<PassCarry> = None;
    let mut rounds = 0;
    let checker = loop {
        let _round = timing::round("transfer.round", rounds);
        let (transfer_seed, call_through_seed) =
            previous.as_ref().map(PassCarry::seeds).unwrap_or_default();
        let mut checker = Checker::new_seeded(transfer_seed, call_through_seed);
        if let Some(installed) = &context.installed {
            checker.attach_interfaces(installed)?;
        }
        if let Some(previous) = &previous {
            checker.next_owner.set(previous.next_owner());
            checker.fresh_owner_cursor.set(previous.next_owner());
        }
        checker.previous = previous.take();
        checker.rebind_targets.clone_from(&rebind_targets);
        checker.syntax_origins.clone_from(&syntax_origins);
        checker.param_context = context.param_context().clone();
        checker.context.replace(std::mem::take(context));
        let body_check = {
            let _check = timing::span("check_program");
            checker.check_program(&expanded)
        };
        *context = checker.context.take();
        // A call through a bound leaving out a requirement default binds it
        // (`bound_defaults.rs`); a pass that found a new one checks again
        // with it spelled, whatever its verdict without it.
        if context.record_bound_default_arguments(checker.bound_default_arguments.take())
            && bound_defaults::bind_bound_default_arguments(
                &mut expanded,
                &syntax_origins,
                context.bound_default_arguments(),
            )
        {
            continue;
        }
        body_check?;
        // The previous pass has served every site it could.
        checker.previous = None;
        {
            let _reads = timing::span("reference_reads");
            checker.check_reference_result_reads()?;
        }
        let stale = first_stale(
            &checker.transfer_effects.borrow(),
            &checker.effect_observations.borrow(),
        )
        .or_else(|| {
            first_stale(
                &checker.call_through_effects.borrow(),
                &checker.call_through_observations.borrow(),
            )
        });
        let Some(callable) = stale else {
            break checker;
        };
        rounds += 1;
        if rounds > TRANSFER_EFFECT_ROUNDS {
            return Err(TypeError::TransferEffectDivergence {
                rounds: TRANSFER_EFFECT_ROUNDS,
                callable,
            });
        }
        previous = Some(checker.into_carry(
            Vec::new(),
            HashMap::new(),
            mojito_checked::checked::ConformanceFacts::default(),
        ));
    };
    // A package build exports its interfaces; an importer's check exports
    // nothing.
    if context.record_interfaces {
        let _export = timing::span("export_interfaces");
        context.interfaces = Some(checker.export_interfaces(&expanded, &context.elaborated));
    }
    // Context managers and sequence destructuring are desugared by the
    // checker; later phases see only the ordinary statements it checked.
    desugar::splice_desugars(&mut expanded, &checker.statement_desugars.borrow());
    let explicit_destroy_types = {
        let _finish = timing::span("explicit_destroy");
        let explicit_destroy_types = explicit_destroy_types(&checker);
        run_explicit_destroy(&checker, &expanded, &explicit_destroy_types)?;
        explicit_destroy_types
    };
    {
        let _conventions = timing::span("result_conventions");
        checker.record_result_conventions();
    }
    let conformances = {
        let _conformances = timing::span("conformances");
        checker.conformance_facts()
    };
    let _carry = timing::span("carry");
    Ok(checker.into_carry(expanded, explicit_destroy_types, conformances))
}

mod body_carry;
mod declaration_table;
pub(crate) use declaration_table::DeclarationTable;
mod interface;
mod node_facts;
pub use interface::{
    DeclarationInterfaces, ElaboratedConstant, ElaboratedConstants, InstalledInterfaces,
    InterfaceExport,
};
mod overload_support;
mod probe;
pub use body_carry::PassCarry;
pub use overload_support::is_bundled_module_source;
mod trait_defaults;
mod traits_support;
pub use trait_defaults::trait_member_declaration;

/// Type-check a program and return the concrete lowered callee chosen for
/// every overloaded call site.
///
/// MIR lowering uses this side table so source calls like `f(x)` can lower to
/// a signature-specific function even when overloads share the same arity.
pub fn resolve_overload_targets(stmts: &[Stmt]) -> Result<HashMap<SourceSpan, String>, TypeError> {
    Ok(check_program(stmts)?.overload_targets().clone())
}

/// The bundled `std/builtin/_enum.mojo`, whose source an `enum`'s
/// generated storage writes carry: they mark the result initialized and
/// write its storage, as the module implementing enums does upstream.
pub fn enum_support_source() -> Option<String> {
    mojito_module::module::bundled_root().map(|root| {
        root.join("stdlib/std/builtin/_enum.mojo")
            .to_string_lossy()
            .into_owned()
    })
}

/// The linked name of the `std.builtin._stubs` function `name`, which no
/// user spelling reaches: the compiler calls these by module path, as
/// upstream's `getBuiltinFunction` names them (`list_length_matches` for a
/// sequence pattern, `paramfor_*` for a `comptime for`).
pub(crate) fn builtin_stub(name: &str) -> String {
    mojito_module::module::linked_item_name(STUBS_MODULE, name)
}

/// The module holding the functions the compiler calls by module path.
pub(crate) const STUBS_MODULE: &str = "std.builtin._stubs";

/// The linked name of the bundled `EnumLike` trait, which pattern matching
/// reads a subject's cases through: the compiler names it by module path,
/// as upstream's `getBuiltinTrait` does, never by its spelling.
pub(crate) fn enum_like_trait() -> String {
    mojito_module::module::linked_item_name("std.builtin.enum_like", "EnumLike")
}

/// The linked name of the bundled `std.math` trait `name` (`DivModable`),
/// which the prelude does not export: the compiler names it by module path,
/// never by its spelling.
pub(crate) fn math_trait(name: &str) -> String {
    mojito_module::module::linked_item_name("std.math", name)
}

/// The linked name of the bundled `Hasher` trait, which the prelude does
/// not export: the compiler names it by module path, never by its spelling.
pub fn hasher_trait() -> String {
    mojito_module::module::linked_item_name("std.hash.hasher", "Hasher")
}

/// The linked name of the bundled `std.format` helper a synthesized
/// `Writable` default writes each field through, which no user spelling
/// reaches: the compiler names it by module path.
pub fn writable_field_writer() -> String {
    mojito_module::module::linked_item_name("std.format", "_reflection_write_field_to")
}

/// A static type checker over the parsed AST.
///
/// Top-level struct and trait declarations register order-independently
/// (shells, member types, method signatures) before the source-order walk
/// checks conformance and bodies; everything else checks in a single
/// source-order pass.
#[allow(
    clippy::struct_excessive_bools,
    reason = "TODO: group the flags into a state enum"
)]
pub struct Checker {
    /// Lexical scope chain, innermost last. Starts with the global scope.
    scopes: Vec<HashMap<String, Ty>>,
    /// Binding mutability, parallel to `scopes`. `var` locals are writable;
    /// ordinary function parameters are not.
    mutable_scopes: Vec<HashMap<String, bool>>,
    /// Stable identities for value bindings, parallel to `scopes`. Origin and
    /// loan facts use these identities so a shadowing declaration cannot be
    /// confused with the binding of the same source name in an outer scope.
    owner_scopes: Vec<HashMap<String, mojito_types::origin::OwnerId>>,
    /// Origins retained inside reference-bearing aggregate bindings, parallel
    /// to the lexical value scopes.  Unlike `TyKind::Struct`, this preserves the
    /// use-site owner identity needed for escape checking.
    aggregate_origin_scopes: Vec<HashMap<String, Vec<mojito_types::origin::Origin>>>,
    /// Field-specific projection of `aggregate_origin_scopes`. Keeping direct
    /// reference fields separate prevents a write through one stored handle
    /// from invalidating interiors reached through an unrelated field.
    /// The explicit origin demands of an annotated local (`var v: Span[Int,
    /// origin_of(xs)]`), parallel to the lexical value scopes, so a later
    /// reassignment is judged against the same demand as the declaration.
    storage_origin_demand_scopes: Vec<HashMap<String, Vec<(String, mojito_types::origin::Origin)>>>,
    /// The origin binders a binding's construction bound immutably — through
    /// an `ImmOrigin(o)` cast (`Cell[ImmOrigin(origin_of(xs))](...)`), or by
    /// a callee's read parameter or receiver in a view it returns — parallel
    /// to the lexical value scopes, each with the field path below the
    /// binding at which the cast construction sits (`["cell"]` for
    /// `Wrap(Cell[ImmOrigin(…)](…))`, empty for the binding's own): the cast
    /// drops the source's mutable capability, which the construction-time
    /// origins alone cannot show.
    immutable_origin_binder_scopes: Vec<HashMap<String, Vec<ImmutableOriginBinder>>>,
    aggregate_field_origin_scopes:
        Vec<HashMap<String, HashMap<String, Vec<mojito_types::origin::Origin>>>>,
    /// Reference-parameter handle types. Parameter expression typing still
    /// reads through to the declared referent, while storage contexts can ask
    /// for the handle explicitly.
    reference_parameter_scopes: Vec<HashMap<String, mojito_types::origin::RefTy>>,
    /// The enclosing struct's origin binder a `ref[Self.o]` parameter's clause
    /// names, by the parameter's owner: `Pointer(to=param)` mints that binder
    /// (upstream's iterator-storage shape `self.src = Pointer(to=xs)` into a
    /// `Pointer[T, Self.o]` field) instead of the parameter slot's place.
    reference_parameter_binders:
        HashMap<mojito_types::origin::OwnerId, mojito_types::origin::PointerOrigin>,
    /// Origin-parameter declarations for callable values, parallel to the
    /// lexical value scopes. The outer vector stored per name has one entry per
    /// overload declaration. Each entry also retains the original compile-time
    /// parameter order so erased Origin arguments can participate in overload
    /// and generic candidate selection.
    callable_origin_scopes: Vec<HashMap<String, Vec<CallableOriginSignature>>>,
    /// Monotonic binding-identity source. A `Cell` because materialized
    /// borrow-source temporaries mint anonymous owners from shared-borrow
    /// inference contexts.
    next_owner: std::cell::Cell<u32>,
    /// The first identity above every range a previous pass recorded, where
    /// a body without a range of its own, or one that outgrew its range,
    /// allocates (`reserve_owners`).
    fresh_owner_cursor: std::cell::Cell<u32>,
    /// While a body is inferred again inside the range it had, the first
    /// identity past that range.
    owner_ceiling: std::cell::Cell<Option<u32>>,
    /// Whether the body being inferred outgrew its range, so its locals no
    /// longer form one.
    owner_range_split: std::cell::Cell<bool>,
    /// How many leading entries of `enclosing_type_params` are the enclosing
    /// STRUCT's own parameters (method-own parameters are appended after
    /// them while a method checks). Member origin clauses reject bare
    /// references into this prefix; later (method-own) binders stay bare.
    enclosing_struct_type_params: std::cell::Cell<usize>,
    /// True while resolving a signature (return) annotation: an origin
    /// argument that is syntactically origin-shaped but names places only the
    /// body can resolve (`origin_of(self.entries)`) is accepted and erased
    /// instead of failing place resolution.
    signature_origin_leniency: std::cell::Cell<bool>,
    /// Index of the local scope for each function currently being checked.
    function_bases: Vec<usize>,
    /// Function/method scope base and caller-owned inputs which may legally
    /// appear inside a returned reference-bearing aggregate.
    aggregate_escape_contexts: Vec<(usize, HashSet<mojito_types::origin::OwnerId>)>,
    /// Explicit capture policy for each nested function body being checked.
    capture_contexts: RefCell<Vec<CapturePolicy>>,
    /// Defined structs, by name (a separate namespace from value bindings).
    structs: declaration_table::DeclarationTable<StructInfo>,
    /// Top-level struct symbols in the checked program, collected before body
    /// checking. Concrete Tuple types can therefore select their generated
    /// implementation independent of declaration order.
    declared_structs: declaration_table::DeclarationSet,
    /// The program's overloaded top-level names, so an overloaded `def` owns
    /// its binders under the signature-qualified symbol MIR names it by: its
    /// own, and an imported package's, answered from the package's
    /// declarations. Shared with the checked program it carries to MIR.
    overload_sets: std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    /// The program's module integer constants, each folded once before any
    /// declaration's symbol spells it: its own, and an imported package's.
    /// Shared with the checked program it carries to MIR.
    program_constants: std::sync::Arc<mojito_checked::package::ProgramConstants>,
    /// The binder owner of each struct method, shared with the elaborator.
    method_binder_owners: mojito_symbol::symbol::MethodBinderOwners,
    /// The wrapper of every trait default a top-level struct inherits, in
    /// program order.
    default_wrappers: Vec<trait_defaults::DefaultWrapper>,
    /// Every trait default the check checked as its trait's member, in
    /// program order.
    trait_members: Vec<trait_defaults::TraitMember>,
    /// Whether the declaration `check_def` checks next is a trait member,
    /// whose receiver is a method receiver.
    checking_trait_member: bool,
    /// Top-level structs whose shell, member types, and method signatures were
    /// registered by `check_program`'s order-independent pre-passes. The
    /// source-order walk removes each entry and runs only the completion phase
    /// (conformance verification and method bodies).
    predeclared_structs: HashSet<String>,
    /// The module constants the declaration pass bound ahead of their
    /// statements, which a body before the statement reads; the statement's
    /// own binding replaces the pre-bound one.
    predeclared_constants: HashSet<String>,
    /// The module constants whose initializer applies a callable, or reads
    /// such a constant: a parameter expression, never a type, so
    /// `comptime E = A` over one is a value binding and not an alias.
    applied_module_constants: declaration_table::DeclarationSet,
    /// The object of the field read being inferred: a compile-time value
    /// read there is projected, never materialized whole.
    projected_object: RefCell<Option<SourceSpan>>,
    /// Top-level traits registered by `check_program`'s pre-pass; the walk
    /// removes each entry instead of re-registering.
    predeclared_traits: HashSet<String>,
    /// Defined traits, by name (their method requirements).
    traits: declaration_table::DeclarationTable<TraitInfo>,
    /// Stack of a generic `def`'s checked type parameters, innermost last. A
    /// bare `T` annotation resolves to the complete `TyKind::Param`, including any
    /// anonymous callable-trait contract.
    /// (A `def`'s *value* parameters are ordinary `Int` locals, not here.)
    tparams: Vec<HashMap<String, Ty>>,
    /// Value parameters in scope by bare name, innermost last: the typed
    /// references a dependent parameter expression resolves its names to.
    vparams: Vec<HashMap<String, ParamExpr>>,
    /// Variadic packs in scope by bare name, type packs (`*Ts`) and value
    /// packs (`*values: T`) alike, level for level beside `tparams`: the
    /// parameter-list references a pack element indexes. A value pack is
    /// here and a raw parameter list (`values: KGENParamList[T]`), whose
    /// references share its meta-type, is not.
    pack_params: Vec<HashMap<String, ParamExpr>>,
    /// The dependent pack element each bounded view of one stands for, by the
    /// view's parameter name (see `opaque_element`).
    pack_element_views: RefCell<HashMap<String, Ty>>,
    /// The compilation's parameter-expression context, shared with every
    /// checker run and the elaborator of the same compilation.
    param_context: ParamContext,
    /// The enclosing struct's parameters while checking its fields and methods,
    /// so `Self.T` resolves to `TyKind::Param` and `Self.n` to a value parameter.
    /// Saved/restored around a (possibly nested) struct definition.
    self_decls: Vec<ParamDecl>,
    /// What each enclosing scope knows, innermost last, as upstream's
    /// per-declaration `knownAssumptions`: the conjuncts of a declaration's
    /// own `where` clauses, and of a `comptime if`, `comptime assert`, or
    /// case guard condition, each a canonical Bool proposition. A
    /// conformance fact is a `conforms_to`, `IsTrivially*`, or pack
    /// `conforms_to` atom. Facts are kept apart from `self_decls`: adding a
    /// temporary bound to the declaration changes the identity of
    /// `TyKind::Param`.
    assumptions: RefCell<Vec<Vec<ParamExpr>>>,
    /// Source-validation mode (`validate_comptime_templates`): compile-time
    /// control flow is checked with every arm visited and the declaration's
    /// parameters symbolic, and a module-level function or method body
    /// without such a construct is declared but not checked.
    /// Per-scope function-local `comptime NAME = <type>` aliases, each the
    /// type its parameter expression names. Consulted ahead of `tparams`.
    local_type_aliases: Vec<HashMap<String, Ty>>,
    /// The parameter expression each of `local_type_aliases` was bound to,
    /// as written: a call of the alias constructs its application.
    local_type_alias_values: Vec<HashMap<String, Expr>>,
    /// Per-scope function-local `comptime NAME = <compile-time value>`
    /// bindings the checker cannot type as a runtime value (a `TypeList`
    /// construction); a compile-time position inlines the defining
    /// expression in their place.
    local_comptime_values: Vec<HashMap<String, Expr>>,
    /// Per-scope function-local `comptime NAME = <parameter expression>`
    /// bindings over the binders of the template body being checked, each
    /// beside the parameter-scope depth it was bound at: a compile-time
    /// position reads the expression, as it reads a value parameter.
    local_comptime_parameters: Vec<HashMap<String, (usize, ParamExpr)>>,
    /// Per-scope function-local `comptime NAME = <display>` bindings over
    /// the binders of the template body being checked, whose display the
    /// elaborator evaluates per instance: a `comptime for` over the name
    /// iterates the binding.
    local_comptime_displays: Vec<HashMap<String, comptime_validation::BoundDisplay>>,
    /// The local `comptime` bindings the executable check has declared, each
    /// by its binding: a compile-time expression the elaborator evaluates
    /// per instance may read them.
    comptime_binding_owners: HashSet<mojito_types::origin::OwnerId>,
    /// The package interfaces the check resolves its misses from, and the
    /// first of the owners reserved for their module bindings, which bind
    /// below the module scope ([`Self::installed_binding`]).
    installed: Option<(InstalledInterfaces, u32)>,
    /// How many type or parameter arguments, or local `comptime` values,
    /// the check is evaluating: an `Int` or `Bool` expression there that
    /// compiles to no parameter expression is lifted as a function
    /// ([`Self::lifted_application`]).
    lifting_positions: std::rc::Rc<std::cell::Cell<usize>>,
    /// The module functions and static methods a compile-time call applies
    /// by name, every overload under the name a call spells
    /// ([`comptime_validation::applicable_functions`]): known before any
    /// declaration is checked, since a field type or a signature may call
    /// one declared below it.
    applicable_functions:
        declaration_table::DeclarationTable<Vec<comptime_validation::ApplicableFunction>>,
    /// The declared result of each non-generic module `def` that declares
    /// one, the first declaration's under an overloaded name: what a module
    /// constant applying it denotes ([`Self::applied_constant_expr`]).
    module_def_results: declaration_table::DeclarationTable<SourceType>,
    /// The expressions lifted so far, each with the application it denotes.
    lifted_applications: RefCell<Vec<comptime_validation::LiftedApplication>>,
    /// The lowered name of the declaration whose body is being checked,
    /// which owns every function the check lifts from it; `None` at module
    /// scope (`__toplevel__`).
    lifting_owner: Option<String>,
    /// The index of the next function the check lifts, per owning
    /// declaration.
    lifted_counts: RefCell<HashMap<String, usize>>,
    /// The element type `paramfor_next_value` gives over each iterator
    /// type a `comptime for` stepped, typed once per type.
    comptime_iterator_elements: RefCell<HashMap<Ty, Ty>>,
    /// The sequence of each local display binding, by its owning
    /// declaration and identity, so that a binding checked again denotes
    /// the function it was first given.
    lifted_displays: RefCell<HashMap<(String, mojito_types::origin::OwnerId), ParamExpr>>,
    /// The application each lifted expression of the executable check
    /// denotes, by the expression: the pass records them on the checked
    /// operations when it ends ([`Self::record_lifted_applications`]), after
    /// every inference of the expression has recorded its own.
    lifted_expressions: RefCell<FactMap<SourceSpan, ParamExpr>>,
    /// The retypings the erased `rebind[Dest](value)` calls left behind (see
    /// `rebind.rs`).
    rebind_targets: RebindTargets,
    /// The bodies a `rebind` keys, scanned before the erasure removed the
    /// calls. Source validation checks exactly these beside the
    /// compile-time-keyed ones; empty in the executable check, which sees
    /// only clones.
    /// The bodies this validation run reached no verdict on
    /// (`symbolic_verdict`), keyed at the body's first statement.
    /// The positional collectors declared `var *args`: forwarding one as a
    /// whole needs the `^`, and a read one cannot be transferred.
    owned_packs: HashSet<mojito_types::origin::OwnerId>,
    /// The bindings of executable bodies' value parameters. Each occupies a
    /// runtime slot but denotes a compile-time value, so a nested function
    /// or lambda reads it through an implicit `imm` capture.
    value_parameter_owners: HashSet<mojito_types::origin::OwnerId>,
    /// The loop and comprehension binders not declared `var`: each binds
    /// the yielded element by reference, as upstream, so it is no tracked
    /// root a `^` may move out of.
    reference_binders: HashSet<mojito_types::origin::OwnerId>,
    /// Whether each function's positional collector is `var`, by name, for
    /// the ownership a forwarded pack must match (`TyKind::GenericFunc` carries
    /// the regular conventions only). A name whose overloads disagree is
    /// absent.
    owned_collectors: declaration_table::DeclarationTable<bool>,
    /// Each function's positional collector's parameter name, by function
    /// name, for the argument exclusivity rule's diagnostics. A name whose
    /// overloads disagree is absent.
    collector_names: declaration_table::DeclarationTable<String>,
    /// Per-scope compile-time bindings of a validated body: `comptime for`
    /// variables and the declaration's value parameters. The elaborator
    /// substitutes each as a literal, so a nested function or lambda reading
    /// one captures nothing.
    compile_time_bindings: Vec<HashSet<String>>,
    /// How many compile-time positions enclose the expression being typed: a
    /// `comptime` binding's value, a `comptime if` condition, a `comptime
    /// for` iterable, a `materialize[...]` operand
    /// ([`Self::comptime_position`]). A compile-time collection is read
    /// freely there and crosses nowhere else.
    comptime_positions: std::rc::Rc<std::cell::Cell<usize>>,
    enclosing_type_params: Vec<mojito_ast::ast::TypeParam>,
    /// The `Ty` denoted by a bare `Self` while checking a struct's members (the
    /// struct type) or a trait's requirements (`TyKind::SelfType`). `None` elsewhere.
    self_ty: Option<Ty>,
    /// Trait-associated comptime requirements in scope while checking a trait's
    /// own method requirement signatures, so `Self.Element` can resolve.
    trait_self_comptime: Vec<HashMap<String, CtMemberReq>>,
    /// Exact integer constants declared by `comptime NAME = value`.
    comptimes: declaration_table::DeclarationTable<mojito_common::literal::IntLiteral>,
    /// Module constants declared by `comptime NAME = <literal>`, which a
    /// trait requirement's default folds to (`Float`, `Bool`, string).
    comptime_literals: declaration_table::DeclarationTable<Expr>,
    /// Module constants whose initializer folds to a closed attribute
    /// (`comptime D = DType.int16`, `comptime T = (1, 2)`, `comptime K =
    /// Int32(4)`, a string, a vector of literal lanes): the value a body's
    /// read denotes (decision D3).
    module_values: declaration_table::DeclarationTable<CtValue>,
    /// Module constants whose initializer applies a function (`comptime B =
    /// f(A)`, `comptime S = size_of[T]()`), as the symbolic application the
    /// pin keeps through the check (decision D3,
    /// `docs/notes/ctfe-request-path.md`).
    comptime_applied: declaration_table::DeclarationTable<ParamExpr>,
    /// Dtypes declared by `comptime NAME = <dtype>`, for SIMD element-type
    /// arguments, one map per lexical scope: a binding is visible to the
    /// rest of its block. A binding of a `[dt: DType]` parameter keys a lane
    /// symbolically, so a template body may name its own dtype through one.
    comptime_dtypes: Vec<HashMap<String, SimdDtype>>,
    /// Generic top-level type aliases declared by `comptime NAME[params] = Type`,
    /// expanded per application during type resolution. See [`ComptimeAlias`].
    comptime_aliases: declaration_table::DeclarationTable<ComptimeAlias>,
    /// Whether `self` is writable in the method body being checked — set while
    /// checking a `mut self` method's body (so `self.x = e` is allowed there).
    self_mutable: bool,
    /// An `out self` lifecycle initializer is establishing field storage.  For
    /// a reference-valued field, assigning a reference here stores its handle;
    /// later assignments write through the established handle instead.
    self_initializing: bool,
    /// Per-method accumulation of symbolic origin write requirements inherited
    /// from calls through generic receiver fields. The completed frame is
    /// merged into that method's registered signature.
    parametric_write_frames: RefCell<Vec<Vec<mojito_types::origin::OriginParamId>>>,
    /// Whether a parameter annotation is being resolved: the only position
    /// where a bare `Pointer[T, _]` placeholder is concrete (as the immutable
    /// alias; upstream infers the origin per call there and rejects the
    /// placeholder elsewhere as not concrete).
    resolving_parameter_annotation: std::cell::Cell<bool>,
    /// Set while resolving a parameter annotation that is exactly
    /// `StringLiteral` or `IntLiteral`: upstream's value-parameterized
    /// literal types infer their parameter there and are not concrete
    /// anywhere else.
    bare_literal_parameter: std::cell::Cell<bool>,
    /// Set while a subscript's getter is resolved only to learn its element
    /// type (upstream's `filterOverloadSet` in `emitGetterSetterAccess`):
    /// nothing is emitted, so its `raises` is no effect of the body.
    resolving_accessor_only: std::cell::Cell<bool>,
    /// The declaration currently being checked comes from the bundled
    /// standard-library crossing package (`stdlib/std/memory/`). Only such
    /// declarations may name compiler-private storage types (`__UninitStorage`)
    /// in sourceless type-annotation positions.
    bundled_stdlib_declaration: bool,
    /// The declaration currently being checked is an `enum`, whose
    /// generated members name the compiler's case-indexed `__EnumStorage`.
    enum_declaration: bool,
    /// Every fact the check decided at an occurrence, in the occurrence's one
    /// record (`node_facts`): the checked handoff, written as it is decided.
    node_facts: RefCell<FactMap<SourceSpan, mojito_checked::checked::NodeFacts>>,
    /// Free functions decorated `@__unsafe_nested_origins_read_only`; see
    /// [`MethodSig::nested_origins`].
    nested_origins_read_only_functions: declaration_table::DeclarationSet,
    /// The type binders bounded by a trait-valued sibling (`T: Tr`), each
    /// with that sibling: as upstream decides nothing of a `ParamType` over
    /// the trait metatype, a value of one is destroyed by the generic
    /// default, and a pack's `Ts.values` is a list of `Tr`.
    trait_bounded_binders: HashMap<mojito_types::param_expr::ParamId, ParamRef>,
    /// The types a generic `def` call solved for the callee's binders, held
    /// from the call's typing until its transfer replay takes them.
    call_bindings: RefCell<FactMap<SourceSpan, TySubst>>,
    /// Every generic-struct application reached as a constructor target or
    /// method-call receiver, a checked fact of the call.
    struct_instantiations: RefCell<FactVec<mojito_checked::checked::StructInstantiation>>,
    /// Per-body accumulation frames for inferred loan-transfer effects.
    transfer_frames: RefCell<Vec<TransferFrame>>,
    /// Inferred per-callable transfer effects, keyed by callable name
    /// (`name` / `Struct.method`); consulted at later call sites.
    transfer_effects: RefCell<FactMap<String, Vec<mojito_checked::checked::TransferEffect>>>,
    /// First-seen callee effects per `apply_transfer_effects` lookup. The
    /// two-phase pass reruns the check when a callee's final committed
    /// effects differ from what its stalest call-site query observed.
    effect_observations: RefCell<HashMap<String, Vec<mojito_checked::checked::TransferEffect>>>,
    /// Inferred higher-order call-through residues per callable, keyed like
    /// `transfer_effects`; each call site resolves them against the concrete
    /// callable it supplies.
    call_through_effects: RefCell<FactMap<String, Vec<mojito_checked::checked::CallThroughEffect>>>,
    /// First-seen call-through observations, mirroring `effect_observations`.
    call_through_observations:
        RefCell<HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>>>,
    /// Origins transferred into a binding by a callee's store (keyed by the
    /// binding's owner) — an interior-mutability overlay over the
    /// aggregate-origin scopes, merged on lookup.
    transferred_origins:
        RefCell<FactMap<mojito_types::origin::OwnerId, Vec<mojito_types::origin::Origin>>>,
    /// Innermost-last observation frames recording whether a raising
    /// operation that escapes the bracketed region was checked inside it.
    /// Owned iteration over linear elements pushes one around its body —
    /// such a call aborts the loop and abandons residual explicit-destroy
    /// obligations — and every callable body pushes a barrier frame so a
    /// nested `def`'s raising calls never mark an enclosing loop. The
    /// `usize` is the `handled_raise_depth` at region entry: a call under a
    /// deeper depth is contained by a `try` inside the region and does not
    /// escape it, while a handler outside the region still lets the error
    /// abort the region itself.
    raise_observation_frames: RefCell<Vec<(usize, bool)>>,
    /// Whether type resolution is inside a storage annotation (a struct field
    /// or local `var` type) — the positions where explicit origin slots must
    /// be bound. An initialized local may still leave a BARE generic to infer
    /// wholly from its initializer (upstream-attested via `StringSlice`);
    /// fields and uninitialized locals may not.
    strict_storage_annotation: std::cell::Cell<StorageStrictness>,
    /// The nesting depth of the annotation being resolved: `1` at the
    /// outermost application, more inside its type arguments, where a struct
    /// must bind its origin slots whatever the annotation position allows.
    annotation_depth: std::cell::Cell<u32>,
    /// Whether the annotation being resolved belongs to a compiler-generated
    /// (`$`-mangled) declaration, whose spellings rebind already-checked types
    /// and so keep their position's leniency at every nesting depth.
    generated_declaration: std::cell::Cell<bool>,
    /// While a storage annotation is resolved with
    /// `resolve_storage_annotation_with_origins`, the explicit origin
    /// arguments its outermost struct application supplied (`(slot name,
    /// origin)`), so the binding site can judge the initializer against them
    /// after they are erased from the type. `None` = not collecting.
    storage_origin_demands: RefCell<Option<Vec<(String, mojito_types::origin::Origin)>>>,
    /// Per construction or view-returning call expression, the origin
    /// binders it bound immutably (see `immutable_origin_binder_scopes`),
    /// nested constructions' included under their field path; a `var`
    /// binding of the expression copies them into
    /// `immutable_origin_binder_scopes`.
    construction_immutable_binders: RefCell<FactMap<SourceSpan, Vec<ImmutableOriginBinder>>>,
    /// Per view-returning call, the concrete origin each origin slot of the
    /// returned struct binds (`make(xs)` over `-> P[origin_of(xs)]` binds
    /// `P`'s slot to `xs`), from the callee's [`ViewReturnOrigin`] contract
    /// substituted with the call's places, with the callee's capability over
    /// the place when the contract fixes it. `aggregate_field_origins` and
    /// `aggregate_origins` read it, so a call result — and a local bound
    /// from one — resolves a pointer field's symbolic binder exactly as a
    /// direct construction does.
    call_result_origins: RefCell<FactMap<SourceSpan, Vec<CallResultOrigin>>>,
    /// Per unpacked value, what [`Self::tuple_unpack_plan`] built the plan
    /// from beside the value's type. Checker-only: a template keeps it as the
    /// recipe of its plan.
    /// Per comprehension, the iterable of the clause that declares each of
    /// its generator binders, in clause order. Checker-only: a template keeps
    /// it as the recipe of each binder's plan.
    comprehension_iterables: RefCell<FactMap<SourceSpan, Vec<SourceSpan>>>,
    /// Per nested `def` statement, the bindings of its runtime parameters,
    /// in declaration order. Checker-only: a template keeps them as locals
    /// its statement declares.
    nested_def_params: RefCell<FactMap<SourceSpan, Vec<mojito_types::origin::OwnerId>>>,
    /// Per view-returning method call, the owned-interior tags its declared
    /// return origin projects on the receiver (see
    /// [`MethodSig::view_return_interior`]). Checker-only: MIR loans stay
    /// whole-place.
    view_result_interiors: RefCell<FactMap<SourceSpan, Vec<String>>>,
    /// Per selected call, the callee's runtime parameters in declaration
    /// order, for rules that name a parameter or read a free call's
    /// conventions (free calls record no `CheckedCallContract`).
    call_parameters: RefCell<FactMap<SourceSpan, Vec<CallParameter>>>,
    declaration_types: RefCell<FactMap<mojito_checked::checked::AnnotationSite, Ty>>,
    generic_parameters:
        RefCell<FactMap<mojito_checked::checked::GenericSite, Vec<mojito_types::types::ParamDecl>>>,
    /// Checked raising contract and reference-return fact per callable
    /// declaration; lowering never re-reads source `raises`/return syntax.
    declaration_effects: RefCell<
        FactMap<
            mojito_checked::checked::AnnotationSite,
            mojito_checked::checked::DeclarationEffect,
        >,
    >,
    /// Each desugared statement's replacement (a `with` statement or a
    /// sequence destructuring assignment, keyed by the statement), spliced
    /// into the final tree after the last transfer round.
    statement_desugars: RefCell<FactMap<SourceSpan, desugar::StatementDesugar>>,
    /// Positive site-sensitive drop facts retained for the later explicit-
    /// destroy CFG pass. Conditional conformances are meaningful only in the
    /// constraint environment in which a binding was checked.
    explicit_destroy_deletability: RefCell<crate::explicit_destroy::CheckedDeletability>,
    /// Reference-result reads proven copyable in their site-sensitive generic
    /// constraint environment. Final validation runs after those method scopes
    /// have been popped, so it must retain rather than recompute this fact.
    copyable_reference_result_reads: RefCell<FactSet<SourceSpan>>,
    /// Reference results a statement discards (`f.params(1)` as a
    /// statement, `_ = f.params(1)`): no value is read, so no copy is
    /// required (upstream only warns that the value is unused).
    discarded_reference_results: RefCell<FactSet<SourceSpan>>,
    /// Reference-result expressions used as method-call receivers. The call
    /// borrows the referent for its `self` convention (consuming receivers
    /// are gated separately), so the result is never read out as a value.
    borrowed_reference_receivers: RefCell<FactSet<SourceSpan>>,
    /// Each slice literal's operands (lower, upper, stride), by the
    /// checker-only argument standing for it while its subscript's overload
    /// is selected (`synthetic_slice_descriptor`).
    slice_literals: RefCell<HashMap<SourceSpan, [Option<Expr>; 3]>>,
    /// Compile-time bindings read as the object of a field chain the checker
    /// compiled to a parameter expression (`q` of `q.a`): the chain is read,
    /// never the binding, which therefore materializes no temporary.
    parameter_field_objects: RefCell<FactSet<SourceSpan>>,
    /// Expressions whose value nothing takes ownership of: a temporary bound
    /// to a read parameter, an argument of `print`, a discarded statement
    /// expression. A linear one is abandoned there.
    unconsumed_temporaries: RefCell<FactSet<SourceSpan>>,
    /// Call results typed by one of the enclosing body's own type parameters
    /// whose bounds do not prove `Deinitable`: the caller owns each and cannot
    /// destroy it (`explicit_destroy`'s abandoned-temporary rule).
    linear_temporaries: RefCell<FactMap<SourceSpan, crate::explicit_destroy::Linear>>,
    /// Iterables whose selected `__iter__` raises: the loop header is a
    /// raise point (`explicit_destroy`'s raise edges).
    raising_iterations: RefCell<FactSet<SourceSpan>>,
    return_ref_contracts: Vec<Option<ReturnRefContract>>,
    /// The enclosing body's value-return annotation, re-resolved over the
    /// body's own places to judge a returned struct's origin tail, flagged
    /// when it belongs to a compiler-generated (`$`-mangled) specialization;
    /// `None` for a reference return.
    return_annotations: Vec<Option<(mojito_ast::ast::SourceType, bool)>>,
    named_result_context: Vec<bool>,
    raising_context: Vec<Option<Ty>>,
    handled_raise_depth: usize,
    handled_raise_types: RefCell<Vec<Vec<Ty>>>,
    uninitialized: RefCell<HashSet<mojito_types::origin::OwnerId>>,
    /// The compilation's check context. The driver lends it for one check
    /// and takes it back; every other entry point checks in a default one.
    context: RefCell<CheckContext>,
    /// The identity each occurrence had before the final re-key, which the
    /// bound-default binding keys by.
    syntax_origins: mojito_ast::ast::SyntaxOrigins,
    /// The requirement defaults each call through a bound leaves out, by the
    /// call's origin (`bound_defaults.rs`).
    bound_default_arguments: RefCell<HashMap<SyntaxId, BoundDefaultArguments>>,
    /// The module and name of the struct whose method is being checked, for
    /// the method's display name.
    method_site: Option<(Option<String>, String)>,
    /// The identity of every module-level callable and method whose body
    /// this pass checked, whose effect summaries a package keeps.
    top_level_callables: RefCell<HashSet<String>>,
    /// The previous checker pass, whose facts a body site with unchanged
    /// inputs takes instead of being inferred (`body_carry`).
    previous: Option<PassCarry>,
    /// What each body site of this pass wrote, for the next pass.
    body_records: RefCell<HashMap<SourceSpan, body_carry::BodyRecord>>,
    /// The effect entries the open body site has read, exactly as read.
    site_reads: RefCell<Option<Vec<(String, body_carry::ObservedEffects)>>>,
}

impl Checker {
    pub fn new() -> Self {
        Self::new_seeded(HashMap::new(), HashMap::new())
    }

    /// Whether a value of `ty` is memory-only, which upstream passes to and
    /// returns from a call by address. A compiler-known type upstream
    /// declares `TrivialRegisterPassable` (a scalar, a pointer, a literal)
    /// and a function value, which upstream holds as a raw MLIR value, are
    /// passed in registers; a closure's storage is a struct, passed as its
    /// captures allow.
    pub(crate) fn memory_only(&self, ty: &Ty) -> bool {
        let function = ty.callable().is_some_and(|func| {
            !matches!(
                func.environment,
                mojito_types::origin::CallableEnvironment::Storage(_)
            )
        });
        !mojito_types::types::trivially_register_passable_leaf(ty.kind())
            && !function
            && self.register_passability(ty) != traits::RegisterPassability::Proven
    }

    fn new_seeded(
        transfer_seed: HashMap<String, Vec<mojito_checked::checked::TransferEffect>>,
        call_through_seed: HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>>,
    ) -> Self {
        // The bundled seeds are never inferred from bodies, so a prior
        // round's committed map overlays them rather than replacing them.
        let mut transfer_effects = seeded_transfer_effects();
        for (callable, effects) in transfer_seed {
            let entry = transfer_effects.entry(callable).or_default();
            for effect in effects {
                if !entry.contains(&effect) {
                    entry.push(effect);
                }
            }
        }
        Self {
            scopes: vec![HashMap::new()],
            mutable_scopes: vec![HashMap::new()],
            owner_scopes: vec![HashMap::new()],
            aggregate_origin_scopes: vec![HashMap::new()],
            aggregate_field_origin_scopes: vec![HashMap::new()],
            storage_origin_demand_scopes: vec![HashMap::new()],
            immutable_origin_binder_scopes: vec![HashMap::new()],
            construction_immutable_binders: RefCell::new(FactMap::default()),
            call_result_origins: RefCell::new(FactMap::default()),
            reference_parameter_scopes: vec![HashMap::new()],
            reference_parameter_binders: HashMap::new(),
            callable_origin_scopes: vec![HashMap::new()],
            next_owner: std::cell::Cell::new(0),
            fresh_owner_cursor: std::cell::Cell::new(0),
            owner_ceiling: std::cell::Cell::new(None),
            owner_range_split: std::cell::Cell::new(false),
            signature_origin_leniency: std::cell::Cell::new(false),
            enclosing_struct_type_params: std::cell::Cell::new(0),
            function_bases: Vec::new(),
            aggregate_escape_contexts: Vec::new(),
            capture_contexts: RefCell::new(Vec::new()),
            structs: declaration_table::DeclarationTable::default(),
            declared_structs: declaration_table::DeclarationSet::default(),
            overload_sets: std::sync::Arc::default(),
            program_constants: std::sync::Arc::default(),
            method_binder_owners: mojito_symbol::symbol::MethodBinderOwners::default(),
            default_wrappers: Vec::new(),
            trait_members: Vec::new(),
            checking_trait_member: false,
            predeclared_structs: HashSet::new(),
            predeclared_constants: HashSet::new(),
            applied_module_constants: declaration_table::DeclarationSet::default(),
            projected_object: RefCell::new(None),
            predeclared_traits: HashSet::new(),
            traits: declaration_table::DeclarationTable::default(),
            tparams: Vec::new(),
            vparams: Vec::new(),
            pack_params: Vec::new(),
            pack_element_views: RefCell::new(HashMap::new()),
            param_context: ParamContext::detached(),
            self_decls: Vec::new(),
            assumptions: RefCell::new(Vec::new()),
            local_type_aliases: vec![HashMap::new()],
            local_type_alias_values: vec![HashMap::new()],
            local_comptime_values: vec![HashMap::new()],
            local_comptime_parameters: vec![HashMap::new()],
            local_comptime_displays: vec![HashMap::new()],
            comptime_binding_owners: HashSet::new(),
            installed: None,
            lifting_positions: std::rc::Rc::default(),
            applicable_functions: declaration_table::DeclarationTable::default(),
            module_def_results: declaration_table::DeclarationTable::default(),
            lifted_applications: RefCell::new(Vec::new()),
            lifting_owner: None,
            lifted_counts: RefCell::new(HashMap::new()),
            comptime_iterator_elements: RefCell::new(HashMap::new()),
            lifted_displays: RefCell::new(HashMap::new()),
            lifted_expressions: RefCell::new(FactMap::default()),
            rebind_targets: RebindTargets::default(),
            owned_packs: HashSet::new(),
            value_parameter_owners: HashSet::new(),
            reference_binders: HashSet::new(),
            owned_collectors: declaration_table::DeclarationTable::default(),
            collector_names: declaration_table::DeclarationTable::default(),
            compile_time_bindings: vec![HashSet::new()],
            comptime_positions: std::rc::Rc::default(),
            enclosing_type_params: Vec::new(),
            self_ty: None,
            trait_self_comptime: Vec::new(),
            comptimes: declaration_table::DeclarationTable::default(),
            comptime_literals: declaration_table::DeclarationTable::default(),
            module_values: declaration_table::DeclarationTable::default(),
            comptime_applied: declaration_table::DeclarationTable::default(),
            comptime_dtypes: vec![HashMap::new()],
            comptime_aliases: declaration_table::DeclarationTable::default(),
            self_mutable: false,
            self_initializing: false,
            parametric_write_frames: RefCell::new(Vec::new()),
            bundled_stdlib_declaration: false,
            enum_declaration: false,
            node_facts: RefCell::new(FactMap::default()),
            nested_origins_read_only_functions: declaration_table::DeclarationSet::default(),
            trait_bounded_binders: HashMap::new(),
            call_bindings: RefCell::new(FactMap::default()),
            struct_instantiations: RefCell::new(FactVec::default()),
            transfer_frames: RefCell::new(Vec::new()),
            transfer_effects: RefCell::new(transfer_effects.into()),
            resolving_parameter_annotation: std::cell::Cell::new(false),
            bare_literal_parameter: std::cell::Cell::new(false),
            resolving_accessor_only: std::cell::Cell::new(false),
            effect_observations: RefCell::new(HashMap::new()),
            call_through_effects: RefCell::new(call_through_seed.into()),
            call_through_observations: RefCell::new(HashMap::new()),
            transferred_origins: RefCell::new(FactMap::default()),
            raise_observation_frames: RefCell::new(Vec::new()),
            strict_storage_annotation: std::cell::Cell::new(StorageStrictness::Off),
            annotation_depth: std::cell::Cell::new(0),
            generated_declaration: std::cell::Cell::new(false),
            storage_origin_demands: RefCell::new(None),
            comprehension_iterables: RefCell::new(FactMap::default()),
            nested_def_params: RefCell::new(FactMap::default()),
            view_result_interiors: RefCell::new(FactMap::default()),
            call_parameters: RefCell::new(FactMap::default()),
            declaration_types: RefCell::new(FactMap::default()),
            generic_parameters: RefCell::new(FactMap::default()),
            declaration_effects: RefCell::new(FactMap::default()),
            statement_desugars: RefCell::new(FactMap::default()),
            explicit_destroy_deletability: RefCell::new(
                crate::explicit_destroy::CheckedDeletability::default(),
            ),
            copyable_reference_result_reads: RefCell::new(FactSet::default()),
            discarded_reference_results: RefCell::new(FactSet::default()),
            borrowed_reference_receivers: RefCell::new(FactSet::default()),
            slice_literals: RefCell::new(HashMap::new()),
            parameter_field_objects: RefCell::new(FactSet::default()),
            unconsumed_temporaries: RefCell::new(FactSet::default()),
            linear_temporaries: RefCell::new(FactMap::default()),
            raising_iterations: RefCell::new(FactSet::default()),
            return_ref_contracts: Vec::new(),
            return_annotations: Vec::new(),
            named_result_context: Vec::new(),
            raising_context: Vec::new(),
            handled_raise_depth: 0,
            handled_raise_types: RefCell::new(Vec::new()),
            uninitialized: RefCell::new(HashSet::new()),
            context: RefCell::new(CheckContext::default()),
            syntax_origins: mojito_ast::ast::SyntaxOrigins::default(),
            bound_default_arguments: RefCell::new(HashMap::new()),
            top_level_callables: RefCell::new(HashSet::new()),
            method_site: None,
            previous: None,
            body_records: RefCell::new(HashMap::new()),
            site_reads: RefCell::new(None),
        }
    }

    fn raising_allowed(&self) -> bool {
        self.handled_raise_depth > 0
            || self
                .raising_context
                .last()
                .is_some_and(|error| error.as_ref().is_some_and(|ty| *ty != TyKind::Never))
    }

    fn require_error(&self, operation: impl Into<String>, error: Ty) -> Result<(), TypeError> {
        if self.resolving_accessor_only.get() {
            return Ok(());
        }
        // A raising operation escapes the innermost observed region (a
        // linear owned-iteration body) unless a `try` INSIDE that region
        // contains it — a handler outside the region still aborts the region.
        if let Some((baseline, flag)) = self.raise_observation_frames.borrow_mut().last_mut()
            && self.handled_raise_depth <= *baseline
        {
            *flag = true;
        }
        if self.handled_raise_depth > 0 {
            if let Some(types) = self.handled_raise_types.borrow_mut().last_mut() {
                types.push(error);
            }
            return Ok(());
        }
        if let Some(Some(expected)) = self.raising_context.last() {
            if *expected == error {
                return Ok(());
            }
            return Err(TypeError::RaiseTypeMismatch {
                expected: expected.to_string(),
                found: error.to_string(),
            });
        }
        if self.raising_allowed() {
            Ok(())
        } else {
            Err(TypeError::UnhandledRaise(operation.into()))
        }
    }

    fn record_call_effect(&self, span: SourceSpan, error: Ty) {
        self.record_at(span).effects = Some(mojito_checked::checked::EffectFacts {
            raises: Some(error),
            may_suspend: false,
            diverges: false,
        });
    }

    fn concrete_callable_captures(&self, ty: &Ty) -> Vec<mojito_types::origin::CaptureOrigin> {
        use mojito_types::origin::CaptureOriginSet;
        let callable = match ty.kind() {
            TyKind::Struct(name, _) => self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref()),
            _ => Some(ty),
        };
        match callable
            .and_then(Ty::callable)
            .and_then(|func| func.environment.captures())
        {
            Some(CaptureOriginSet::Concrete(captures)) => captures.clone(),
            _ => Vec::new(),
        }
    }

    /// Retain every concrete environment access that may happen while a call is
    /// active. Callable arguments are included because Mojito supports only
    /// non-escaping downward funargs: their environments may be invoked by the
    /// callee before it returns.
    fn call_capture_effects<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<mojito_types::origin::CaptureOrigin> {
        use mojito_types::origin::CaptureOriginSet;
        let captures = types
            .into_iter()
            .flat_map(|ty| self.concrete_callable_captures(ty))
            .collect::<Vec<_>>();
        let CaptureOriginSet::Concrete(captures) = CaptureOriginSet::concrete(captures) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        captures
    }

    fn record_call_capture_effects<'a>(
        &self,
        span: SourceSpan,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<mojito_types::origin::CaptureOrigin> {
        let captures = self.call_capture_effects(types);
        if captures.is_empty() {
            return captures;
        }
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
        ));
        captures
    }

    fn record_call_environment_effects(
        &self,
        span: SourceSpan,
        callable: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        // Call checking may have supplied essential context to an argument
        // before capture-effect collection runs (notably an empty collection
        // display, an exact literal, or an overloaded callable value). Reuse
        // that checked type instead of independently re-inferring the syntax
        // without its selected parameter contract.
        let checked_argument_type = |expression: &Expr| {
            let checked = { self.fact(&expression.source_span(), |facts| facts.ty.as_ref()) };
            match checked {
                Some(ty) => Ok(ty),
                None => self.infer(expression),
            }
        };
        let mut types = Vec::with_capacity(1 + param_args.len() + args.len() + kwargs.len());
        types.push(callable.clone());
        for expression in self.runtime_param_arg_expressions(param_args) {
            // A bracket argument is a compile-time position.
            let _position = self.comptime_position();
            types.push(checked_argument_type(expression)?);
        }
        for argument in args {
            types.push(checked_argument_type(argument)?);
        }
        for argument in kwargs {
            types.push(checked_argument_type(&argument.value)?);
        }
        self.record_call_capture_effects(span, &types);
        Ok(())
    }

    /// Type every bracket argument a call still evaluates at run time
    /// (`Counter[1 + Self.n](i)` in a generic body), so its lowering reads
    /// checked facts. One the call already typed keeps its contextual type.
    fn type_runtime_param_args(
        &self,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(), TypeError> {
        // A bracket argument is a compile-time position.
        let _position = self.comptime_position();
        for expression in self.runtime_param_arg_expressions(param_args) {
            if !self.has_fact(&expression.source_span(), |facts| facts.ty.is_some()) {
                self.infer(expression)?;
            }
        }
        Ok(())
    }

    /// The value expressions among `param_args` that binding left as runtime
    /// values. A bare type argument (`hash[MyHasher](x)`) is a value
    /// expression syntactically; binding already resolved it as a type and
    /// recorded it as an attribute, so it is not one of them.
    fn runtime_param_arg_expressions<'a>(
        &self,
        param_args: &'a [mojito_ast::ast::ParamArg],
    ) -> Vec<&'a Expr> {
        let records = self.node_facts.borrow();
        param_args
            .iter()
            .filter_map(|argument| match argument {
                mojito_ast::ast::ParamArg::Value(expression) => Some(expression),
                mojito_ast::ast::ParamArg::Named { value, .. } => match &**value {
                    mojito_ast::ast::ParamArg::Value(expression) => Some(expression),
                    mojito_ast::ast::ParamArg::Type(_)
                    | mojito_ast::ast::ParamArg::Named { .. } => None,
                },
                mojito_ast::ast::ParamArg::Type(_) => None,
            })
            .filter(|expression| {
                records
                    .get(&expression.source_span())
                    .is_none_or(|facts| facts.argument_attribute.is_none())
            })
            .collect()
    }

    fn declared_error(
        &self,
        raises: bool,
        raises_type: Option<&SourceType>,
    ) -> Result<Option<Ty>, TypeError> {
        if !raises {
            return Ok(None);
        }
        Ok(Some(match raises_type {
            Some(error) => self.ty_from_anno(error)?,
            None => Ty::error(),
        }))
    }

    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    fn lower_callable_environment(
        &self,
        type_params: &[mojito_ast::ast::TypeParam],
        thin: bool,
        capturing: Option<&mojito_ast::ast::OriginSpec>,
    ) -> Result<mojito_types::origin::CallableEnvironment, TypeError> {
        use mojito_types::origin::{CallableEnvironment, CaptureOriginSet, CaptureSetParamId};
        if thin && capturing.is_some() {
            return Err(TypeError::Unsupported(
                "a callable type cannot be both 'thin' and 'capturing'".to_string(),
            ));
        }
        if thin {
            return Ok(CallableEnvironment::Thin);
        }
        let Some(origins) = capturing else {
            return Ok(CallableEnvironment::Default);
        };
        match origins.as_slice() {
            [] => Ok(CallableEnvironment::Capturing(CaptureOriginSet::empty())),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] if name == "_" => Ok(CallableEnvironment::Capturing(CaptureOriginSet::Infer)),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] => {
                let index = type_params
                    .iter()
                    .position(|parameter| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["OriginSet"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(CallableEnvironment::Capturing(CaptureOriginSet::Param(
                    CaptureSetParamId(index as u32),
                )))
            }
            _ => Err(TypeError::Unsupported(
                "capturing[...] requires exactly one OriginSet parameter or '_'".to_string(),
            )),
        }
    }

    /// Lower a callable annotation's origin contract. Declaration contracts
    /// normally refer to their own parameters and use `lower_ref_sig`; a
    /// contextual function-value type may additionally bind `origin_of(local)`
    /// directly to the checked local owner.
    fn lower_callable_ref_sig(
        &self,
        spec: &mojito_ast::ast::OriginSpec,
        type_params: &[mojito_ast::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<mojito_types::origin::RefSig, TypeError> {
        use mojito_types::origin::{Mutability, RefSig, SigMutability, SigOrigin};

        if let Ok(signature) = lower_ref_sig(spec, type_params, params, 0) {
            return Ok(signature);
        }
        let mut members = Vec::new();
        let mut bound_mutability = Vec::new();
        for expression in spec {
            match &expression.kind {
                ExprKind::Call {
                    name,
                    args,
                    kwargs,
                    param_args,
                } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                    if args.is_empty() {
                        return Err(TypeError::Unsupported(
                            "origin_of requires at least one place".to_string(),
                        ));
                    }
                    for argument in args {
                        let (root, path) = place_path(argument).ok_or_else(|| {
                            TypeError::Unsupported("origin_of requires places".to_string())
                        })?;
                        if root == "self" {
                            members.push(project_sig_origin(SigOrigin::Self_, &path));
                        } else if let Some(index) =
                            params.iter().position(|parameter| parameter.name == root)
                        {
                            members.push(project_sig_origin(SigOrigin::Param(index), &path));
                        } else {
                            let actual = self.reference_actual(argument)?;
                            bound_mutability.push(actual.mutability);
                            members.push(SigOrigin::Bound(actual.origin));
                        }
                    }
                }
                _ => members.push(lower_sig_origin_expression(
                    expression,
                    type_params,
                    params,
                    0,
                )?),
            }
        }
        let origin = match members.as_slice() {
            [] => SigOrigin::Infer,
            [single] => single.clone(),
            _ => SigOrigin::union(members),
        };
        let mutability = if bound_mutability.is_empty() {
            SigMutability::Infer
        } else if bound_mutability
            .iter()
            .all(|mutability| *mutability == Mutability::Immutable)
        {
            SigMutability::Immutable
        } else {
            SigMutability::Mutable
        };
        Ok(RefSig { origin, mutability })
    }

    fn lower_callable_ref_param_sigs(
        &self,
        type_params: &[mojito_ast::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<Vec<Option<mojito_types::origin::RefSig>>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if parameter.convention != Some(ArgConvention::Ref) {
                    return Ok(None);
                }
                match &parameter.origin {
                    Some(spec) => self
                        .lower_callable_ref_sig(spec, type_params, params)
                        .map(Some),
                    None => Ok(Some(mojito_types::origin::RefSig {
                        origin: mojito_types::origin::SigOrigin::Infer,
                        mutability: mojito_types::origin::SigMutability::Infer,
                    })),
                }
            })
            .collect()
    }

    /// Contextually instantiate a generic function value when a monomorphic
    /// callable type supplies all of its type information. Runtime execution is
    /// still type-erased; this produces the checked callable view used by the
    /// binding or argument site.
    fn value_coerces(&self, from: &Ty, to: &Ty) -> bool {
        if coerces(from, to) {
            return true;
        }
        if let TyKind::Param { bounds, .. } = to.kind()
            && bounds.iter().all(|bound| self.conforms_to(from, bound))
        {
            return true;
        }
        if matches!(
            (from.kind(), to.kind()),
            (TyKind::GenericFunc(_), TyKind::GenericFunc(_))
        ) && callable_bound_accepts(from, to)
        {
            return true;
        }
        if matches!(
            (from.kind(), to.kind()),
            (TyKind::GenericFunc(_), TyKind::GenericFunc(_))
        ) {
            return callable_bound_accepts(from, to);
        }
        let (
            Some(GenericFuncTy { decls, func }),
            Some(FuncTy {
                params: expected_params,
                ret: expected_ret,
                ..
            }),
        ) = (from.as_generic_func(), to.as_func())
        else {
            return false;
        };
        let mut patterns = func.params.clone();
        patterns.push(func.ret.clone());
        let mut actuals = expected_params.clone();
        actuals.push((*expected_ret).clone());
        let Ok(BoundParameters { subst, .. }) =
            self.resolve_use_params("<generic callable>", decls, &[], &patterns, &actuals)
        else {
            return false;
        };
        let instantiated = Ty::new(TyKind::Func(Box::new(FuncTy {
            names: (0..func.params.len())
                .map(|index| format!("arg{index}"))
                .collect(),
            transfers: TransferSet::default(),
            ..func.map_types(|ty| substitute(ty, &subst))
        })));
        coerces(&instantiated, to)
    }

    /// Checker-selected nominal target when the concrete callable type is
    /// known, otherwise the signature-qualified abstract dispatch target for a
    /// `def(...)` value. Runtime retargeting uses the latter only when the value
    /// is a callable struct.
    fn indirect_callable_target(&self, callable: &Ty) -> Option<String> {
        if let TyKind::Struct(name, _) = callable.kind() {
            return self
                .structs
                .get(name)
                .and_then(|info| info.callable_target.clone());
        }
        callable_contract_target(callable)
    }

    /// Resolve the callable contract an actual type is *declared* to provide.
    /// Structs participate only through nominal `struct S(def(...))`
    /// conformance; merely defining a shape-compatible `__call__` is not enough.
    fn declared_callable_contract(&self, actual: &Ty) -> Option<Ty> {
        match actual.kind() {
            TyKind::Func(_) => Some(actual.clone()),
            TyKind::Param {
                callable_bound: Some(bound),
                ..
            } => Some((*bound).clone()),
            TyKind::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let contract = info.callable_conformance.as_ref()?;
                Some(substitute(contract, &struct_subst(&info.decls, arguments)))
            }
            // A generic function has a callable identity only for a matching
            // generic anonymous contract. `callable_bound_accepts` rejects it
            // when the required contract is monomorphic.
            TyKind::GenericFunc(_) => Some(actual.clone()),
            // An overload set has no unique callable identity.
            TyKind::Overload(_) => None,
            _ => None,
        }
    }

    fn validate_callable_parameter_bounds(
        &self,
        generic_name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let subst = struct_subst(decls, arguments);
        for (decl, argument) in decls.iter().zip(arguments) {
            let ParamDecl::Type {
                name,
                callable_bound: Some(bound),
                ..
            } = decl
            else {
                continue;
            };
            let TyArg::Ty(actual) = argument else {
                return Err(TypeError::Unsupported(format!(
                    "variadic callable type parameter '{}' in '{generic_name}'",
                    name.trim_start_matches('*')
                )));
            };
            let contract = substitute(bound, &subst);
            let Some(actual_contract) = self.declared_callable_contract(actual) else {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(
                        "requires one monomorphic callable type or nominal callable conformance"
                            .to_string(),
                    ),
                });
            };
            if !callable_bound_accepts(&actual_contract, &contract) {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(format!(
                        "declared callable contract '{actual_contract}' is incompatible"
                    )),
                });
            }
        }
        Ok(())
    }

    /// Check each type binder bounded by a trait-valued sibling (`T: Tr`,
    /// `*Ts: Tr`) against the trait that sibling received, as upstream
    /// substitutes the symbolic bound at the use: the declared bound the
    /// argument already satisfied is all a generic body assumes, and the
    /// bound trait may refine it.
    fn validate_trait_parameter_bounds(
        &self,
        generic_name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        for (decl, argument) in decls.iter().zip(arguments) {
            let ParamDecl::Type {
                name,
                trait_param: Some(binder),
                ..
            } = decl
            else {
                continue;
            };
            let Some(TyArg::Val(bound)) = decls
                .iter()
                .position(|sibling| sibling.id() == &binder.id)
                .and_then(|index| arguments.get(index))
            else {
                continue;
            };
            let traits = match bound {
                CtValue::Trait(traits) => traits.clone(),
                CtValue::Expr(expr) => match expr.meta().as_value().map(Ty::kind) {
                    Some(TyKind::AnyTrait(traits)) => traits.clone(),
                    _ => continue,
                },
                _ => continue,
            };
            let types: Vec<&Ty> = match argument {
                TyArg::Ty(ty) => vec![ty],
                TyArg::Val(CtValue::Tuple(elements)) => elements
                    .iter()
                    .filter_map(|element| match element {
                        CtValue::Type(ty) => Some(ty),
                        _ => None,
                    })
                    .collect(),
                TyArg::Val(_) | TyArg::Origin(_) => Vec::new(),
            };
            if let Some(ty) = types
                .into_iter()
                .find(|ty| !traits.iter().all(|required| self.conforms_to(ty, required)))
            {
                return Err(TypeError::ParamValueType {
                    callee: generic_name.to_string(),
                    param: name.trim_start_matches('*').to_string(),
                    expected: traits.join(" & "),
                    found: format!("AnyStruct[{ty}]"),
                });
            }
        }
        Ok(())
    }

    /// The selected `@implicit` constructor for a conversion, when one
    /// applies. The second component is `Some(loan mutability)` when that
    /// constructor borrows its argument through a `ref [origin]` parameter (a
    /// view construction): the caller records the source borrow so the
    /// conversion result's origin refines to the source expression's place
    /// (temporary-origin inference).
    fn implicit_conversion_target(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<(String, Option<bool>)>, TypeError> {
        Ok(self
            .implicit_conversion_constructor(from, to)?
            .map(|selected| (selected.target, selected.source_borrow)))
    }

    /// Whether storing a `found` value where `expected` is declared goes
    /// through an implicit converting constructor that borrows its source
    /// (a view construction such as `var s: Span[Int, _] = xs`). The value
    /// consumed at that storage boundary is the conversion temporary, so the
    /// source place is not implicitly copied.
    pub(super) fn storage_conversion_borrows_source(
        &self,
        found: &Ty,
        expected: Option<&Ty>,
    ) -> bool {
        let Some(expected) = expected else {
            return false;
        };
        if self.value_coerces(found, expected) {
            return false;
        }
        matches!(
            self.implicit_conversion_constructor(found, expected),
            Ok(Some(SelectedConversion {
                consumes_source: false,
                ..
            }))
        )
    }

    /// The selected implicit converting constructor from `from` to `to`.
    fn implicit_conversion_constructor(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<SelectedConversion>, TypeError> {
        // The builtin `SIMD` declares upstream's `@implicit
        // SIMD.__init__(Scalar[dtype])` splat by this reserved target.
        if mojito_types::types::scalar_splats_into(from, to) {
            return Ok(Some(SelectedConversion {
                target: mojito_symbol::symbol::SCALAR_SPLAT_CONVERSION.to_string(),
                source_borrow: None,
                consumes_source: false,
                error: None,
            }));
        }
        // And upstream's `@implicit SIMD.__init__(SIMDLength)` by this one.
        if mojito_types::types::is_simd_length(from) && matches!(to.kind(), TyKind::Simd { .. }) {
            return Ok(Some(SelectedConversion {
                target: mojito_symbol::symbol::SIMD_LENGTH_CONVERSION.to_string(),
                source_borrow: None,
                consumes_source: false,
                error: None,
            }));
        }
        let TyKind::Struct(name, args) = to.kind() else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        // The declared binders form the argument prefix; an origin tail may
        // follow it.
        if args.len() < info.decls.len() {
            return Ok(None);
        }
        let subst = struct_subst(&info.decls, args);
        let Some(constructors) = info.methods.get("__init__") else {
            return Ok(None);
        };
        let matches = constructors
            .iter()
            .filter(|sig| {
                sig.implicit
                    && sig.params.len() == 1
                    && coerces(from, &substitute(&sig.params[0], &subst))
            })
            .collect::<Vec<_>>();
        // Several converting constructors may accept the source; one whose
        // parameter is exactly the source type wins, and failing that one
        // whose parameter is the literal's default type, as in current Mojo
        // (a literal selects an `IntLiteral` constructor over an `Int` one).
        let exactly = |source: &Ty| {
            matches
                .iter()
                .filter(|sig| substitute(&sig.params[0], &subst) == *source)
                .copied()
                .collect::<Vec<_>>()
        };
        let matches = [
            exactly(from),
            exactly(&mojito_types::types::default_literal(from)),
        ]
        .into_iter()
        .find(|exact| exact.len() == 1)
        .unwrap_or(matches);
        match matches.as_slice() {
            [] => Ok(None),
            [sig] => {
                let target = constructor_symbol(
                    name,
                    "__init__",
                    constructors.len() > 1,
                    sig,
                    self.self_instance_ty(name).as_ref(),
                );
                let source_borrow = sig
                    .ref_params
                    .first()
                    .and_then(|signature| signature.as_ref())
                    .map(|signature| {
                        signature.mutability == mojito_types::origin::SigMutability::Mutable
                    });
                let consumes_source = matches!(
                    sig.conventions.first().copied().flatten(),
                    Some(
                        mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                );
                let error = sig
                    .raises
                    .then(|| sig.error.clone().unwrap_or_else(Ty::error));
                Ok(Some(SelectedConversion {
                    target,
                    source_borrow,
                    consumes_source,
                    error,
                }))
            }
            _ => Err(TypeError::BadCall {
                func: name.clone(),
                reason: format!("ambiguous implicit conversion from '{from}' to '{to}'"),
            }),
        }
    }

    /// Record only a constructor-based implicit conversion (the `@implicit`
    /// single-parameter constructor), never a plain value coercion. The
    /// construction/storage checks use this so a failure of the stricter
    /// storage rule cannot be masked by a permissive value coercion (for
    /// example a capturing closure erasing into a plain `def` field).
    fn record_constructor_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        let Some(SelectedConversion {
            target,
            source_borrow,
            error,
            ..
        }) = self.implicit_conversion_constructor(from, to)?
        else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, to, source_borrow, error)?;
        Ok(true)
    }

    /// Install one selected converting constructor plus, for a `ref`-parameter
    /// (view) constructor, the source-borrow fact that refines the conversion
    /// temporary's origin to the source expression's place. A temporary
    /// source has no place to lend: it materializes as an anonymous owned
    /// binding — the hidden slot an explicit `View(temp)` construction gets —
    /// so the view borrows real frame storage and the temporary lives as long
    /// as its borrower. A raising constructor makes the conversion site a
    /// raising call.
    fn record_selected_conversion(
        &self,
        expression: &Expr,
        target: String,
        to: &Ty,
        source_borrow: Option<bool>,
        error: Option<Ty>,
    ) -> Result<(), TypeError> {
        let span = expression.source_span();
        if let Some(error) = error {
            self.record_at(span.clone()).conversion_raises = Some(error.clone());
            self.require_error(
                format!("implicit conversion through raising '{target}'"),
                error,
            )?;
        }
        if let Some(mutable) = source_borrow {
            self.record_at(span.clone()).conversion_source_borrow = Some(mutable);
            if self.origin_place(expression).is_err()
                && self.infer_reference_value(expression).is_none()
            {
                self.materialize_borrow_owner(expression, mutable)?;
            }
        }
        // A constructor restates its struct's parameters, so the conversion
        // passes the converted-to type's binder prefix as any construction
        // does.
        if let TyKind::Struct(name, arguments) = to.kind()
            && mojito_symbol::symbol::lifecycle_constructor(&target)
                .is_some_and(|(_, method)| method == "__init__")
            && let Some(info) = self.structs.get(name)
            && !info.decls.is_empty()
        {
            self.record_at(span.clone()).conversion_instantiation =
                Some(arguments[..info.decls.len().min(arguments.len())].to_vec());
        }
        self.record_at(span.clone()).implicit_conversion_type = Some(to.clone());
        self.record_at(span).implicit_conversion = Some(target);
        Ok(())
    }

    fn record_implicit_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        if let TyKind::Overload(candidates) = from.kind() {
            let matches: Vec<_> = candidates
                .iter()
                .filter(|candidate| self.value_coerces(candidate, to))
                .filter_map(|candidate| {
                    let ExprKind::Identifier(name) = &expression.kind else {
                        return None;
                    };
                    callable_lowered_name(name, candidate).map(|target| (candidate, target))
                })
                .collect();
            return match matches.as_slice() {
                [(_, target)] => {
                    self.record_at(expression.source_span()).overload_target = Some(target.clone());
                    Ok(true)
                }
                [] => Ok(false),
                _ => Err(TypeError::BadCall {
                    func: "overloaded callable value".to_string(),
                    reason: format!("multiple overloads fit expected type '{to}'"),
                }),
            };
        }
        if let TyKind::Simd {
            dtype,
            width: SimdWidth::Known(1),
        } = to.kind()
            && splats_to(from, dtype)
        {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        if self.value_coerces(from, to) {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        // Upstream's `@implicit Bool.__init__(value: Scalar[DType.bool])`:
        // a scalar comparison stands where a `Bool` is wanted.
        if *to == TyKind::Bool && scalar_simd_dtype(from) == Some(Dtype::Bool) {
            self.record_selected_conversion(expression, "Bool".to_string(), to, None, None)?;
            return Ok(true);
        }
        let Some(SelectedConversion {
            target,
            source_borrow,
            error,
            ..
        }) = self.implicit_conversion_constructor(from, to)?
        else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, to, source_borrow, error)?;
        Ok(true)
    }

    /// Retain every exact-literal boundary selected by contextual typing.  The
    /// recursion matters for aggregates: `(1, 2.0)` materialized as
    /// `Tuple[Int, Float64]` has two scalar boundaries, not one tuple cast.
    /// A closure's storage accepted as a run-time callable is converted to
    /// its closure the same way
    /// ([`mojito_checked::checked::SemanticAdjustment::ClosureCallable`]).
    fn record_literal_materializations(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<(), TypeError> {
        if closure_converts_to_callable(from, to) {
            self.record_at(expression.source_span()).closure_callable = true;
            return Ok(());
        }
        // Upstream's implicit `Bool` to `SIMD[DType.bool, n]` constructor,
        // which asserts one lane.
        if *from == TyKind::Bool
            && let TyKind::Simd { width, .. } = to.kind()
        {
            return self.record_instance_assertion(
                &expression.source_span(),
                width,
                mojito_types::types::BOOL_MASK_CONVERSION_MESSAGE,
            );
        }
        let scalar_boundary = matches!(from.kind(), TyKind::IntLiteral | TyKind::FloatLiteral)
            && (matches!(to.kind(), TyKind::INT | TyKind::UINT | TyKind::FLOAT64)
                || matches!(to.kind(), TyKind::Simd { width, .. } if !width.is_inferred()));
        if scalar_boundary {
            if let Some(value) = self.exact_literal_value(expression)
                && !self.literal_value_fits_target(&value, to)
            {
                return Err(TypeError::TypeMismatch {
                    expected: to.to_string(),
                    found: from.to_string(),
                    context: format!("numeric literal materialization of '{value}'"),
                });
            }
            // A literal parameter value read (`A.K` of `comptime K = 7`)
            // materializes as its constant.
            let mut record = self.record_at(expression.source_span());
            if let Some(mojito_checked::checked::SemanticAdjustment::ParamValue { value, .. }) =
                record.adjustment.as_deref_mut()
                && let Some(materialized) = value
                    .as_constant()
                    .and_then(|constant| constant.clone().materialize_as(to))
                    .and_then(|constant| self.param_context.constant(constant).ok())
            {
                *value = materialized;
                return Ok(());
            }
            record.adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(to.clone()),
            ));
            return Ok(());
        }

        // A tuple literal's elements materialize to the element types the
        // `Tuple` it initializes declares.
        if let (Some(actual), Some(expected)) = (tuple_elements(from), tuple_elements(to))
            && actual.len() == expected.len()
        {
            let values = match &expression.kind {
                ExprKind::TupleLit(values) => Some(values.as_slice()),
                ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                _ => None,
            };
            if let Some(values) = values {
                for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                    self.record_literal_materializations(value, actual, expected)?;
                }
            }
            return Ok(());
        }

        match (from.kind(), to.kind()) {
            (TyKind::Tuple(actual), TyKind::Tuple(expected)) if actual.len() == expected.len() => {
                let values = match &expression.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            (TyKind::ComptimeList(actual), TyKind::ComptimeList(expected)) => {
                let values = match &expression.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn exact_literal_value(&self, expression: &Expr) -> Option<CtValue> {
        match self.eval_associated_ct(expression).ok()? {
            value @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)) => Some(value),
            _ => None,
        }
    }

    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    fn literal_value_fits_target(&self, value: &CtValue, target: &Ty) -> bool {
        // A literal's fit into a symbolic lane is the instantiation's to check.
        match (value, target.kind()) {
            (CtValue::IntLiteral(_), TyKind::Simd { dtype, width }) if !width.is_inferred() => {
                dtype.licenses(int_literal_materializes_to_dtype)
            }
            (CtValue::FloatLiteral(_), TyKind::Simd { dtype, width }) if !width.is_inferred() => {
                dtype.licenses(Dtype::is_float)
            }
            (value, TyKind::INT | TyKind::UINT | TyKind::FLOAT64) => {
                value.clone().materialize_as(target).is_some()
            }
            _ => false,
        }
    }

    /// Record each result's convention, as upstream's signature decides
    /// whether a callee returns through a synthesized `ByRefResult`
    /// argument: a result whose declared type (the selected callee's, an
    /// initializer's struct at its own parameters, else the node's own) is
    /// memory-only is returned through memory
    /// ([`mojito_checked::checked::SemanticAdjustment::IndirectResult`]);
    /// so is an implicit conversion's when its type is. A read argument
    /// whose type is not memory-only is passed by value
    /// ([`mojito_checked::checked::SemanticAdjustment::RegisterPassableRead`]).
    fn record_result_conventions(&self) {
        let memory_only = |ty: &Ty| self.memory_only(ty);
        // An initializer's declared result is its `out self`, the struct at
        // its own parameters, so a conditional `RegisterPassable`
        // conformance is not proven there.
        let constructed = |target: &str| {
            target
                .split_once(".__init__")
                .filter(|(_, rest)| rest.is_empty() || rest.starts_with('$'))
                .and_then(|(name, _)| self.self_instance_ty(name))
        };
        let judged: Vec<(SourceSpan, bool, bool, bool)> = {
            let records = self.node_facts.borrow();
            records
                .iter()
                .map(|(span, facts)| {
                    let by_value = facts.borrowed_read_call_place
                        && facts.ty.as_ref().is_some_and(|ty| !memory_only(ty));
                    let indirect = facts
                        .declared_result
                        .clone()
                        .or_else(|| facts.overload_target.as_deref().and_then(constructed))
                        .or_else(|| facts.ty.clone())
                        .is_some_and(|ty| memory_only(&ty));
                    let converted = facts
                        .implicit_conversion
                        .as_deref()
                        .and_then(constructed)
                        .or_else(|| facts.implicit_conversion_type.clone())
                        .is_some_and(|ty| memory_only(&ty));
                    (span.clone(), indirect, converted, by_value)
                })
                .filter(|(_, indirect, converted, by_value)| *indirect || *converted || *by_value)
                .collect()
        };
        let mut records = self.node_facts.borrow_mut();
        for (span, indirect, converted, by_value) in judged {
            if let Some(facts) = records.get_mut(&span) {
                facts.indirect_result = indirect;
                facts.conversion_indirect_result = converted;
                facts.register_passable_read = by_value;
            }
        }
    }

    /// An ordinary value use of a reference-returning expression owns an
    /// independent value: MIR lowers it to `ReadRef` followed by `CopyValue`.
    /// Reference-bearing contexts record a `reference_value_use` fact and
    /// retain the handle instead. Validate the remaining reads after the whole
    /// tree has been checked so a later enclosing context can mark a nested
    /// result as a retained handle before this distinction is enforced.
    fn check_reference_result_reads(&self) -> Result<(), TypeError> {
        let read_referents: Vec<Ty> = {
            let records = self.node_facts.borrow();
            let copyable_reads = self.copyable_reference_result_reads.borrow();
            let discarded = self.discarded_reference_results.borrow();
            let borrowed_receivers = self.borrowed_reference_receivers.borrow();
            records
                .iter()
                .filter_map(|(span, facts)| match facts.adjustment.as_deref() {
                    Some(mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                        reference,
                    }) if facts.reference_value_use.is_none()
                        && !facts.borrowed_read_call_place
                        && !copyable_reads.contains(span)
                        && !discarded.contains(span)
                        && !borrowed_receivers.contains(span) =>
                    {
                        Some(reference.referent.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        for referent in read_referents {
            if self.is_implicitly_copyable(&referent) {
                continue;
            }
            let context = "ordinary value read through a reference result".to_string();
            if !self.is_copyable(&referent) {
                return Err(TypeError::NonCopyable {
                    ty: referent.to_string(),
                    context,
                });
            }
            return Err(TypeError::ImplicitCopy {
                ty: referent.to_string(),
                context,
                transferable: false,
                copyable: true,
            });
        }
        Ok(())
    }

    /// Compiler-generated keyword collectors use the bundled self-hosted
    /// `StringDict`. Its current `List`/`DictEntry` implementation is copy-based,
    /// so accept only element types that it can store without duplicating a
    /// linear value. Reference-bearing values also wait for collector origin
    /// metadata instead of silently losing their loans at the call boundary.
    fn kwargs_collector_ty(&self, element: Ty, context: &str) -> Result<Ty, TypeError> {
        if !self.is_copyable(&element) {
            return Err(TypeError::TraitNotSatisfied {
                param: "V".to_string(),
                ty: element.to_string(),
                trait_name: "Copyable".to_string(),
                reason: Some(format!(
                    "{context} is materialized as StringDict[V], whose current storage is copy-based"
                )),
            });
        }
        if self.type_contains_reference(&element) {
            return Err(TypeError::Unsupported(format!(
                "{context} cannot contain references until keyword collectors carry origin metadata"
            )));
        }
        Ok(TyKind::Struct("StringDict".to_string(), vec![TyArg::Ty(element)]).into())
    }

    /// Require `expr` to be usable as a condition (`if`/`while`/ternary/
    /// comprehension filter), recording a truthiness mark for MIR to convert
    /// where [`Self::condition_truthiness`] asks for one. The conversion
    /// reads its operand as upstream's `Bool.__init__[T: Boolable](value: T)`
    /// does: a named struct, parameter, or pack-element value, or an element an accessor
    /// returns by reference, is borrowed where it lies and a temporary is
    /// read in its call-result slot, so nothing is copied.
    fn expect_bool(&self, expr: &Expr, context: &str) -> Result<(), TypeError> {
        let ty = self.infer(expr)?;
        if self.condition_truthiness(&ty, context)? {
            self.record_at(expr.source_span()).truthiness_condition = true;
            if !is_numeric(&ty) && !is_scalar_simd(&ty) {
                let element = matches!(expr.kind, ExprKind::Index { .. })
                    && self.infer_reference_value(expr).is_some();
                if element {
                    self.record_at(expr.source_span()).borrowed_read_call_place = true;
                }
                self.borrow_nominal_place_argument(expr, &ty);
                self.borrow_parameter_place_argument(expr, &ty);
                self.record_read_temporary_argument(expr);
            }
        }
        Ok(())
    }

    /// Whether a condition of type `ty` converts through `Bool(x)`: `false`
    /// for a `Bool`, `true` for a numeric scalar (`if n:` over an `Int`, a
    /// `Float64`, or a width-1 lane such as `if a == b` over `UInt64`s), a
    /// `Boolable` type parameter or pack element, or a struct whose
    /// `__bool__` supplies its truth value (`if collection:`), and an error
    /// for anything else.
    fn condition_truthiness(&self, ty: &Ty, context: &str) -> Result<bool, TypeError> {
        let bounded = self.opaque_element(ty).unwrap_or_else(|| ty.clone());
        if *ty == TyKind::Bool {
            Ok(false)
        } else if is_numeric(ty) || is_scalar_simd(ty) || self.param_has_bound(&bounded, "Boolable")
        {
            Ok(true)
        } else if let Some(result) = self.struct_dunder(ty, "__bool__", &[]) {
            require_dunder_ret(result?, &Ty::bool(), "__bool__")?;
            Ok(true)
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Bool".to_string(),
                found: ty.to_string(),
                context: context.to_string(),
            })
        }
    }
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "TODO: group the flags into a state enum"
)]
struct MethodSig {
    decls: Vec<ParamDecl>,
    availability: Vec<GenericConstraint>,
    has_self: bool,
    /// Regular parameters only; variadic element type is stored separately.
    params: Vec<Ty>,
    names: Vec<String>,
    required: Vec<bool>,
    /// Per regular parameter, its declared default attribute. A conforming
    /// method may declare its own: a call through the bound leaving one out
    /// runs the requirement's, which the check binds at the call where some
    /// witness differs (`bound_defaults.rs`).
    defaults: Vec<Option<SignatureDefault>>,
    variadic: Option<Ty>,
    variadic_index: Option<usize>,
    /// The positional collector's convention (`var *args`), which a pack
    /// forwarded into it must match.
    variadic_convention: Option<ArgConvention>,
    /// The positional collector's parameter name.
    variadic_name: Option<String>,
    kw_variadic: Option<Ty>,
    kw_variadic_index: Option<usize>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    conventions: Vec<Option<ArgConvention>>,
    ret: Ty,
    raises: bool,
    error: Option<Ty>,
    /// Receiver convention. `None` means plain read-only `self`; explicit
    /// conventions (`mut`, `var`, `ref`, ...) are preserved so trait
    /// requirements can compare them exactly. Today only `mut self` changes call
    /// checking behavior.
    self_convention: Option<mojito_ast::ast::ArgConvention>,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<mojito_types::origin::RefSig>,
    /// The owned-interior tags a struct-typed view return projects on the
    /// receiver's origin (`-> StringSpan[origin_of(self)._get_owned_interior["bytes"]]`
    /// is `["bytes"]`), innermost first. Empty for every other return.
    view_return_interior: Vec<String>,
    /// The origin slots a struct-typed view return binds to the callee's
    /// places (`-> P[origin_of(self.xs)]`); see [`ViewReturnOrigin`].
    view_return: Vec<ViewReturnOrigin>,
    implicit: bool,
    /// Origin parameters the body writes through via a parametric-mut ref
    /// field subscript (`self.field[i] = v`). The write is legal only for
    /// instantiations binding the origin to a mutable source, judged at each
    /// call site against the receiver's concrete origin arguments.
    parametric_origin_writes: Vec<mojito_types::origin::OriginParamId>,
    /// Per regular parameter, the enclosing struct's origin binder its `ref`
    /// clause names (`ref [Self.origin] list`), so a constructor call can
    /// check an explicitly applied origin against the argument filling that
    /// slot. `None` for every other parameter.
    origin_binders: Vec<Option<mojito_types::origin::PointerOrigin>>,
    /// A synthesized trait default ([`mojito_ast::ast::MethodProvenance::SynthesizedDefault`]):
    /// the elaborator mints no per-instance clone of it.
    synthesized_default: bool,
    /// How a call's exclusivity check counts the origins the argument
    /// types carry.
    nested_origins: NestedOrigins,
    /// The signature qualifier of the overload this method is
    /// (`$ov$T$Copyable$Int`), when it is one of several same-named
    /// overloads: what a call selecting that overload records. `None` for
    /// every other method.
    overload: Option<String>,
}

impl MethodSig {
    fn intrinsic(params: Vec<Ty>, ret: Ty) -> Self {
        let len = params.len();
        Self {
            decls: Vec::new(),
            availability: Vec::new(),
            has_self: true,
            params,
            names: (0..len).map(|i| format!("arg{i}")).collect(),
            required: vec![true; len],
            defaults: vec![None; len],
            variadic: None,
            variadic_index: None,
            variadic_convention: None,
            variadic_name: None,
            kw_variadic: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            conventions: vec![None; len],
            ret,
            raises: false,
            error: None,
            self_convention: None,
            ref_params: vec![None; len],
            ref_return: None,
            view_return_interior: Vec::new(),
            view_return: Vec::new(),
            implicit: false,
            parametric_origin_writes: Vec::new(),
            origin_binders: vec![None; len],
            synthesized_default: false,
            nested_origins: NestedOrigins::AsDeclared,
            overload: None,
        }
    }
}

/// A regular parameter's declared default, as a checked signature holds it:
/// the attribute its lowered declaration holds, for an own method, a trait
/// requirement, and an imported method alike.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum SignatureDefault {
    /// A literal, as its lowered declaration's constant before any
    /// conversion to the parameter type.
    Constant(mojito_checked::checked::CheckedConst),
    /// A compile-time expression over the method's own value parameters
    /// (`factor: Int = n * TWO - m`), module constants folded at the
    /// declaration; only a trait requirement holds one.
    Parameter(ParamExpr),
    /// Any other default, which the lowered declaration runs as code.
    Evaluated,
}

impl SignatureDefault {
    /// The attribute of a lowered declaration's default: its literal, a
    /// conversion of a literal being the parameter type's.
    fn of_declared(declared: &mojito_checked::checked::CheckedConst) -> Self {
        match declared {
            mojito_checked::checked::CheckedConst::Evaluate { .. } => Self::Evaluated,
            declared => Self::Constant(declared.unconverted().clone()),
        }
    }

    /// Whether the default is the literal `literal`.
    fn is_literal(&self, literal: &mojito_checked::checked::CheckedConst) -> bool {
        matches!(self, Self::Constant(constant) if constant == literal)
    }
}

/// The checked signature of a struct, kept in the checker's registry.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct StructInfo {
    /// Compile-time parameters (type and value); empty for a non-generic struct.
    decls: Vec<ParamDecl>,
    /// Raw source parameter list as declared, including Origin/OriginSet
    /// parameters and their `mut=` Bool binders, which `decls` erases.
    /// Direct applications consult it to accept, validate, and erase explicit
    /// origin arguments (`EntryIter[K, V, some_origin]`).
    source_params: Vec<mojito_ast::ast::TypeParam>,
    /// Traits this struct declares conformance to (verified at definition).
    conforms: Vec<String>,
    callable_conformance: Option<Ty>,
    /// Exact lowered `__call__` method selected by the declared callable
    /// conformance. This is distinct from the callable contract: overloaded
    /// methods with the same arity require the signature-qualified target to
    /// survive into indirect-call MIR.
    callable_target: Option<String>,
    /// The witness table of each declared conformance, verified at
    /// definition; see [`mojito_checked::declaration::StructDeclaration::witness_tables`].
    witness_tables: Vec<mojito_checked::declaration::WitnessTable>,
    #[serde(serialize_with = "interface::sorted")]
    conformance_conditions: HashMap<String, Expr>,
    /// Declared fields, in order (drives the fieldwise constructor).
    fields: Vec<(String, Ty)>,
    /// The declared field names, available from shell registration on (the
    /// typed `fields` arrive later), so a `Self.<field>` in a compile-time
    /// position can be told from an unknown parameter.
    declared_field_names: Vec<String>,
    /// For each field whose SOURCE annotation applied origin-binder
    /// arguments (`var iter: EntryIter[Self.o2]`), the (callee origin-param
    /// index, enclosing origin-param index) pairs the application bound —
    /// both in their declaration lists' full-index (`OriginParamId`) domain.
    /// Origin arguments are erased from checked identity, so this is the
    /// surviving record delegated-call origin clauses resolve binder
    /// correspondences through.
    #[serde(serialize_with = "interface::sorted")]
    field_origin_arguments: HashMap<String, Vec<(u32, u32)>>,
    /// Associated compile-time facts declared by `comptime NAME = ...` in the
    /// struct body. These live on the type, not on runtime instances.
    #[serde(serialize_with = "interface::sorted")]
    associated: HashMap<String, CtValue>,
    /// The declared type of each `associated` value whose annotation names
    /// one (`comptime _types: KGENParamListType[AnyType] = ...`), or the
    /// raw-list type of an unannotated list body (`comptime _cases =
    /// Self.Ts.values`), at the struct's own binders: a read of the member
    /// has this type.
    #[serde(serialize_with = "interface::sorted")]
    associated_value_types: HashMap<String, Ty>,
    /// Availability constraints for monomorphic associated members, one entry
    /// per trailing `where` clause. They are evaluated against the enclosing
    /// struct arguments at projection time.
    #[serde(serialize_with = "interface::sorted")]
    associated_constraints: HashMap<String, Vec<GenericConstraint>>,
    /// Parameterized associated types declared by `comptime NAME[params] = body`.
    /// Unlike `associated`, the body cannot be evaluated eagerly (it references
    /// the member's own parameters); it is lowered once to a symbolic template
    /// and substituted per application. See [`ParameterizedMember`].
    #[serde(serialize_with = "interface::sorted")]
    parameterized_associated: HashMap<String, ParameterizedMember>,
    #[serde(serialize_with = "interface::sorted")]
    methods: HashMap<String, Vec<MethodSig>>,
    fieldwise_init: bool,
    explicit_destroy_message: Option<String>,
    #[serde(serialize_with = "interface::sorted")]
    explicit_destructors: HashMap<String, bool>,
}

impl StructInfo {
    /// Whether the typed `fields` have arrived: a struct's members resolve
    /// before its fields, so a reflection query read while they do has no
    /// field table to answer from yet.
    const fn fields_resolved(&self) -> bool {
        self.fields.len() == self.declared_field_names.len()
    }

    /// The struct's origin tail slots — every explicit (non-infer-only)
    /// `Origin`/`OriginSet` source parameter with its full-index binder id, in
    /// source order. A struct instance's `TyKind::Struct` arguments carry one
    /// `TyArg::Origin` per slot after the `decls`-aligned prefix.
    fn origin_slots(
        &self,
    ) -> Vec<(
        mojito_types::origin::OriginParamId,
        &mojito_ast::ast::TypeParam,
    )> {
        struct_origin_slots(&self.source_params)
    }

    /// The arguments of the struct's own `Self` type: each declared parameter
    /// as itself, then each origin slot as its own binder.
    fn self_arguments(&self) -> Vec<TyArg> {
        self_struct_arguments(&self.decls, &self.source_params)
    }

    /// The origin tail of an instance's arguments (empty for an erased
    /// spelling that carries none).
    fn origin_tail<'a>(&self, arguments: &'a [TyArg]) -> &'a [TyArg] {
        arguments.get(self.decls.len()..).unwrap_or_default()
    }

    /// The binder-to-origin map an instance's tail binds, for substituting
    /// the struct's own origin binders (`Self.o`) in member signatures.
    fn tail_origin_bindings(
        &self,
        arguments: &[TyArg],
    ) -> HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin> {
        self.origin_slots()
            .into_iter()
            .zip(self.origin_tail(arguments))
            .filter_map(|((id, _), argument)| match argument {
                TyArg::Origin(origin) => Some((id, origin.clone())),
                TyArg::Ty(_) | TyArg::Val(_) => None,
            })
            .collect()
    }
}

/// The explicit-destruction obligations a checked program's own structs
/// carry: one entry per struct whose self-type is not `Deinitable`.
fn explicit_destroy_types(
    checker: &Checker,
) -> HashMap<String, mojito_checked::checked::ExplicitDestroyInfo> {
    checker
        .structs
        .own()
        .iter()
        .filter_map(|(name, info)| {
            let self_ty = Ty::new(TyKind::Struct(name.clone(), params_as_args(&info.decls)));
            (!checker.is_deinitable(&self_ty)).then(|| {
                (
                    name.clone(),
                    mojito_checked::checked::ExplicitDestroyInfo {
                        message: info.explicit_destroy_message.clone().unwrap_or_else(|| {
                            mojito_checked::checked::ExplicitDestroyInfo::default_message(name)
                        }),
                        declared: info.explicit_destroy_message.is_some(),
                        destructors: info.explicit_destructors.clone(),
                        fields: info
                            .fields
                            .iter()
                            .filter_map(|(field, ty)| match ty.kind() {
                                TyKind::Struct(field_ty, _) if !checker.is_deinitable(ty) => {
                                    Some((field.clone(), field_ty.clone()))
                                }
                                _ => None,
                            })
                            .collect(),
                    },
                )
            })
        })
        .collect()
}

/// Run the explicit-destruction analysis over `program` with the facts
/// `checker` recorded for it.
fn run_explicit_destroy(
    checker: &Checker,
    program: &[Stmt],
    struct_types: &HashMap<String, mojito_checked::checked::ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let node_facts = checker.node_facts.borrow();
    let deletability = checker.explicit_destroy_deletability.borrow();
    // An installed struct's entry is the package build's.
    let types = checker.explicit_destroy_table(struct_types.clone());
    // A non-`Deinitable` temporary in a position that takes no ownership of
    // it is one the caller owns and cannot implicitly destroy; any other is
    // owned in flight until the operation that consumes it.
    let unconsumed = checker.unconsumed_temporaries.borrow();
    let (linear_temporaries, owned_temporaries): (HashMap<_, _>, HashMap<_, _>) = checker
        .linear_temporaries
        .borrow()
        .iter()
        .map(|(span, linear)| (span.clone(), linear.clone()))
        .partition(|(span, _)| unconsumed.contains(span));
    let members: Vec<Stmt> = checker
        .checked_trait_members(program)
        .into_iter()
        .map(|member| member.declaration)
        .collect();
    crate::explicit_destroy::check(
        program,
        &members,
        &node_facts,
        &deletability,
        &types,
        crate::explicit_destroy::SpanFacts {
            lent: node_facts
                .iter()
                .filter(|(_, facts)| facts.borrowed_read_call_place)
                .map(|(span, _)| span.clone())
                .collect(),
            linear_temporaries,
            owned_temporaries,
            generators: checker
                .comprehension_iterables
                .borrow()
                .values()
                .flatten()
                .filter_map(|span| {
                    deletability
                        .bindings
                        .get(span)
                        .map(|linear| (span.clone(), linear.clone()))
                })
                .collect(),
            raise_points: node_facts
                .iter()
                .filter(|(_, facts)| {
                    facts.effects.as_ref().is_some_and(|e| e.raises.is_some())
                        || facts.conversion_raises.is_some()
                })
                .map(|(span, _)| span.clone())
                .chain(checker.raising_iterations.borrow().iter().cloned())
                .collect(),
        },
    )
}

/// Whether a struct parameter is an origin slot (`o: Origin[mut=m]`, an
/// `OriginSet`), which `classify_params` erases from the declared binders.
fn is_origin_type_param(param: &mojito_ast::ast::TypeParam) -> bool {
    matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
}

/// See [`StructInfo::origin_slots`].
fn struct_origin_slots(
    source_params: &[mojito_ast::ast::TypeParam],
) -> Vec<(
    mojito_types::origin::OriginParamId,
    &mojito_ast::ast::TypeParam,
)> {
    source_params
        .iter()
        .enumerate()
        .filter(|(_, param)| !param.infer_only && is_origin_type_param(param))
        .map(|(index, param)| (mojito_types::origin::OriginParamId(index as u32), param))
        .collect()
}

/// See [`StructInfo::self_arguments`].
fn self_struct_arguments(
    decls: &[ParamDecl],
    source_params: &[mojito_ast::ast::TypeParam],
) -> Vec<TyArg> {
    params_as_args(decls)
        .into_iter()
        .chain(
            struct_origin_slots(source_params)
                .into_iter()
                .map(|(id, _)| TyArg::Origin(mojito_types::origin::Origin::Param(id))),
        )
        .collect()
}

/// A generic top-level alias (`comptime Alias[params] = Type` or a Bool
/// proposition). The parameters are classified `ParamDecl`s — trailing `where`
/// clauses attach to the last one — so each type-bodied application validates
/// arity, bounds, defaults, and declaration constraints through the same
/// `resolve_use_params` contract as a struct application. A type body is
/// lowered once to a symbolic template (`TyKind::Param` / `CtValue::Expr`) and
/// substituted per application; a Bool body is lowered once to a symbolic
/// [`GenericConstraint`] and inlined into the consuming proposition per
/// application. Aliases lower sequentially at declaration, so a body may
/// reference only already-declared names: self-reference fails as an unknown
/// type, and an alias expanding an earlier alias bakes the expansion into its
/// template. Origin parameters are rejected at declaration, so no origin
/// bindings exist.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ComptimeAlias {
    decls: Vec<ParamDecl>,
    body: AliasBody,
    /// The declaration's parameter expression as written: the value a read
    /// through a zero-parameter type alias is spelled over
    /// ([`Checker::infer_type_alias_read`]).
    value: Expr,
    /// A predicate alias's body as the `Bool` value over its own binders:
    /// the body of the generator the alias read unapplied is
    /// ([`Checker::alias_generator`]). `None` for any other alias.
    predicate_value: Option<ParamExpr>,
}

/// The `@implicit` converting constructor selected for one conversion.
struct SelectedConversion {
    /// The constructor's lowered symbol.
    target: String,
    /// `Some(loan mutability)` when the constructor borrows its argument
    /// through a `ref [origin]` parameter (a view construction).
    source_borrow: Option<bool>,
    /// Whether the constructor consumes its source (a `var`/`deinit`
    /// parameter).
    consumes_source: bool,
    /// The error type the constructor raises, if it raises.
    error: Option<Ty>,
}

/// The lowered body of a [`ComptimeAlias`]: a symbolic type template, a
/// symbolic Bool proposition (a predicate alias, usable exactly where
/// `conforms_to`/`IsTrivially*` propositions are — never in type positions),
/// or a compile-time value over the alias's binders, which every application
/// binds by substitution and the elaborator folds per instance.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum AliasBody {
    Type(Ty),
    Predicate(Box<GenericConstraint>),
    Value(ParamExpr),
}

/// The body of a [`ParameterizedMember`]: upstream's generator over the
/// member's own parameters, a type or a typed value.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum MemberTemplate {
    /// A type body over the member's classified binders (`comptime
    /// Buf[n: Int] = Fixed[n]`), which an application binds by identity.
    Type { decls: Vec<ParamDecl>, template: Ty },
    /// A value body (`comptime get[idx: Int]: Int = Self.values[idx]`) over
    /// the member's classified binders, which an application binds like a
    /// module alias's.
    Value {
        decls: Vec<ParamDecl>,
        body: ParamExpr,
    },
}

/// A parameterized `comptime` member a struct defines: a type
/// (`comptime Buf[n: Int] = Fixed[n]`) or a value (`comptime plus[k: Int] =
/// Self.n + k`). The body is lowered once with the member's own parameters
/// in scope, so the resulting `template` carries them symbolically
/// (`TyKind::Param`, `CtValue::Expr`, `Origin::Param`); concrete resolution
/// substitutes an application's arguments into it. The raw
/// `TypeParam`s are retained (rather than classified `ParamDecl`s) so the
/// argument-to-parameter binding can distinguish type, value, and origin kinds.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ParameterizedMember {
    /// The declaration that owns the member's own binders
    /// (`annotations::member_binder`): `Struct.member`.
    binder_owner: String,
    params: Vec<mojito_ast::ast::TypeParam>,
    template: MemberTemplate,
    /// Constraints (one per trailing `where` clause) evaluated against both
    /// enclosing struct arguments and the member application's explicit
    /// arguments.
    availability: Vec<GenericConstraint>,
    /// The index the member's parameters started at in `enclosing_type_params`
    /// while the template was lowered. An origin parameter at member position `k`
    /// therefore appears in the template as `Origin::Param(param_base + k)`, which
    /// concrete substitution uses to bind the application's origin argument.
    param_base: usize,
}

impl ParameterizedMember {
    /// The member as the checked declaration fact names it: its explicit
    /// parameters keyed exactly as `resolve_parameterized_member` binds an
    /// application's arguments. `None` for a value member, which is
    /// [`Self::checked_value_fact`].
    fn checked_fact(
        &self,
        name: &str,
    ) -> Option<mojito_checked::declaration::ParameterizedAssociatedType> {
        use mojito_checked::declaration::AssociatedParameter;

        let MemberTemplate::Type { template, .. } = &self.template else {
            return None;
        };
        let params = self
            .params
            .iter()
            .enumerate()
            .filter(|(_, param)| !param.infer_only)
            .map(|(index, param)| {
                if type_resolution::is_origin_parameter(param) {
                    AssociatedParameter::Origin(mojito_types::origin::OriginParamId(
                        (self.param_base + index) as u32,
                    ))
                } else if type_resolution::is_value_parameter(param) {
                    AssociatedParameter::Value(param.name.clone())
                } else {
                    AssociatedParameter::Type(member_binder(&self.binder_owner, index, &param.name))
                }
            })
            .collect();
        Some(mojito_checked::declaration::ParameterizedAssociatedType {
            name: name.to_string(),
            params,
            template: template.clone(),
        })
    }

    /// A value member as the checked declaration fact names it, which the
    /// elaborator answers a read through a bound from (`T.get[0]`); the
    /// check answers a read through the struct itself. `None` for a type
    /// member.
    fn checked_value_fact(
        &self,
        name: &str,
    ) -> Option<mojito_checked::declaration::ParameterizedAssociatedValue> {
        let MemberTemplate::Value { decls, body } = &self.template else {
            return None;
        };
        Some(mojito_checked::declaration::ParameterizedAssociatedValue {
            name: name.to_string(),
            params: decls.clone(),
            body: body.clone(),
        })
    }
}

/// A struct's checked associated compile-time members: the eagerly evaluated
/// monomorphic values with their declared types, and the parameterized members
/// lowered to their symbolic templates for later concrete substitution.
type StructAssociatedMembers = (
    HashMap<String, CtValue>,
    HashMap<String, Ty>,
    HashMap<String, Vec<GenericConstraint>>,
    HashMap<String, ParameterizedMember>,
);

/// The checked signature of a trait: required methods plus associated
/// compile-time facts. A method requirement's signature may mention
/// `TyKind::SelfType` (the conforming type).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TraitInfo {
    refines: Vec<String>,
    /// The symbol path of the module declaring the trait
    /// (`std.builtin.comparable`), which orders a composition's traits as
    /// upstream's `sortAndDeduplicateTraitSymbols` does. An installed
    /// trait's is the package directory's.
    #[serde(skip)]
    module: Vec<String>,
    #[serde(serialize_with = "interface::sorted")]
    methods: HashMap<String, Vec<MethodSig>>,
    #[serde(serialize_with = "interface::sorted")]
    comptime_members: HashMap<String, CtMemberReq>,
    /// Per-member declaration constraints, one entry per trailing `where`
    /// clause on the requirement.
    #[serde(serialize_with = "interface::sorted")]
    comptime_constraints: HashMap<String, Vec<GenericConstraint>>,
    /// The trait's own defaulted `comptime` requirements, in declaration
    /// order, each as the member a conformer that omits it declares.
    comptime_defaults: Vec<StructComptime>,
    /// The trait's own defaults, in declaration order; a refinement's are
    /// its parent's. Each names its requirement row in [`Self::methods`].
    defaults: Vec<trait_defaults::DefaultShape>,
    /// The trait's own requirements, in declaration order, each beside its
    /// spelled arity and its signature's index among [`Self::methods`]'
    /// overloads of its name, which merge its parents'. Its lowered
    /// declaration carries them, so a residue does not.
    #[serde(skip)]
    requirements: Vec<(String, usize, usize)>,
}

/// A concrete method candidate after receiver-type substitution and argument
/// scoring. Named fields keep overload resolution readable as it evolves.
/// One body's transfer-effect accumulation frame: the callable's key and
/// its ordered parameter owners, so accepted outliving stores abstract to
/// signature-relative effects.
struct TransferFrame {
    callable: String,
    param_owners: Vec<mojito_types::origin::OwnerId>,
    /// Whether each parameter's convention borrows caller storage
    /// (`mut`/`ref`) rather than owning a moved value.
    param_borrowed: Vec<bool>,
    self_owner: Option<mojito_types::origin::OwnerId>,
    /// Compile-time callable value parameters (decl names) in scope in this
    /// body — call-through recording keys on them.
    value_callables: Vec<String>,
    effects: Vec<mojito_checked::checked::TransferEffect>,
    /// Every effect recorded on this frame, in order, with the stored type
    /// of each latent one: an outward store of a symbolic value that may
    /// carry a loan once instantiated. A latent effect is not published; it
    /// is kept for an instance derived from this body, whose own check would
    /// record it at a loan-carrying argument.
    recorded: Vec<(mojito_checked::checked::TransferEffect, Option<Ty>)>,
    /// A latent store whose origin would escape: an instance at a
    /// loan-carrying argument rejects the store, so the body's facts are not
    /// captured for one to be derived from.
    latent_escapes: bool,
    call_throughs: Vec<mojito_checked::checked::CallThroughEffect>,
}

impl TransferFrame {
    /// Record `effect`, published unless `latent` names the stored type
    /// that makes it latent. An effect already recorded keeps its place in
    /// the order, and a latent one recorded again for real is published.
    fn record(&mut self, effect: mojito_checked::checked::TransferEffect, latent: Option<Ty>) {
        let published = latent.is_none();
        if let Some((_, kept_latent)) = self.recorded.iter_mut().find(|(kept, _)| *kept == effect) {
            if published && kept_latent.take().is_some() {
                self.effects.push(effect);
            }
            return;
        }
        if published {
            self.effects.push(effect.clone());
        }
        self.recorded.push((effect, latent));
    }
}

struct MethodCallResolution {
    conversion_score: usize,
    /// How many arguments with a lane the signature binds to a bare type
    /// parameter; see `select_method_overload`.
    simd_erasures: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
    variadic_element: Option<Ty>,
    /// The positional collector's declared convention.
    variadic_convention: Option<ArgConvention>,
    /// The positional collector's parameter name.
    variadic_name: Option<String>,
    keyword_element: Option<Ty>,
    conventions: Vec<Option<ArgConvention>>,
    self_convention: Option<ArgConvention>,
    return_type: Ty,
    result_adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
    raises: bool,
    error: Option<Ty>,
    mutates_receiver: bool,
    consumes_receiver: bool,
    lowered_name: Option<String>,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<mojito_types::origin::RefSig>,
    param_types: Vec<Ty>,
    param_decls: Vec<ParamDecl>,
    /// See [`MethodSig::parametric_origin_writes`].
    parametric_origin_writes: Vec<mojito_types::origin::OriginParamId>,
    /// The method's own resolved compile-time arguments, in declaration
    /// order, when the selected signature declares any.
    instantiation: Option<Vec<TyArg>>,
    /// The selected signature's runtime parameter names, in declaration order.
    parameter_names: Vec<String>,
    /// See [`MethodSig::view_return_interior`].
    view_return_interior: Vec<String>,
    /// See [`MethodSig::view_return`].
    view_return: Vec<ViewReturnOrigin>,
    /// The selected struct member's declared result type, with the struct's
    /// and the method's parameters abstract; `None` when the result has no
    /// separate declaration. Loans the result carries only through a type
    /// argument (`List[RefBox[o]].pop()`) are not a view of the receiver.
    declared_return: Option<Ty>,
    /// The selected signature's declared parameter types, with type
    /// parameters abstract: the origins a parameter *names*, whose slots fix
    /// their own mutability for the argument exclusivity rule.
    declared_params: Vec<Ty>,
    /// See [`MethodSig::nested_origins`].
    nested_origins: NestedOrigins,
    /// What each of the call's bracket arguments bound to.
    bracket_attributes: BracketAttributes,
    /// See [`mojito_checked::checked::CheckedCallContract::refined_bound`].
    refined_bound: Option<Vec<String>>,
}

impl MethodCallResolution {
    /// The positions the positional collector gathers and whether it owns
    /// them.
    fn gathered_arguments(&self) -> Gathered<'_> {
        Gathered::of_convention(self.variadic_convention, &self.positional_overflow)
    }
}

/// One origin slot of a struct-typed view return (`-> P[origin_of(xs)]`):
/// the returned struct's origin parameter and the contract its argument
/// lowers to, whose mutability is the callee's own capability over that
/// place (`mut xs` writes, a read parameter or receiver does not). A call
/// site substitutes its actual places to learn what the result's pointer
/// fields designate.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ViewReturnOrigin {
    slot: mojito_types::origin::OriginParamId,
    sig: mojito_types::origin::RefSig,
}

/// An origin binder a construction bound immutably, with the field path
/// below the constructed value at which the binding construction sits
/// (empty for the value's own explicit application).
type ImmutableOriginBinder = (Vec<String>, mojito_types::origin::OriginParamId);

/// One resolved origin slot of a view-returning call's result: the slot, the
/// concrete origin it binds at this call, and the callee's capability over
/// that place when its contract fixes one (`None` stays the caller's place
/// judgment).
type CallResultOrigin = (
    mojito_types::origin::OriginParamId,
    mojito_types::origin::Origin,
    Option<mojito_types::origin::Mutability>,
);

/// One runtime parameter of a selected callee, recorded per call site.
#[derive(Debug, Clone, PartialEq)]
struct CallParameter {
    name: String,
    convention: Option<ArgConvention>,
    ty: Ty,
}

/// How strictly a storage annotation must bind explicit origin slots.
///
/// `Full`: bare origin-slotted generics and partial applications both reject
/// (struct fields, uninitialized locals). `AllowBare`: a bare generic may
/// infer wholly from the binding's initializer, but a partial application
/// still cannot omit an origin slot (initialized locals — both halves
/// probe-attested against the pinned upstream). `Off`: ordinary resolution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageStrictness {
    Off,
    AllowBare,
    Full,
}

/// One overload candidate's rank is a single lexicographic word, most
/// significant term first, as the pinned Mojo orders them:
///
/// ```text
/// bits 31+   conversions            CONVERSION_UNIT units
/// bits 25-30 literal materializations  LITERAL_DEFAULT_RANK
/// bit  24    variadic use           VARIADIC_RANK
/// bit  23    empty collector        EMPTY_COLLECTOR_RANK   (variadic only)
/// bits 18-22 copies into `var`      COPY_RANK              (every candidate)
/// bits 10-17 signature length       SIGNATURE_LENGTH_RANK
/// bits 0-9   ArgumentBinding's low word and the generic bit
/// ```
const LITERAL_DEFAULT_RANK: usize = 1 << 25;

/// The units of a candidate's conversion cost, ranked from
/// `LITERAL_DEFAULT_RANK` up: an argument converting to its parameter costs
/// one whole `CONVERSION_UNIT`, and a literal materialized at its default
/// type (an `IntLiteral` bound to an `Int`) costs one unit, so it decides
/// only between candidates that convert alike, as an exact `IntLiteral`
/// parameter beside an `Int` one. Six bits: a candidate materializing more
/// literals than that ties with one converting once more.
const CONVERSION_UNIT: usize = 1 << 6;

const VARIADIC_RANK: usize = 1 << 24;

const EMPTY_COLLECTOR_RANK: usize = 1 << 23;

/// Five bits: a candidate copying more than `COPIES_MAX` arguments ties with
/// one copying that many, which loses an ordering rather than inverting it.
const COPY_RANK: usize = 1 << 18;

/// Eight bits, above the low word `ArgumentBinding::rank` and the generic bit
/// own.
const SIGNATURE_LENGTH_RANK: usize = 1 << 10;

/// Source-level arguments attached to a method invocation. Keeping the runtime
/// and compile-time argument lists together prevents the two method-resolution
/// paths from slowly acquiring different call-shape parameters.
#[derive(Clone, Copy)]
struct MethodCallArguments<'a> {
    param_args: &'a [mojito_ast::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
    parameterized_syntax: bool,
    /// The caller separately records a more precise projected write, so a
    /// `mut self` call must not also invalidate every receiver interior.
    preserves_receiver_interiors: bool,
}

impl<'a> MethodCallArguments<'a> {
    const fn ordinary(args: &'a [Expr], kwargs: &'a [mojito_ast::ast::KwArg]) -> Self {
        Self {
            param_args: &[],
            args,
            kwargs,
            parameterized_syntax: false,
            preserves_receiver_interiors: false,
        }
    }

    const fn interior_preserving(args: &'a [Expr], kwargs: &'a [mojito_ast::ast::KwArg]) -> Self {
        Self {
            preserves_receiver_interiors: true,
            ..Self::ordinary(args, kwargs)
        }
    }

    const fn parameterized(
        param_args: &'a [mojito_ast::ast::ParamArg],
        args: &'a [Expr],
        kwargs: &'a [mojito_ast::ast::KwArg],
    ) -> Self {
        Self {
            param_args,
            args,
            kwargs,
            parameterized_syntax: true,
            preserves_receiver_interiors: false,
        }
    }
}

type ReturnRefContract = (
    mojito_types::origin::RefSig,
    Vec<mojito_types::origin::OwnerId>,
    Option<mojito_types::origin::OriginPlace>,
);

#[derive(Clone)]
struct CapturePolicy {
    /// Scope index at which the nested function's own locals begin.
    base: usize,
    function_name: String,
    declaration: SourceSpan,
    entries: HashMap<String, mojito_ast::ast::CaptureKind>,
    default: Option<mojito_ast::ast::CaptureKind>,
    /// `@__parameter` environment: an unlisted name captures `mut` when its
    /// binding is mutable, `imm` otherwise.
    implicit_by_binding: bool,
    /// Whether this policy belongs to a lambda expression's hidden definition
    /// (selects the lambda wording for capture-convention diagnostics).
    lambda: bool,
}

/// How one source-level `Origin` parameter is represented by the callable's
/// slot-relative reference contracts. Origin parameters are erased from
/// `TyKind::Func`'s ordinary compile-time parameter list, so this checker-owned fact
/// is what lets a value expression such as `borrow[origin_of(value)]` bind that
/// parameter without reconstructing it from source later in MIR lowering.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CallableOriginParam {
    name: String,
    slots: Vec<usize>,
}

/// One source compile-time parameter in a callable declaration. `Origin`
/// parameters are erased from `GenericFuncTy::decls`, so retaining this
/// ordered layout is necessary to split a mixed specialization such as
/// `borrow[Int, origin_of(value)]` without shifting the ordinary type argument.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CallableSourceParam {
    name: String,
    infer_only: bool,
    origin: Option<usize>,
    ordinary: bool,
}

impl mojito_ast::call::CompileTimeParam for CallableSourceParam {
    fn keyword(&self) -> &str {
        self.name.trim_start_matches('*')
    }

    fn infer_only(&self) -> bool {
        self.infer_only
    }

    /// Whether the parameter is a pack (`*Ts`, `*values: Int`), which
    /// collects every positional argument from its slot on.
    fn variadic(&self) -> bool {
        self.name.starts_with('*')
    }
}

/// Origin-specialization metadata for one declaration in an overload set.
/// Entries are registered in the same order as `TyKind::Overload` candidates.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CallableOriginSignature {
    origins: Vec<CallableOriginParam>,
    source: Vec<CallableSourceParam>,
    /// Declaration constraints whose only binders were erased origin
    /// metadata, one per trailing `where` clause. They are checked after
    /// call-origin solving recovers the inferred Bool mutability arguments.
    availability: Vec<GenericConstraint>,
    /// See [`MethodSig::view_return`]; the free-function twin.
    view_return: Vec<ViewReturnOrigin>,
    /// The rejection of a use of an `@unavailable` declaration.
    unavailable: Option<String>,
}

/// The source-level pieces of a struct declaration passed through checking.
struct StructDeclaration<'a> {
    module: &'a Option<String>,
    name: &'a str,
    type_params: &'a [mojito_ast::ast::TypeParam],
    conforms: &'a [String],
    callable_conformance: &'a Option<SourceType>,
    conformance_conditions: &'a [(String, Expr)],
    where_clauses: &'a [Expr],
    fields: &'a [mojito_ast::ast::Param],
    associated: &'a [StructComptime],
    methods: &'a [Method],
    fieldwise_init: bool,
    decorators: &'a [mojito_ast::ast::Decorator],
    /// The cases of an `enum`; `None` for a `struct`.
    enum_cases: Option<&'a [mojito_ast::ast::EnumCase]>,
}

/// A method's own parameters instantiated at one call: its parameter,
/// collector, and keyword-collector types, the substitution, its own
/// arguments, and what each of the call's bracket arguments bound to.
type MethodInstantiation = (
    Vec<Ty>,
    Option<Ty>,
    Option<Ty>,
    TySubst,
    Vec<TyArg>,
    BracketAttributes,
);

struct MethodCallScore {
    rank: usize,
    simd_erasures: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
}

/// Interior-generation state immediately before applying one selected method
/// contract. Candidate scoring has already inferred the argument expressions at
/// this point, so subtracting this snapshot isolates callee effects from effects
/// which belong to evaluation of the source expression itself.
struct CallBoundarySnapshot {
    invalidations: HashMap<SourceSpan, Vec<mojito_checked::checked::InteriorInvalidation>>,
}

#[derive(Clone)]
struct ValueAdjustmentSnapshot {
    source: SourceSpan,
    overload_target: Option<String>,
    implicit_conversion: Option<String>,
    operation: Option<Box<mojito_checked::checked::SemanticAdjustment>>,
}

type SplitCallableSpecialization = (
    Vec<mojito_ast::ast::ParamArg>,
    ForwardedBrackets,
    Vec<(Vec<usize>, mojito_types::origin::Origin)>,
    HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
);

mod places;

mod builtin_fold;

mod comptime_validation;

mod comptime_requests;
pub use comptime_requests::{
    Callee, CalleeOracle, applies_callable, computes_value, lifts_comptime_value,
};

mod reflection;

mod rebind;

mod generics;

mod declarations;

mod annotations;

mod calls;

mod builtins;

pub use builtins::{builtin_copy_is_value_read, callable_environment_coerces};

mod operators;

mod iteration;

mod type_resolution;
use type_resolution::{
    BoundParameters, BracketAttributes, ForwardedBrackets, bracket_attribute, dtype_attribute,
    tyarg_is_symbolic, types_attributes,
};

mod constraints;

mod scopes;

mod origins;

pub use mojito_symbol::symbol::callable_contract_target;
pub use mojito_types::types::{callable_bound_accepts, callable_contract_ty};

mod traits;

mod decorators;

mod inference;

mod initializer_list;

mod template_string;

mod comptime_iteration;

pub use template_string::template_string_entry;

mod indexing;

mod method_calls;

/// Count one body inference under `body_inference.template` for a generic
/// body and `body_inference.plain` otherwise, naming the body in the notes.
fn count_body_inference(generic: bool, name: impl FnOnce() -> String) {
    let counter = if generic {
        "body_inference.template"
    } else {
        "body_inference.plain"
    };
    timing::count(counter, 1);
    timing::note(counter, name);
}

mod call_inference;
mod ffi_calls;

mod bound_defaults;
mod clusters;
mod coverage;
mod destructure;
mod desugar;
mod patterns;
pub use patterns::irrefutable as irrefutable_pattern;
mod statements;
mod with_stmt;

#[cfg(test)]
mod dependent_callable_signature_tests {
    use super::*;

    fn indexed_callable(binder: &str, offset: i64) -> Ty {
        let context = ParamContext::new();
        // Each contract owns its binder: alpha-equivalence must come from the
        // signature slots, not from two declarations sharing an identity.
        let reference = value_binder_expr(
            ParamId::new(&format!("$contract:{binder}"), 0),
            binder,
            &Ty::int(),
        );
        let index = context
            .infix(
                InfixOp::Add,
                &reference,
                &context
                    .constant(CtValue::Int(offset))
                    .expect("an Int is a constant"),
            )
            .expect("Int + Int builds");
        TyKind::GenericFunc(Box::new(GenericFuncTy {
            decls: vec![ParamDecl::Value {
                id: ParamId::new(&format!("$contract:{binder}"), 0),
                name: binder.to_string(),
                ty: Ty::int(),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            func: FuncTy {
                environment: mojito_types::origin::CallableEnvironment::Thin,
                params: vec![DependentType::resolve(
                    context
                        .select(vec![Ty::int(), Ty::string_literal()], &index)
                        .expect("an Int index selects"),
                )],
                names: vec!["element".to_string()],
                ret: Ty::none(),
                required: vec![true],
                variadic: None,
                kw_variadic: None,
                positional_only: None,
                keyword_only: None,
                raises: false,
                error: None,
                conventions: vec![Some(ArgConvention::Var)],
                ref_params: vec![None],
                ref_return: None,
                transfers: TransferSet::default(),
            },
        }))
        .into()
    }

    fn callable_with_parameter_role(role: mojito_ast::ast::ParamKind) -> Ty {
        let mut callable = indexed_callable("index", 0);
        callable.update(|kind| {
            let Some(FuncTy {
                params,
                names,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_params,
                ..
            }) = kind.callable_mut()
            else {
                unreachable!("indexed_callable constructs a generic function")
            };
            let parameter = params.pop().expect("one regular parameter");
            names.clear();
            required.clear();
            conventions.clear();
            ref_params.clear();
            match role {
                mojito_ast::ast::ParamKind::Regular => params.push(parameter),
                mojito_ast::ast::ParamKind::Variadic => *variadic = Some(parameter),
                mojito_ast::ast::ParamKind::KwVariadic => *kw_variadic = Some(parameter),
            }
        });
        callable
    }

    #[test]
    fn dependent_callable_binders_are_alpha_equivalent() {
        assert!(same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 0),
        ));
        assert!(!same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 1),
        ));
    }

    #[test]
    fn generic_parameter_roles_are_not_redeclaration_equivalent() {
        let regular = callable_with_parameter_role(mojito_ast::ast::ParamKind::Regular);
        let variadic = callable_with_parameter_role(mojito_ast::ast::ParamKind::Variadic);
        let kw_variadic = callable_with_parameter_role(mojito_ast::ast::ParamKind::KwVariadic);
        assert!(!same_callable_signature(&regular, &variadic));
        assert!(!same_callable_signature(&regular, &kw_variadic));
        assert!(!same_callable_signature(&variadic, &kw_variadic));
    }
}
