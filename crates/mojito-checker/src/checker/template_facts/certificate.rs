//! The certificate a freshly captured generic body earns: the class its
//! declaration falls in, the obligations an instance must discharge, and the
//! reason a body outside every class keeps the clone check.

use super::{
    BodyDeclaration, BodyShape, BodySite, GrammarNotes, adjustment_derives, bound_binder,
    callable_binder, closed_scalar, fact_at, grammar_scalar, nested_def_calls, origin_binder,
    template_callee,
};
use crate::checker::Checker;
use crate::checker::builtins::simd_wildcard_binder;
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    CheckedBodyFacts, FactTable, IncompleteReason, MethodFeatures, OccurrenceId, TemplateClass,
    TemplateCoverage,
};
use mojito_types::types::{ParamDecl, Ty};
use std::cell::RefCell;

impl Checker {
    /// The certificate of a body's class, with the operators the grammar
    /// admitted over parameter-typed operands. With no facts it judges the
    /// declaration's syntax alone: `Certified` then means only that the body
    /// is worth capturing.
    pub(super) fn certificate(
        &self,
        site: &BodySite<'_>,
        facts: Option<&CheckedBodyFacts>,
    ) -> (TemplateCoverage, GrammarNotes) {
        let (decls, ret_ty) = (site.decls, site.ret_ty);
        match site.declaration {
            BodyDeclaration::Def(stmt) => self.template_certificate(stmt, decls, ret_ty, facts),
            BodyDeclaration::Method(method) => {
                self.method_certificate(method, decls, ret_ty, facts)
            }
        }
    }

    /// The certificate a freshly captured module-level generic function earns.
    ///
    /// Every class shares a declaration shape: plain type parameters, plain
    /// regular runtime parameters (read, and in a runtime body `var` or `mut`),
    /// a concrete scalar result, and no captures, decorators, or `where`
    /// clauses, so the declaration's bounds —
    /// discharged where an instance is requested — are all an instance owes.
    /// Only a [`TemplateClass::FunctionBody`] may raise
    /// (`MethodFeatures::RAISES`).
    ///
    /// [`TemplateClass::ClosedScalarBody`] bodies name nothing: every fact is
    /// closed and inherited unchanged. [`TemplateClass::FixedCalls`] bodies also
    /// call module-scope functions directly. The checks a clone would repeat for
    /// such a call are covered as follows.
    ///
    /// - Selection: the call resolved to one module-scope declaration, or to one
    ///   member of an overload set, whose lowered symbol is retained. An instance
    ///   inherits that choice and never ranks the set again on its concrete
    ///   arguments: the pinned Mojo binds a call inside a generic body once, when
    ///   it checks the body (`conformance/probes/template_overload_binding.mojo`).
    /// - Argument typing: the template recorded no copy, move, or adjustment
    ///   at the call (any such table refuses the capture), so each argument
    ///   either matched its parameter exactly with `T` symbolic, and matches
    ///   exactly after substitution, or converts through an `@implicit`
    ///   constructor, which the instance selects again from its own source
    ///   and target types (`realize_conversion`) and which never re-ranks the
    ///   callee. A keyed body converts in an arm the same way: the instance
    ///   re-selects only in the arms the elaborator kept.
    /// - Binding conventions: borrows are decided from slots, conventions, and
    ///   argument shape, none of which mention a type.
    /// - Effects: the callee raises only where the body may (`effect_derives`),
    ///   and its transfer and call-through summaries were empty; a realization
    ///   re-reads them and installs the same fixpoint observation a clone
    ///   check would.
    /// - Requests: the retained application substitutes, and
    ///   `realize_instance_facts` repeats the clone check's only concrete
    ///   decision, whether that application's clone already exists.
    ///
    /// A [`TemplateClass::FixedCalls`] body may also call the built-in `len`.
    /// With `T` symbolic its bound proves the call; an instance owes the
    /// witness and takes the concrete read-in-place fact
    /// (`realize_builtin_len`).
    ///
    /// An operator is admitted only over operands whose recorded types are
    /// closed scalars, so no operator in these classes dispatches through a
    /// bound.
    fn template_certificate(
        &self,
        stmt: &Stmt,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        facts: Option<&CheckedBodyFacts>,
    ) -> (TemplateCoverage, GrammarNotes) {
        let outside = |what| {
            (
                TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what)),
                GrammarNotes::default(),
            )
        };
        let StmtKind::Def {
            type_params,
            params,
            captures,
            body,
            raises,
            raises_type,
            decorators,
            ..
        } = &stmt.kind
        else {
            return outside("not a function");
        };
        // A type pack is fixed per instance as a type binder is: the
        // elaborator writes its elements into the clone's signature, and an
        // element the body reads by loop index is fixed by the unrolling
        // (`TemplateClass::PackElements`).
        let pack_binders: Vec<&str> = decls
            .iter()
            .filter_map(|decl| match decl {
                ParamDecl::Type {
                    name,
                    variadic: true,
                    ..
                } => Some(name.trim_start_matches('*')),
                ParamDecl::Type { .. } | ParamDecl::Value { .. } => None,
            })
            .collect();
        let plain_binders = type_params.iter().all(|parameter| {
            parameter.callable_bound.is_none()
                && parameter.default.is_none()
                && parameter.origin_mutability.is_none()
        }) && decls.iter().all(|decl| match decl {
            // A binder's constraints are the declaration's `where` clauses:
            // the requesting call and the elaborator discharge them before an
            // instance exists (`TemplateObligation::DeclarationConstraints`).
            ParamDecl::Type {
                callable_bound: None,
                ..
            } => true,
            // A keyed body reads a `DType` binder only as a lane dtype, which
            // the grammar admits in a construction alone
            // ([`BodyShape::simd_construction`]).
            ParamDecl::Value {
                ty,
                variadic: false,
                ..
            } => {
                matches!(**ty, Ty::Bool | Ty::Int) || (self.source_validation && **ty == Ty::Dtype)
            }
            ParamDecl::Type { .. } | ParamDecl::Value { .. } => false,
        });
        if !plain_binders || decls.len() != type_params.len() {
            return outside("a compile-time parameter is not a plain type or scalar value");
        }
        // A reflection query over a parameter is validated as a node; the
        // instance's field facts are the elaborator's, so no fact of this
        // body derives.
        if crate::checker::comptime_validation::reads_reflection(body) {
            return outside("a body reading a reflection handle keeps its clone check");
        }
        // One producer per body: source validation owns every body it
        // checks (keyed by a `comptime for`, a pack, or a `rebind`), the
        // executable check the surviving ones, a value-keyed body with
        // neither among them. A `comptime if` keys nothing: the template
        // keeps the region, and the elaborator below MIR selects.
        let keyed = self.source_validation;
        if !keyed
            && (!pack_binders.is_empty()
                || crate::checker::rebind::body_keys_rebind(body, &self.rebind_keyed_bodies))
        {
            return outside("a compile-time-keyed body is source validation's to certify");
        }
        // A variadic parameter is admitted only as the collector of one of
        // the declaration's own packs, which an instance binds as the tuple
        // of the elements written in its signature.
        let pack_collector = |parameter: &mojito_ast::ast::FnParam| {
            parameter.kind == mojito_ast::ast::ParamKind::Variadic
                && matches!(&parameter.ty, mojito_ast::ast::Type::Named(name, arguments)
                    if arguments.is_empty()
                        && pack_binders.contains(&name.trim_start_matches('*')))
        };
        // A `var` or `mut` parameter of a runtime body is bound from its
        // declared convention alone, and is rooted at its own binding under
        // every instance, as a method's is.
        let owned_param = |parameter: &mojito_ast::ast::FnParam| {
            !keyed
                && matches!(
                    parameter.convention,
                    Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Mut)
                )
        };
        let plain_params = params.iter().all(|parameter| {
            (parameter.kind == mojito_ast::ast::ParamKind::Regular || pack_collector(parameter))
                && (parameter.convention.is_none() || owned_param(parameter))
                && parameter.default.as_ref().is_none_or(|default| {
                    literal_default(default) || self.literal_construction(default)
                })
                && parameter.origin.is_none()
        });
        if !plain_params {
            return outside(
                "a parameter is not an immutable, 'var', or 'mut' regular parameter with at most \
                 a literal default or a construction from literals",
            );
        }
        let owned_params = params.iter().any(owned_param);
        let mut_params: Vec<&str> = params
            .iter()
            .filter(|parameter| parameter.convention == Some(mojito_ast::ast::ArgConvention::Mut))
            .map(|parameter| parameter.name.as_str())
            .collect();
        if captures.is_some() || mojito_ast::ast::has_body_decorator(decorators) {
            return outside("the declaration captures or is decorated");
        }
        if keyed && (*raises || raises_type.is_some()) {
            return outside("a keyed body raises");
        }
        // A body returning nothing falls off its end: the grammar admits no
        // value `return` for it, and a bare `return` only in a runtime body.
        // A runtime body may return a whole value of any type, which every
        // `return` must move or copy at exactly the declared type.
        if !closed_scalar(ret_ty) && *ret_ty != Ty::None && keyed {
            return outside("the return type is not a concrete scalar");
        }
        let packs: Vec<&str> = params
            .iter()
            .filter(|parameter| pack_collector(parameter))
            .map(|parameter| parameter.name.as_str())
            .collect();
        let desugars = self.with_desugars.borrow();
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            structs: &self.structs,
            traits: &self.traits,
            desugars: &desugars,
            desugar_depth: std::cell::Cell::new(0),
            error_binders: RefCell::new(Vec::new()),
            constants: &self.comptimes,
            vector_aliases: self.vector_aliases(),
            params: params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            packs,
            pack_struct: None,
            pack_binders: pack_binders.clone(),
            loop_vars: RefCell::new(Vec::new()),
            values: decls
                .iter()
                .filter_map(|decl| match decl {
                    ParamDecl::Value { name, .. } => Some(name.as_str()),
                    ParamDecl::Type { .. } => None,
                })
                .collect(),
            struct_values: Vec::new(),
            struct_lanes: Vec::new(),
            struct_vectors: Vec::new(),
            print_calls: RefCell::new(Vec::new()),
            nested_depth: std::cell::Cell::new(0),
            runtime_loops: std::cell::Cell::new(0),
            borrowed_params: mut_params.clone(),
            mut_params,
            deinit_params: Vec::new(),
            keyed,
            receiver: false,
            self_convention: None,
            // A runtime body may hold a whole value of any type in a local
            // or an argument, and iterate a place, as a method's may
            // (`FunctionBody`); a keyed body keeps source validation's rules.
            moved_result: (!keyed).then_some(ret_ty),
            reference_result: None,
            // A `var` or `mut` parameter makes the body a `FunctionBody`, whose
            // instances owe plain-data arguments.
            features: std::cell::Cell::new(
                [
                    (*raises || raises_type.is_some(), MethodFeatures::RAISES),
                    (owned_params, MethodFeatures::OWNED_PARAMETERS),
                ]
                .into_iter()
                .filter(|(held, _)| *held)
                .fold(MethodFeatures::default(), |features, (_, feature)| {
                    features.union(MethodFeatures::STATEMENTS).union(feature)
                }),
            ),
            locals: RefCell::new(Vec::new()),
            nested_params: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
            subscripts: RefCell::new(Vec::new()),
            places: RefCell::new(Vec::new()),
            operators: RefCell::new(Vec::new()),
            bound_builtins: RefCell::new(Vec::new()),
            constructions: RefCell::new(Vec::new()),
            binders: Vec::new(),
            struct_binders: Vec::new(),
            binder_constructions: RefCell::new(Vec::new()),
            type_arguments: RefCell::new(Vec::new()),
            callable_params: Vec::new(),
            callable_calls: RefCell::new(Vec::new()),
            callable_binders: Vec::new(),
            static_calls: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
            lane_binders: Vec::new(),
            simd_to_bits: RefCell::new(Vec::new()),
            simd_casts: RefCell::new(Vec::new()),
            simd_lengths: RefCell::new(Vec::new()),
            pack_relocations: RefCell::new(Vec::new()),
            pack_spreads: RefCell::new(Vec::new()),
            element_constructions: RefCell::new(Vec::new()),
            stringified: RefCell::new(Vec::new()),
            copied_writes: RefCell::new(Vec::new()),
        };
        if !shape.block(body)
            || !shape.operators.borrow().is_empty()
            || !shape.bound_builtins.borrow().is_empty()
        {
            return outside("the body is not scalar returns over direct calls and 'len'");
        }
        // The scalar classes argue for runtime statements over closed scalars
        // (`STATEMENTS`); a body holding more is a `FunctionBody`, whose
        // certificate argues for each feature `FUNCTION_FEATURES` names. With
        // no facts the grammar reads a name by its syntax alone, so a feature
        // it holds may be one the recorded types rule out (`ptr.unsafe_free()`
        // on a local is a copied consuming call until its type names an
        // untracked pointer).
        let features = shape.features.get();
        let widened = !features.without(MethodFeatures::STATEMENTS).is_empty();
        if facts.is_some() && !FUNCTION_FEATURES.contains(features) {
            return outside("the body holds a construct outside the function classes");
        }
        let class = |class| {
            (
                TemplateCoverage::Certified(class),
                GrammarNotes {
                    repr_calls: shape.repr_calls.borrow().clone(),
                    print_calls: shape.print_calls.borrow().clone(),
                    constructions: shape.constructions.borrow().clone(),
                    simd_to_bits: shape.simd_to_bits.borrow().clone(),
                    simd_casts: shape.simd_casts.borrow().clone(),
                    simd_lengths: shape.simd_lengths.borrow().clone(),
                    element_constructions: shape.element_constructions.borrow().clone(),
                    ..GrammarNotes::default()
                },
            )
        };
        let Some(facts) = facts else {
            return class(TemplateClass::ClosedScalarBody);
        };
        if !shape.references_recorded(facts) {
            return outside("an expression yields or keeps a reference");
        }
        if !facts.call_throughs.is_empty() || !facts.call_through_reads.is_empty() {
            return outside("the body calls or forwards a callable parameter");
        }
        if !facts
            .expression_effects
            .iter()
            .all(|(_, effects)| effect_derives(effects))
        {
            return outside("a call has an effect other than raising");
        }
        let adjustments_derive = facts
            .operation_adjustments
            .iter()
            .all(|(_, adjustment)| adjustment_derives(adjustment));
        if !adjustments_derive {
            return (
                TemplateCoverage::Incomplete(IncompleteReason::UnsupportedTable(
                    FactTable::OperationAdjustments,
                )),
                GrammarNotes::default(),
            );
        }
        // Every call selected a module-scope declaration or is a method call
        // or in-place update the grammar admitted, which records its
        // parameters here too; every effect summary read belongs to one of
        // those calls, and no application carries a pack.
        let callees: Option<Vec<&str>> = facts
            .call_parameters
            .iter()
            .map(|(id, _)| {
                fact_at(&facts.selected_calls, *id)
                    .or_else(|| fact_at(&facts.inplace_updates, *id))
                    .map_or_else(
                        || template_callee(facts, *id),
                        |call| Some(call.contract.target.as_str()),
                    )
            })
            .collect();
        let Some(callees) = callees else {
            return outside("a call's callee is not a module-scope declaration");
        };
        if !facts.effect_free_callees.iter().all(|callee| {
            callees.contains(&callee.as_str()) || dispatched_conformer(&callees, callee)
        }) {
            return outside("an effect summary was read outside a direct call");
        }
        if facts.replays_transfers() {
            return outside("the body replays a transfer summary");
        }
        // A construction's `__init__` target is re-selected per instance
        // ([`Checker::realize_construction`]); it records no call parameters.
        let at_call = |id: &OccurrenceId| facts.call_parameters.iter().any(|(call, _)| call == id);
        let constructions = shape.constructions.borrow();
        let stringified = shape.stringified.borrow();
        if !facts
            .overload_targets
            .iter()
            .all(|(id, _)| at_call(id) || constructions.contains(id) || stringified.contains(id))
            || facts
                .generic_instantiations
                .iter()
                .any(|(id, application)| application.variadic.is_some() || !at_call(id))
        {
            return outside("an application carries a pack or is not a direct call");
        }
        let closed = facts
            .expression_types
            .iter()
            .chain(&facts.expression_place_types)
            .chain(&facts.binding_types)
            .all(|(_, ty)| !mojito_types::types::is_symbolic(ty));
        class(if !shape.packs.is_empty() {
            TemplateClass::PackElements
        } else if keyed {
            TemplateClass::ScalarBranches
        } else if widened {
            TemplateClass::FunctionBody(features)
        } else if !facts.call_parameters.is_empty()
            || !facts.builtin_len_calls.is_empty()
            || !closed
        {
            TemplateClass::FixedCalls
        } else {
            TemplateClass::ClosedScalarBody
        })
    }

    /// Whether a parameter default constructs a declared struct from literal
    /// arguments (`String("-")`). It names no binding and no binder, so the
    /// callee evaluates it at a call leaving the slot out as the same
    /// construction under every instance, as it does a literal default.
    fn literal_construction(&self, default: &Expr) -> bool {
        matches!(&default.kind, ExprKind::Call { name, param_args, args, kwargs }
            if param_args.is_empty()
                && self.structs.contains_key(name)
                && args.iter().all(literal_default)
                && kwargs.iter().all(|keyword| literal_default(&keyword.value)))
    }

    /// The certificate a freshly captured method of a generic struct earns.
    ///
    /// [`TemplateClass::MethodScalarBody`]: a method with a plain read `self`
    /// and no binders of its own, on a struct whose binders are plain type
    /// parameters, returning a concrete scalar over closed scalars, runtime
    /// parameters, and reads of `self`'s scalar fields. It calls nothing.
    ///
    /// What a clone check would repeat is covered as follows.
    ///
    /// - Types: a field read is the field's declared type under the struct's
    ///   arguments in a template and a clone alike, and the class admits the
    ///   read only when that type is already a closed scalar. `self` itself
    ///   substitutes to the clone's resolved receiver.
    /// - Copies: a scalar field copied out is implicitly copyable under every
    ///   instance; realization still re-judges each copied place.
    /// - The generated-declaration leniency a clone's name switches on bears
    ///   on origin-bearing return annotations only, and the result is a
    ///   scalar.
    /// - Nothing outside the fact tables: the body wrote through no origin
    ///   parameter, transferred nothing, and recorded no request.
    ///
    /// [`TemplateClass::MethodBody`] widens that along eight independent
    /// [`MethodFeatures`]. The receiver may be `mut`, `var`, `deinit`, a bare
    /// `ref`, the `out` of an initializer (the copy and move ones too), or
    /// absent (`@staticmethod`), and
    /// the method may carry a `where` clause: the elaborator mints a clone
    /// only where the clause holds, and a clone's signature no longer states
    /// it. A parameter may be `var`, `deinit`, `mut`, or a bare `ref`: it is
    /// bound from
    /// its declared convention and rooted at its own binding under every
    /// instance, its loan state is decided by a property
    /// [`TemplateObligation::PlainDataArguments`] rules out, and what its
    /// caller owes lives in the signature, which is checked per clone. A `mut`
    /// parameter may be stored to; a bare `ref` one has parametric
    /// mutability, and a write through it is refused below. A `var *values`
    /// collector is such a parameter too: its type is a pack of a
    /// substituted type, and the body owns its binding. A `None` default is
    /// the same value under every instance. A `deinit` parameter (a move
    /// initializer's source) is torn down by the body, which may move its
    /// fields out. A receiver or parameter origin and binders stay outside.
    ///
    /// - `STATEMENTS`: a runtime statement is checked once whatever runs it,
    ///   so `if`, `while`, `break`, `continue`, and a bare `return` neither
    ///   drop nor copy an occurrence. A condition's recorded type is exactly
    ///   `Bool`, which `expect_bool` accepts without a truthiness fact, unless
    ///   the body holds `TRUTHINESS`. A
    ///   stored field is a closed scalar, so the store is never an in-place
    ///   operator of the field's type. Invalidations name `self` and locals
    ///   by template owner.
    /// - `OPAQUE_MOVES`: a value of any type is moved or copied whole between
    ///   a parameter, a local, a field of `self`, and the result, and is
    ///   never an operand, a receiver, a condition, or an argument, so
    ///   nothing dispatches on its type. A store or a result has the value's
    ///   own recorded type, which stays equal under substitution, so neither
    ///   check converts. What a clone check still decides from the type is
    ///   owed per instance: the copy of a place (admitted only where the
    ///   template recorded it), `Movable` at a transfer, whether a local can
    ///   be destroyed, and that an argument is plain data
    ///   ([`TemplateObligation`]).
    /// - `POINTER_SLOTS`: a pointer field with no tracked provenance holds no
    ///   loan, names no place, and is a pointer under every instance, so its
    ///   methods are the built-in ones and record an adjustment naming at
    ///   most the pointee (`derive_adjustment`). Every other judgment there
    ///   only produces an error, and the template's is at least as strict.
    /// - `SIBLING_CALLS`: see [`Self::realize_method_call`] and
    ///   `closed_method_contract`. Arguments are closed scalars or kept
    ///   places in both checks; a call-through residue refuses.
    /// - `VALUE_ARGUMENTS`: see [`BodyShape::argument`] and
    ///   `value_method_contract`. A whole value bound by value has exactly
    ///   its parameter's type, so nothing converts it under any instance,
    ///   and the borrow, temporary, transfer, and copy facts a call records
    ///   for it are decided by its syntax and the callee's conventions; the
    ///   copy and the transfer are owed again per instance. An overloaded
    ///   family with symbolic parameter types is admitted only for exact
    ///   arguments, which no member outranks.
    /// - `REPLAYED_TRANSFERS`: see [`Self::body_transfers`] and
    ///   [`Self::realize_transfers`]. The template records each replayed
    ///   transfer, the origins it merged, and the effect its frame derived,
    ///   every source by the binding it is rooted at; an instance replays
    ///   them again, dropping a source whose binding is plain data
    ///   ([`TemplateObligation::ReplayedTransfers`]).
    /// - `OPERATOR_DISPATCH`: see [`BodyShape::operator`] and
    ///   [`Self::realize_operator`]. The template records nothing at an
    ///   operator its bound proves, and both operands are places read where
    ///   they lie; the instance repeats `infer_infix`'s type-driven selection
    ///   and records the target it names, together with the copy, conversion,
    ///   and negated-equality adjustment that selection carries. An
    ///   arithmetic operator's result is the operand's type, so it is a
    ///   temporary of that type wherever the body puts it
    ///   ([`BodyShape::operator_value`]).
    /// - `BOUND_DISPATCH`: see [`BodyShape::bound_dispatch`] and
    ///   [`Self::realize_bound_dispatch`]. A method call through a bound
    ///   records the abstract contract, or the inverted write, and nothing
    ///   the receiver's type decides; the instance repeats
    ///   `infer_method_call`'s type-driven choice between a place read, a
    ///   hashed leaf, and the struct's own method (`bound_witness`).
    /// - `BOUND_BUILTINS`: see [`BodyShape::bound_builtin`] and
    ///   [`Self::realize_bound_builtin`]. `hasher.update(x)` and
    ///   `writer.write(x)` select no callee and record at an argument only
    ///   what its syntax decides; the instance proves the argument's bound
    ///   again at its own type.
    /// - `BOUND_BINDERS`: a trait-bounded binder of the method's own
    ///   (`[H: Hasher]`) is kept by every clone, bound symbolically as the
    ///   template binds it, and substituted by nothing, so a construction
    ///   of one records the same adjustment in every clone (see
    ///   [`BodyShape::binder_construction`]).
    /// - `REFERENCE_RESULT`: see [`BodyShape::returned_place`]. The handle a
    ///   `return` keeps is decided by the declaration and the statement's
    ///   syntax, and the declared origin is checked on the place's path and
    ///   the signature, which the clone's own signature check repeats.
    /// - `REFERENCE_CALLS`: see [`BodyShape::reference_call`] and
    ///   `closed_reference_contract`. The reference a call yields names the
    ///   receiver, so it is kept by template owner
    ///   ([`TemplateReference`]), and an instance marks its copyable reads
    ///   again at its own referent.
    /// - `REFERENCE_LOCALS`: see [`BodyShape::bound_place`]. A `ref`
    ///   binding's type is a reference naming a binding, kept by template
    ///   owner like a call's; every use records the referent.
    /// - `REFERENCE_RECEIVERS`: see [`BodyShape::reference_receiver`]. A call
    ///   borrows a receiver reached through a reference because of what the
    ///   receiver is, and the callee is realized as a sibling call's is.
    /// - `REFERENCE_ARGUMENTS`: see [`BodyShape::reference_argument`]. An
    ///   argument reached through a reference is read, copied, kept, or lent
    ///   as a named place is, by its syntax and the callee's conventions; a
    ///   constructor's `BorrowRefArguments` names the lending positions and
    ///   each loan's mutability, neither of which an instance changes
    ///   (`derive_adjustment`).
    /// - `RAISES`: see [`BodyShape::raised`] and [`effect_derives`]. A
    ///   `raise` records nothing of its own, a raising call's effect and
    ///   contract carry the callee's error type, which substitutes, and the
    ///   judgment an instance repeats at either holds under every
    ///   substitution the template's holds under.
    /// - `CONSUMING_CALLS`: see [`BodyShape::consuming_call`] and
    ///   `consuming_nominal_contract`. The receiver's move is recorded at its
    ///   `^` transfer, and a named `deinit self` destructor's mark is the
    ///   struct's declaration, so an instance changes only the target, as a
    ///   sibling call's.
    /// - `COPIED_RECEIVERS`: see [`BodyShape::copied_consuming_call`]. The
    ///   call's copy of its receiver is decided by the receiver's syntax and
    ///   the callee's convention, and the instance owes it at its own type.
    /// - `DIRECT_CALLS`: see `method_direct_calls`. A callee taking closed
    ///   scalars, the member of an overload set (`range(n)`) or a generic
    ///   one applied to types (`unsafe_alloc[Self.T](n)`), is selected alike
    ///   under every instance, which realizes it as a function template's
    ///   direct call, substituting the application's arguments.
    /// - `ITERATION`: see [`BodyShape::iterable`] and `realize_iterations`.
    ///   The protocol a loop records is selected from the iterable's type
    ///   and resolved against the place it borrows, so an instance keeps the
    ///   place and selects again from its own substituted type; the loop
    ///   variable's binding is a local like any other.
    /// - `SIMD_CONSTRUCTIONS`: see [`BodyShape::simd_construction`]. Only a
    ///   closed construction records its dtype and width, which the instance
    ///   installs as they stand; its value goes to a checker builtin, a
    ///   by-value parameter, or a `var` local.
    /// - `TRUTHINESS`: see [`BodyShape::truthiness_condition`]. A condition
    ///   read whole from a place is marked for `Bool(x)` by its type alone,
    ///   so an instance repeats `expect_bool`'s judgment at its own type.
    /// - `TUPLE_UNPACKS`: see [`BodyShape::tuple_unpack`]. The element reads
    ///   are a function of the value's type and place, from which an
    ///   instance builds them again.
    /// - `PARAMETERIZED_CALLS`: see [`BodyShape::parameterized_call`]. The
    ///   declared compile-time parameters are the callee's, and on a
    ///   non-generic receiver the per-call clone the call targets is the same
    ///   under every instance (`realize_method_call`).
    /// - `COMPREHENSIONS`: see [`BodyShape::comprehension`]. Each clause's
    ///   protocol is an `ITERATION` loop's, and each binder is declared from
    ///   that protocol's binding plan, which an instance selects again
    ///   (`install_comprehension_bindings`).
    /// - `COMPTIME_CONTROL`: a `comptime if` or `comptime for` in a body
    ///   source validation checked. Every arm was checked once with the
    ///   struct's parameters symbolic; an instance keeps the occurrences of
    ///   the arms the elaborator selected, once per unrolled copy, and drops
    ///   the rest with their facts (`CheckedBodyFacts::selected`). A loop
    ///   variable read as a runtime value is the copy's literal, which takes
    ///   a literal's facts (`folded_literals`), and a local declared inside
    ///   the loop is one binding per copy (`renumber_locals`), as in a keyed
    ///   `def`. A `rebind` in such a body is erased and its equality taken on
    ///   faith, which each instance discharges at its own type
    ///   (`TemplateObligation::RebindEqualities`): one that does not hold
    ///   refuses the derivation, and the clone check reports it.
    /// - `NESTED_DEFS`: see [`BodyShape::nested_def`]. A nested `def`'s
    ///   declaration facts are its signature, keyed by its statement and
    ///   substituted, and its captures name the body's own bindings, so an
    ///   instance writes them again under its own statement and bindings
    ///   (`install_nested_defs`); a call of it selects the declaration the
    ///   body introduces, under every instance. A nested body's effect reads
    ///   are the enclosing body's too.
    /// - `STATIC_CALLS`: see [`BodyShape::static_call`]. A static records at
    ///   most its overload member and, behind a leading-dot root, the
    ///   expected type's head. An instance inherits the head, and the member
    ///   too, except that a generic struct's spelled receiver names the
    ///   instance's clone of it (`realize_static_overloads`), or the
    ///   per-call clone of a member with binders of its own
    ///   (`realize_static_instantiations`).
    ///
    /// Any other handle, borrowed receiver, reference result, interior
    /// reference, or copyable read in the body refuses it
    /// ([`BodyShape::references_recorded`]).
    /// The struct binders `select` picks for a member only source
    /// validation checks: a struct specialized whole, whose clones fold them.
    fn validated_struct_binders(&self, select: fn(&[ParamDecl]) -> Vec<&str>) -> Vec<&str> {
        if self.source_validation {
            select(&self.self_decls)
        } else {
            Vec::new()
        }
    }

    fn method_certificate(
        &self,
        method: &mojito_ast::ast::Method,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        facts: Option<&CheckedBodyFacts>,
    ) -> (TemplateCoverage, GrammarNotes) {
        use mojito_ast::ast::ArgConvention;
        let outside = |what| {
            (
                TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what)),
                GrammarNotes::default(),
            )
        };
        // `__init__(out self, …)` is an ordinary owned receiver here: which
        // fields a body initializes is its syntax, and definite initialization
        // is judged outside the body check. The copy and move initializers are
        // such initializers too: their `copy:` and `deinit move:` source is an
        // ordinary parameter of the struct's own type.
        let lifecycle = mojito_symbol::symbol::lifecycle_method_name(method);
        let constructs = matches!(lifecycle, "__init__" | "__copyinit__" | "__moveinit__")
            && method.self_convention == Some(ArgConvention::Out);
        let is_static = !method.has_self
            && matches!(method.decorators.as_slice(), [decorator]
                if decorator.path == ["staticmethod"]
                    && decorator.args.is_empty()
                    && decorator.kwargs.is_empty());
        let plain_read =
            method.has_self && matches!(method.self_convention, None | Some(ArgConvention::Imm));
        // A bare `ref self` has parametric mutability, so the body may not
        // write through it, and one binding identity names it under every
        // instance as it does any other receiver.
        let owned_receiver = method.has_self
            && matches!(
                method.self_convention,
                Some(
                    ArgConvention::Mut
                        | ArgConvention::Var
                        | ArgConvention::Deinit
                        | ArgConvention::Ref
                )
            );
        // A receiver origin naming one of the method's own origin binders
        // (`ref [o] self`) is a signature fact like a `ref` parameter's
        // clause: the binder is kept by every clone (`ORIGIN_PARAMETERS`).
        let receiver_origin_kept = method.self_origin.as_ref().is_none_or(|origins| {
            matches!(origins.as_slice(), [origin]
            if matches!(&origin.kind, ExprKind::Identifier(name)
                if method.type_params.iter().any(|binder| {
                    binder.name == *name && origin_binder(binder)
                })))
        });
        if !(plain_read || owned_receiver || is_static || constructs) || !receiver_origin_kept {
            return outside("the receiver carries an origin that is not the method's own binder");
        }
        // A `where` clause is the declaration's constraint: the elaborator
        // mints a clone only where it evaluates true, and a trace exists only
        // for a minted clone (`TemplateObligation::DeclarationConstraints`).
        // An origin binder and a trait-bounded type binder (`[H: Hasher]`)
        // are kept by every clone and bound symbolically as the template
        // binds them, so no fact reads either. The wildcard vector binder is
        // baked by every clone; only source validation sees its body, with
        // the parameter viewed as a lane-shaped vector (`simd_binder_view`),
        // and the elaborated program holds a trap stub in its place, which
        // names nothing the binder stands for.
        // A scalar or `DType` value binder of the method's own (`[n: Int]`,
        // `[dt: DType]`) is folded by every per-call clone, as a value-keyed
        // `def`'s is; a vector over it (`Scalar[dt]`) is value-shaped.
        let simd_binders = method.type_params.iter().any(simd_wildcard_binder);
        let trap_stub = matches!(method.body.as_slice(), [statement]
            if matches!(&statement.kind, StmtKind::Expr(Expr { kind: ExprKind::Call { name, .. }, .. })
                if name == "_mojito_abort"));
        // A compile-time callable binder of the method's own is kept by every
        // clone, which the specializer never folds: it names no callable in
        // generic identity (`CALLABLE_BINDERS`).
        let own_decls = &decls[self.self_decls.len().min(decls.len())..];
        let callable_binders = method_callable_binders(method, own_decls);
        let Some(value_binders) = method_value_binders(method, own_decls) else {
            return outside("a method's own value binder is not an 'Int', 'Bool', or 'DType'");
        };
        let bound_type_binder = |binder: &mojito_ast::ast::TypeParam| {
            bound_binder(binder) && !value_binders.contains(&binder.name.as_str())
        };
        if !method.type_params.iter().all(|binder| {
            origin_binder(binder)
                || bound_binder(binder)
                || callable_binders.contains(&binder.name.as_str())
                || ((self.source_validation || trap_stub) && simd_wildcard_binder(binder))
        }) || (mojito_ast::ast::has_body_decorator(&method.decorators) && !is_static)
        {
            return outside(
                "the method has binders other than origins, bounded types, and scalar values, \
                 or decorators",
            );
        }
        let raises = method.raises || method.raises_type.is_some();
        // A struct's origin binder is erased from its declarations, and a
        // scalar value binder (`Array[T, length: Int]`) is read in the body
        // as `Self.length`, a runtime read of the reified parameter on the
        // erased path every such struct keeps: neither is substituted. A
        // type pack (`Tuple[*Ts]`) is fixed per instance as a type binder is:
        // the struct is specialized whole, its receiver's arguments are the
        // pack's elements, and an element the body reads by loop index is
        // fixed by the unrolling. Only source validation checks such a body.
        // A `DType` or vector value binder (`_SequentialRange[dtype]`,
        // `AHasher[key]`) keys a struct specialized whole: only source
        // validation checks its members, and each specialization's member
        // folds the values its trace names. The method's own `DType`
        // binders are among the declarations, folded as its scalar ones are.
        let plain_struct = decls.iter().all(|decl| {
            matches!(
                decl,
                ParamDecl::Type {
                    variadic: false,
                    callable_bound: None,
                    ..
                }
            ) || (self.source_validation
                && matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: true,
                        callable_bound: None,
                        ..
                    }
                ))
                || matches!(decl, ParamDecl::Value { ty, variadic: false, .. } if closed_scalar(ty))
                || matches!(decl, ParamDecl::Value { name, ty, variadic: false, .. }
                    if matches!(**ty, Ty::Dtype) && value_binders.contains(&name.as_str()))
                || matches!(decl, ParamDecl::Value { name, .. }
                    if callable_binders.contains(&name.as_str()))
                || (self.source_validation
                    && matches!(decl, ParamDecl::Value { ty, variadic: false, .. }
                        if matches!(**ty, Ty::Dtype | Ty::Simd { .. })))
        });
        if !plain_struct {
            return outside("a struct parameter is not a plain type or scalar value parameter");
        }
        // A `mut` or `ref` parameter is bound from its declared convention
        // alone, and is rooted at its own binding under every instance. What
        // its caller owes lives in the signature, which is checked per clone:
        // that holds an origin clause too, which names a binder, `self`, or
        // another parameter and never a struct parameter's type. A struct
        // that declares an origin parameter is outside `plain_struct`, and no
        // clone is minted for one.
        let plain_params = method.params.iter().all(|parameter| {
            (parameter.kind == mojito_ast::ast::ParamKind::Regular
                || (parameter.kind == mojito_ast::ast::ParamKind::Variadic
                    && parameter.convention == Some(ArgConvention::Var)))
                && matches!(
                    parameter.convention,
                    None | Some(
                        ArgConvention::Var
                            | ArgConvention::Deinit
                            | ArgConvention::Mut
                            | ArgConvention::Ref
                    )
                )
                && parameter
                    .default
                    .as_ref()
                    .is_none_or(|default| matches!(default.kind, ExprKind::None))
                && (parameter.origin.is_none() || parameter.convention == Some(ArgConvention::Ref))
        });
        if !plain_params {
            return outside(
                "a parameter has a default other than 'None', an origin on a convention other \
                 than 'ref', a keyword pack, a variadic not taken 'var', or an 'out' convention",
            );
        }
        let origin_parameter = method.type_params.iter().any(origin_binder)
            || method
                .params
                .iter()
                .any(|parameter| parameter.origin.is_some());
        let bound_binders = method.type_params.iter().any(bound_type_binder);
        let params_passed = |conventions: &[ArgConvention]| {
            method
                .params
                .iter()
                .filter(|parameter| {
                    parameter
                        .convention
                        .is_some_and(|convention| conventions.contains(&convention))
                })
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>()
        };
        let returns_reference = self
            .return_ref_contracts
            .last()
            .is_some_and(Option::is_some);
        let owned_parameter = method
            .params
            .iter()
            .any(|parameter| parameter.convention.is_some());
        let (pack_struct, packs) = struct_pack_collectors(method, decls);
        let desugars = self.with_desugars.borrow();
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            structs: &self.structs,
            traits: &self.traits,
            desugars: &desugars,
            desugar_depth: std::cell::Cell::new(0),
            error_binders: RefCell::new(Vec::new()),
            constants: &self.comptimes,
            vector_aliases: self.vector_aliases(),
            params: method
                .params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            callable_params: method
                .params
                .iter()
                .filter(|parameter| matches!(parameter.ty, mojito_ast::ast::Type::Func { .. }))
                .map(|parameter| parameter.name.as_str())
                .collect(),
            callable_calls: RefCell::new(Vec::new()),
            callable_binders: callable_binders.clone(),
            static_calls: RefCell::new(Vec::new()),
            packs,
            pack_struct,
            pack_binders: Vec::new(),
            loop_vars: RefCell::new(Vec::new()),
            values: value_binders.clone(),
            struct_values: struct_scalar_binders(&self.self_decls),
            struct_lanes: self.validated_struct_binders(struct_lane_binders),
            struct_vectors: self.validated_struct_binders(struct_vector_binders),
            print_calls: RefCell::new(Vec::new()),
            nested_depth: std::cell::Cell::new(0),
            runtime_loops: std::cell::Cell::new(0),
            borrowed_params: params_passed(&[ArgConvention::Mut, ArgConvention::Ref]),
            mut_params: params_passed(&[ArgConvention::Mut]),
            deinit_params: params_passed(&[ArgConvention::Deinit]),
            keyed: false,
            receiver: method.has_self,
            self_convention: method.self_convention,
            moved_result: (!returns_reference).then_some(ret_ty),
            reference_result: returns_reference.then_some(ret_ty),
            features: std::cell::Cell::new({
                let mut features = if plain_read && closed_scalar(ret_ty) && !owned_parameter {
                    MethodFeatures::default()
                } else {
                    MethodFeatures::STATEMENTS
                };
                if origin_parameter {
                    features = features.union(MethodFeatures::ORIGIN_PARAMETERS);
                }
                if bound_binders {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::BOUND_BINDERS);
                }
                if simd_binders {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::SIMD_BINDERS);
                }
                if !value_binders.is_empty() {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::VALUE_BINDERS);
                }
                if raises {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::RAISES);
                }
                if !callable_binders.is_empty() {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::CALLABLE_BINDERS);
                }
                features
            }),
            locals: RefCell::new(Vec::new()),
            nested_params: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
            subscripts: RefCell::new(Vec::new()),
            places: RefCell::new(Vec::new()),
            operators: RefCell::new(Vec::new()),
            bound_builtins: RefCell::new(Vec::new()),
            constructions: RefCell::new(Vec::new()),
            binders: method
                .type_params
                .iter()
                .filter(|binder| bound_type_binder(binder))
                .map(|binder| binder.name.as_str())
                .collect(),
            struct_binders: self.self_decls.iter().map(ParamDecl::id).collect(),
            binder_constructions: RefCell::new(Vec::new()),
            type_arguments: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
            lane_binders: method
                .type_params
                .iter()
                .filter(|binder| simd_wildcard_binder(binder))
                .flat_map(|binder| {
                    [
                        format!("{}.dtype", binder.name),
                        format!("{}.size", binder.name),
                    ]
                })
                .collect(),
            simd_to_bits: RefCell::new(Vec::new()),
            simd_casts: RefCell::new(Vec::new()),
            simd_lengths: RefCell::new(Vec::new()),
            pack_relocations: RefCell::new(Vec::new()),
            pack_spreads: RefCell::new(Vec::new()),
            element_constructions: RefCell::new(Vec::new()),
            stringified: RefCell::new(Vec::new()),
            copied_writes: RefCell::new(Vec::new()),
        };
        if !shape.block(&method.body) {
            return outside("the body is outside the method grammar");
        }
        let class = || {
            let features = shape.features.get();
            let coverage = TemplateCoverage::Certified(if features.is_empty() {
                TemplateClass::MethodScalarBody
            } else {
                TemplateClass::MethodBody(features)
            });
            (coverage, shape.grammar_notes())
        };
        let Some(facts) = facts else {
            return class();
        };
        if let Some(what) = self.method_facts_refusal(&shape, facts) {
            return outside(what);
        }
        class()
    }

    /// Why a method body the grammar admitted still falls outside its class,
    /// judged on the facts its check recorded; `None` when none applies.
    fn method_facts_refusal(
        &self,
        shape: &BodyShape<'_>,
        facts: &CheckedBodyFacts,
    ) -> Option<&'static str> {
        if !shape.references_recorded(facts) {
            return Some("a reference is yielded or kept outside the method grammar");
        }
        if stray_method_call(facts, shape) {
            return Some("the body calls something other than a trivial method");
        }
        // A residue the body publishes or reads is republished for an
        // instance, but only a body that calls or forwards its own callable
        // parameter records one the recipe covers.
        let residue = !facts.call_throughs.is_empty() || !facts.call_through_reads.is_empty();
        if residue
            && !shape.holds(MethodFeatures::CALLABLE_PARAMETERS)
            && !shape.holds(MethodFeatures::CALLABLE_BINDERS)
        {
            return Some("a keyed body publishes or reads a call-through residue");
        }
        // A residue naming a compile-time callable names one of the method's
        // own binders, which every clone keeps under its name.
        let own_binder_residues = facts
            .call_throughs
            .iter()
            .all(|residue| match &residue.callee {
                mojito_checked::checked::CallThroughCallee::RuntimeParam(_) => true,
                mojito_checked::checked::CallThroughCallee::ValueParam(name) => {
                    shape.callable_binders.contains(&name.as_str())
                }
            });
        if !own_binder_residues {
            return Some("a residue names a compile-time callable that is not the method's own");
        }
        if facts.replays_transfers()
            && (shape.keyed || !shape.holds(MethodFeatures::REPLAYED_TRANSFERS))
        {
            return Some("a keyed body replays a transfer summary");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| effect_derives(effects));
        let binder_constructions = shape.binder_constructions.borrow();
        let type_arguments = shape.type_arguments.borrow();
        let adjustments_derive = facts.operation_adjustments.iter().all(|(id, adjustment)| {
            binder_constructions.contains(id)
                || type_arguments.contains(id)
                || adjustment_derives(adjustment)
                || matches!(adjustment,
                    mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(target)
                        if shape.struct_lane_simd(target))
        });
        if !effects_closed || !adjustments_derive {
            return Some("an expression has an effect or an adjustment with no recipe");
        }
        let wrote_through_origin = self
            .parametric_write_frames
            .borrow()
            .last()
            .is_some_and(|frame| !frame.is_empty());
        if wrote_through_origin {
            return Some("the body writes through an origin parameter");
        }
        None
    }
}

/// The features a [`TemplateClass::FunctionBody`] may hold: runtime
/// statements, whole values moved or copied between a parameter, a local, an
/// argument, and the result, a runtime `for` over a place, a condition
/// tested through `__bool__`, a `SIMD` construction or lane read, a
/// construction of a declared struct, a method call on a local or a parameter whose contract
/// is a value contract or which dispatches through the parameter's bound, a
/// keyword slice of a closed local, the stringify builtin, `external_call`,
/// an operator over a closed struct type, a pack element's default
/// construction, and a `raises` declaration. Each
/// recipe is a method body's, on a body without a receiver.
const FUNCTION_FEATURES: MethodFeatures = MethodFeatures::STATEMENTS
    .union(MethodFeatures::OPAQUE_MOVES)
    .union(MethodFeatures::VALUE_ARGUMENTS)
    .union(MethodFeatures::ITERATION)
    .union(MethodFeatures::TRUTHINESS)
    .union(MethodFeatures::RAISES)
    .union(MethodFeatures::OWNED_PARAMETERS)
    .union(MethodFeatures::CONSUMING_CALLS)
    .union(MethodFeatures::POINTER_SLOTS)
    .union(MethodFeatures::SIMD_CONSTRUCTIONS)
    .union(MethodFeatures::SIMD_INTRINSICS)
    .union(MethodFeatures::CONSTRUCTIONS)
    .union(MethodFeatures::SIBLING_CALLS)
    .union(MethodFeatures::TUPLE_UNPACKS)
    .union(MethodFeatures::TRY_STATEMENTS)
    .union(MethodFeatures::SLICE_VIEWS)
    .union(MethodFeatures::STRINGIFY)
    .union(MethodFeatures::STRING_BUILTINS)
    .union(MethodFeatures::FOREIGN_CALLS)
    .union(MethodFeatures::CLOSED_OPERATORS)
    .union(MethodFeatures::ELEMENT_CONSTRUCTIONS)
    .union(MethodFeatures::BOUND_DISPATCH);

/// Whether a parameter default is a literal, or a negated numeric one. The
/// callee evaluates its own default at the parameter's type in the
/// instance's signature, so it records nothing in the body and converts
/// alike under every instance, as a method call's omitted argument does.
fn literal_default(default: &Expr) -> bool {
    match &default.kind {
        ExprKind::Prefix(mojito_ast::ast::PrefixOp::Neg, operand) => {
            matches!(operand.kind, ExprKind::Int(_) | ExprKind::Float(_))
        }
        kind => matches!(
            kind,
            ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Str(_)
                | ExprKind::None
        ),
    }
}

/// The module-scope declaration a template's direct call selected: the
/// binding its call occurrence resolved to.
/// Whether a method body selected a callee, or read an effect summary, that
/// its grammar did not admit.
///
/// The grammar admitted every method call it judged closed, every call an
/// element store embeds, every in-place update of a place, every
/// construction, every call through a callable parameter, and every direct
/// call `method_direct_calls` names.
fn stray_method_call(facts: &CheckedBodyFacts, shape: &BodyShape<'_>) -> bool {
    let targets: Vec<&str> = facts
        .selected_calls
        .iter()
        .chain(&facts.inplace_updates)
        .map(|(_, call)| call.contract.target.as_str())
        .chain(facts.augmented_subscripts.iter().flat_map(|(_, store)| {
            store
                .getter
                .iter()
                .chain(&store.inplace)
                .map(|call| call.contract.target.as_str())
        }))
        .collect();
    let constructions = shape.constructions.borrow();
    let callable_calls = shape.callable_calls.borrow();
    let direct_calls = method_direct_calls(facts);
    let nested_calls = nested_def_calls(facts);
    let static_calls = shape.static_calls.borrow();
    // An admitted operator's target is its reflected dunder's
    // ([`BodyShape::operator`]).
    let operators = shape.operators.borrow();
    // The stringify builtin's overload target is chosen by the closed
    // argument type alone ([`BodyShape::stringify`]).
    let stringified = shape.stringified.borrow();
    if !direct_calls.is_empty() {
        shape.holds(MethodFeatures::DIRECT_CALLS);
    }
    let admitted_call = |id: &OccurrenceId| {
        facts.selected_calls.iter().any(|(call, _)| call == id)
            || facts.inplace_updates.iter().any(|(update, _)| update == id)
            || constructions.contains(id)
            || callable_calls.contains(id)
            || direct_calls.iter().any(|(call, _)| call == id)
            || nested_calls.iter().any(|(call, _)| call == id)
            || static_calls.iter().any(|(call, _)| call == id)
    };
    // A call through a bound, at an occurrence or embedded in a store,
    // reads the summaries of every conformer's method of that name.
    let conformer_copy = |callee: &str| dispatched_conformer(&targets, callee);
    facts
        .call_parameters
        .iter()
        .any(|(id, _)| !admitted_call(id))
        || !facts
            .generic_instantiations
            .iter()
            .all(|(id, instantiation)| {
                direct_calls.iter().any(|(call, _)| call == id)
                    || (callable_calls.contains(id)
                        && shape
                            .callable_binders
                            .contains(&instantiation.callee.as_str()))
            })
        || !facts
            .overload_targets
            .iter()
            .all(|(id, _)| admitted_call(id) || operators.contains(id) || stringified.contains(id))
        || !summary_callees(facts).all(|callee| {
            targets.contains(&callee.as_str())
                || direct_calls.iter().any(|(_, direct)| direct == callee)
                || nested_calls.iter().any(|(_, nested)| nested == callee)
                || static_calls.iter().any(|(_, member)| member == callee)
                || conformer_copy(callee)
                || shape.callable_params.contains(&callee.as_str())
                || shape.callable_binders.contains(&callee.as_str())
        })
}

/// Whether `callee` is a conformer's method a call through a bound among
/// `targets` read the summaries of: such a call reads every conformer's
/// method of its name, one key per conformer (`Struct.method`, or the
/// overload symbol `Struct.method$ov$…`), none of them a target. An
/// instance reads its own witness's summaries again
/// ([`Checker::realize_bound_dispatch`]).
fn dispatched_conformer(targets: &[&str], callee: &str) -> bool {
    let member = |name: &'_ str| name.split('$').next().unwrap_or(name).to_string();
    mojito_symbol::symbol::split_method_symbol(callee).is_some_and(|(_, called)| {
        targets
            .iter()
            .filter(|target| mojito_symbol::symbol::is_trait_dispatch_symbol(target))
            .any(|target| {
                member(
                    mojito_symbol::symbol::split_method_symbol(target)
                        .map_or(*target, |(_, method)| method),
                ) == member(called)
            })
    })
}

/// The direct calls a method body makes of a module-scope function that
/// takes only closed scalars, or reads a value at one of its own binders,
/// each with its callee.
///
/// The call selects the same declaration under every instance, or the same
/// member of an overload set, which ranks only the closed argument types,
/// and binds its arguments by value at types no substitution changes. A
/// generic callee's application (`unsafe_alloc[Self.T](n)`) is recorded with
/// the template's arguments, which the instance substitutes, so an instance
/// realizes it as a function template's direct call
/// ([`Checker::realize_direct_call`]): the application's existing clone, or
/// the one the elaborator retargeted the call to. A read parameter typed by
/// the callee's own binder (`hash(e)`) is declared in the callee's binder
/// scope, which no substitution reaches: the argument binds it exactly, and
/// only the application's argument changes per instance
/// ([`BodyShape::generic_call_place`]). A call returning a reference may
/// also bind a `ref` parameter to a named place it keeps
/// ([`BodyShape::module_reference_call`]).
fn method_direct_calls(facts: &CheckedBodyFacts) -> Vec<(OccurrenceId, &str)> {
    facts
        .call_parameters
        .iter()
        .filter(|(id, parameters)| {
            let application = fact_at(&facts.generic_instantiations, *id);
            !facts.selected_calls.iter().any(|(call, _)| call == id)
                && application.is_none_or(|application| application.variadic.is_none())
                && parameters.iter().all(|parameter| {
                    (parameter.convention.is_none()
                        || (parameter.convention == Some(mojito_ast::ast::ArgConvention::Ref)
                            && fact_at(&facts.reference_results, *id).is_some()))
                        && (!mojito_types::types::is_symbolic(&parameter.ty)
                            || (application.is_some() && matches!(parameter.ty, Ty::Param { .. })))
                })
        })
        .filter_map(|(id, _)| template_callee(facts, *id).map(|callee| (*id, callee)))
        .collect()
}

/// The type pack a method's struct declares, and the method's variadic
/// parameters collecting it (`var *args: *Self.Ts`).
fn struct_pack_collectors<'m, 'd>(
    method: &'m mojito_ast::ast::Method,
    decls: &'d [ParamDecl],
) -> (Option<&'d str>, Vec<&'m str>) {
    let pack = decls.iter().find_map(|decl| match decl {
        ParamDecl::Type {
            name,
            variadic: true,
            ..
        } => Some(name.trim_start_matches('*')),
        _ => None,
    });
    let collectors = method
        .params
        .iter()
        .filter(|parameter| {
            let collected = match &parameter.ty {
                mojito_ast::ast::Type::SelfParam(name) | mojito_ast::ast::Type::Named(name, _) => {
                    name.strip_prefix('*')
                }
                _ => None,
            };
            parameter.kind == mojito_ast::ast::ParamKind::Variadic
                && collected.is_some()
                && collected == pack
        })
        .map(|parameter| parameter.name.as_str())
        .collect();
    (pack, collectors)
}

/// The names of a struct's `DType` and `Int` binders, which key a struct
/// specialized whole and name its members' symbolic lane
/// (`Scalar[Self.dtype]`, `SIMD[dt, Self.n]`).
fn struct_lane_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if matches!(**ty, Ty::Dtype | Ty::Int) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// A struct's scalar value binders of a closed type (`rows: Int`).
fn struct_scalar_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if closed_scalar(ty) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// A struct's vector value binders of a closed type (`key: U256`).
fn struct_vector_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if grammar_scalar(ty) && matches!(**ty, Ty::Simd { .. }) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// The names of a method's own value binders (`[n: Int]`, `[dt: DType]`),
/// given its own declarations, or `None` when one is not a plain `Int`,
/// `Bool`, or `DType`. The
/// parser spells such a binder's type as a bound (`n: Int`), so only its
/// declaration tells it from a trait-bounded type binder ([`bound_binder`]).
/// A compile-time callable binder is apart ([`method_callable_binders`]).
fn method_value_binders<'m>(
    method: &'m mojito_ast::ast::Method,
    own_decls: &[ParamDecl],
) -> Option<Vec<&'m str>> {
    let callables = method_callable_binders(method, own_decls);
    method
        .type_params
        .iter()
        .filter(|binder| !callables.contains(&binder.name.as_str()))
        .filter_map(|binder| {
            own_decls
                .iter()
                .find(|decl| decl.name().trim_start_matches('*') == binder.name)
                .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
                .map(|decl| (binder, decl))
        })
        .map(|(binder, decl)| {
            matches!(decl, ParamDecl::Value {
                ty,
                default: None,
                infer_only: false,
                variadic: false,
                ..
            } if matches!(**ty, Ty::Int | Ty::Bool | Ty::Dtype))
            .then_some(binder.name.as_str())
        })
        .collect()
}

/// The names of a method's own compile-time callable binders
/// (`elt_handler: def[index: Int](var element: Self.Ts[index])`), given its
/// own declarations: a callable value with no default, which every clone
/// keeps ([`MethodFeatures::CALLABLE_BINDERS`]).
fn method_callable_binders<'m>(
    method: &'m mojito_ast::ast::Method,
    own_decls: &[ParamDecl],
) -> Vec<&'m str> {
    method
        .type_params
        .iter()
        .filter(|binder| callable_binder(binder))
        .filter(|binder| {
            own_decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Value {
                    name,
                    ty,
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    ..
                } if *name == binder.name
                    && matches!(**ty, Ty::Func { .. } | Ty::GenericFunc { .. }))
            })
        })
        .map(|binder| binder.name.as_str())
        .collect()
}

/// Every callee whose transfer summary a body read at a call: those observed
/// empty and those replayed.
fn summary_callees(facts: &CheckedBodyFacts) -> impl Iterator<Item = &String> {
    facts
        .effect_free_callees
        .iter()
        .chain(facts.transfer_reads.iter().map(|(callee, _)| callee))
}

/// Whether a call's recorded effect derives: raising is the one it may
/// carry.
///
/// The raised type substitutes (`substituted_facts`), and the judgment an
/// instance repeats at the call — whether that type is the declared error
/// type, or the handler's in a `with` desugar — compares two functions of the
/// same parameters, so it holds under every substitution the template's
/// holds under.
const fn effect_derives(effects: &mojito_checked::checked::EffectFacts) -> bool {
    !effects.may_suspend && !effects.diverges
}
