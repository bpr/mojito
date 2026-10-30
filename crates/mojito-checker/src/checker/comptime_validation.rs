//! Source validation of compile-time control flow (`validate_comptime_templates`):
//! a `comptime if` condition is typed as a compile-time `Bool`, every arm and
//! `comptime for` body is checked in its own scope with the declaration's
//! parameters symbolic, and function-local `comptime` bindings the elaborator
//! would otherwise consume are bound here. Extracted from `checker.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ParamArg, SourceType};
use mojito_types::conformance::Conformance;
use mojito_types::types::TyKind;
use mojito_types::types::{ConstraintOperand, GenericConstraint};

/// The bundled `ParameterList`'s linked name, its implicit public identity.
pub(super) const PARAMETER_LIST: &str = "ParameterList";

/// The bundled `TypeList`'s linked name, its implicit public identity.
pub(super) const TYPE_LIST: &str = "TypeList";

/// A local `comptime` binding of a display the elaborator evaluates per
/// instance (`comptime L = [n, n + 1]`).
#[derive(Debug, Clone)]
pub(in crate::checker) struct BoundDisplay {
    /// The display's own type, which a runtime read would materialize.
    ty: Ty,
    /// What the binding denotes in a type or a parameter argument: the
    /// application of the function MIR lifts for the display to the binders
    /// in scope, a list of the display's elements.
    sequence: ParamExpr,
    /// Whether a subscript reads an element by position, as a list
    /// display's does.
    positional: bool,
}

impl BoundDisplay {
    /// The parameter expression of element `index` of the binding
    /// (`L[0]`), for a display a subscript indexes by position.
    pub(in crate::checker) fn element(
        &self,
        context: &ParamContext,
        index: &ParamExpr,
    ) -> Option<Result<ParamExpr, TypeError>> {
        self.positional.then(|| {
            context
                .list_get(&self.sequence, index)
                .map_err(super::annotations::param_error)
        })
    }

    /// The parameter expression of the binding's length (`len(L)`).
    pub(in crate::checker) fn length(
        &self,
        context: &ParamContext,
    ) -> Result<ParamExpr, TypeError> {
        context
            .list_length(&self.sequence)
            .map_err(super::annotations::param_error)
    }
}

/// A compile-time expression the check named a lifted function for
/// ([`Checker::lifted_application`]), with what tells it from another
/// occurrence of the same syntax: the binders it is applied to and the
/// bindings its names resolve to.
#[derive(Debug)]
pub(in crate::checker) struct LiftedApplication {
    owner: String,
    expression: Expr,
    binders: Vec<ParamExpr>,
    bindings: Vec<Option<mojito_types::origin::OwnerId>>,
    application: ParamExpr,
}

/// Where a binder-free `comptime` binding is declared, which decides
/// whether a collection display initializer is lifted
/// ([`super::lifts_comptime_value`]): a module constant's is, typed by its
/// annotation (`comptime E: Dict[String, Int] = {}`), while a local display
/// binding keeps its display route and only a local constructor call is
/// lifted.
#[derive(Clone, Copy, Debug)]
pub(super) enum ConstantScope<'a> {
    Module(Option<&'a SourceType>),
    Local,
}

/// The compile-time test a condition belongs to, which words its
/// diagnostics as the pin does for each.
#[derive(Clone, Copy, Debug)]
pub(super) enum ComptimeTest {
    If,
    Assert,
    /// A `comptime match` subject, and each test its patterns compile to.
    MatchSubject,
}

impl ComptimeTest {
    pub(super) const fn dynamic_value(self) -> &'static str {
        match self {
            Self::If => "cannot use a dynamic value in 'comptime if' condition",
            Self::Assert => "cannot use a dynamic value in 'comptime assert' expression",
            Self::MatchSubject => "cannot use a dynamic value in 'match' subject",
        }
    }

    const fn context(self) -> &'static str {
        match self {
            Self::If => "comptime if condition",
            Self::Assert => "comptime assert condition",
            Self::MatchSubject => "match subject",
        }
    }

    const fn raising_call(self) -> &'static str {
        match self {
            Self::If => "cannot call raising function in 'comptime if' condition",
            Self::Assert => "cannot call raising function in 'comptime assert' expression",
            Self::MatchSubject => "cannot call raising function in boolean condition",
        }
    }
}

/// A call argument that forwards a variadic pack whole (`*args`, `*args^`)
/// while the pack is still a parameter.
pub(super) struct ForwardedPack {
    /// The collector binding the spread names.
    pub(super) binding: String,
    /// The pack as its bounded binder (`*Ts` with its declared bounds).
    pub(super) pack: Ty,
    /// Whether the spread carries the `^`.
    pub(super) transferred: bool,
    /// Whether the collector was declared `var`.
    pub(super) owned: bool,
}

/// A callable a compile-time call applies by name
/// ([`Checker::called_application`]): a module `def`, a static method of a
/// struct, a struct's `__init__`, or an `@always_inline("builtin")` method
/// taking `self`. Its own compile-time parameters are any but a pack, an
/// origin, or a callable; each runtime parameter is taken `read` or `var`
/// at a type a parameter expression holds ([`parameter_value_type`]) —
/// a scalar, a `String` (the domain's `StringLiteral`), a `DType`, a struct,
/// its struct's `Self` or own parameters, or one of its own type
/// parameters — and none is a collector; and it returns a scalar or a
/// named type. A call binds its parameters as a runtime call's do
/// ([`Checker::resolve_use_params`]), its `where` clauses included.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::checker) struct ApplicableFunction {
    /// The MIR declaration the elaborator runs.
    symbol: String,
    /// Whether it is a static method or an `__init__` of a generic struct,
    /// which is applied to the instance it is called on, or constructs,
    /// before any other argument.
    on_instance: bool,
    /// What its `self` is, if it has one.
    self_slot: SelfSlot,
    /// The names of the callable's own compile-time parameters, in
    /// declared order.
    binders: Vec<String>,
    /// Those parameters classified as the callable's own check classifies
    /// them, and its result over them
    /// ([`Checker::classify_applicable_signatures`]); `None` until they
    /// classify, and a callable whose parameters do not is applied by no
    /// call.
    signature: Option<AppliedSignature>,
    /// Its runtime parameters, in declared order.
    params: Vec<ApplicableParameter>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// The declared result: a scalar, a `DType`, a named struct, one of its
    /// own type parameters, or `Self`, which a call resolves at the instance
    /// it constructs or is called on.
    result: SourceType,
    /// Whether it is declared `raises`, which no type or parameter argument
    /// may call.
    raises: bool,
    /// A refined initializer's `out self` annotation (`out self:
    /// Scalar[target_dtype]`): the type a construction's instance must
    /// unify with, inferring the initializer's own parameters, as the pin
    /// resolves the overload.
    refined_self: Option<SourceType>,
    /// Whether it is an `@implicit` initializer.
    implicit: bool,
    /// The inline level its decorators name.
    pub(in crate::checker) inline: mojito_checked::declaration::InlineLevel,
    /// An `@always_inline("builtin")` body folded over
    /// [`super::annotations::builtin_body_binder`] slots, one per argument
    /// in [`Checker::called_application`]'s order, which a call substitutes
    /// its arguments into ([`Checker::fold_builtin_bodies`]).
    pub(in crate::checker) folded_body: Option<FoldedBody>,
    /// Its `where` clauses as the declaration spells them.
    where_clauses: Vec<Expr>,
    /// Those clauses compiled over the same argument slots and its struct's
    /// binders ([`Checker::compile_application_constraints`]); `None` until
    /// they compile, and a callable whose clauses do not is applied by no
    /// call.
    pub(in crate::checker) constraints: Option<Vec<GenericConstraint>>,
}

/// A folded `@always_inline("builtin")` body
/// ([`ApplicableFunction::folded_body`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::checker) enum FoldedBody {
    /// A function's or static method's result.
    Value(ParamExpr),
    /// The fields an `__init__` stores, in declaration order: a call is the
    /// struct value over them ([`mojito_types::param_expr::ParamContext::struct_value`]).
    Fields(Vec<(String, ParamExpr)>),
}

impl ApplicableFunction {
    /// The MIR declaration the elaborator runs.
    pub(in crate::checker) fn symbol(&self) -> &str {
        &self.symbol
    }

    /// The compile-time parameters, then `self`, then the runtime
    /// parameters, in [`Checker::called_application`]'s argument order, each
    /// declared as its slot's own binder
    /// ([`super::annotations::builtin_body_binder`]): a type parameter a type
    /// binder, and every other slot a value binder typed over them, `self`
    /// and a `Self` parameter typed `owner`: the struct template in its own
    /// scope, the instance at a call. A method's struct parameters are no
    /// slot: its body reads them as its struct's own binders, which a call
    /// binds from the instance. `None` for a slot typed `Self` without an
    /// `owner`, or a callable whose parameters have not classified.
    pub(in crate::checker) fn argument_slots(&self, owner: Option<&Ty>) -> Option<Vec<ParamDecl>> {
        let signature = self.signature.as_ref()?;
        let binder = |slot: usize, name: &str| {
            super::annotations::builtin_body_binder(&self.symbol, slot, name)
        };
        let subst: TySubst = signature
            .decls
            .iter()
            .enumerate()
            .filter_map(|(slot, decl)| {
                let ParamDecl::Type {
                    id,
                    bounds,
                    callable_bound,
                    ..
                } = decl
                else {
                    return None;
                };
                let ty = TyKind::Param {
                    binder: binder(slot, decl.name()),
                    bounds: bounds.clone(),
                    callable_bound: callable_bound.clone(),
                };
                Some((id.clone(), ty.into()))
            })
            .collect();
        let receiver =
            (self.self_slot == SelfSlot::Receiver).then(|| Some(("self", Some(owner?.clone()))));
        let slots =
            signature
                .decls
                .iter()
                .map(|decl| match decl {
                    ParamDecl::Value { name, ty, .. } => Some((name.as_str(), Some(ty.clone()))),
                    ParamDecl::Type { name, .. } => Some((name.as_str(), None)),
                })
                .chain(receiver)
                .chain(self.params.iter().map(|param| {
                    Some((param.name.as_str(), Some(param.ty.resolved(owner)?.clone())))
                }))
                .collect::<Option<Vec<_>>>()?;
        Some(
            slots
                .into_iter()
                .enumerate()
                .map(|(slot, (name, ty))| {
                    let id = binder(slot, name).id;
                    if let Some(ty) = ty {
                        return ParamDecl::Value {
                            id,
                            name: name.to_string(),
                            ty: substitute(&ty, &subst),
                            default: None,
                            callable_default: None,
                            infer_only: false,
                            variadic: false,
                            constraints: Vec::new(),
                        };
                    }
                    let mut decl = signature.decls[slot].clone();
                    if let ParamDecl::Type {
                        id: declared,
                        default,
                        constraints,
                        ..
                    } = &mut decl
                    {
                        *declared = id;
                        *default = None;
                        constraints.clear();
                    }
                    decl
                })
                .collect(),
        )
    }

    /// Whether it is an `__init__`, whose result is the instance it
    /// constructs.
    pub(in crate::checker) fn constructs(&self) -> bool {
        self.self_slot == SelfSlot::Constructed
    }

    /// Whether it is a method taking `self`.
    fn takes_self(&self) -> bool {
        self.self_slot == SelfSlot::Receiver
    }

    /// Whether it is declared `raises`.
    pub(in crate::checker) const fn raises(&self) -> bool {
        self.raises
    }

    /// The declared result.
    pub(in crate::checker) const fn result(&self) -> &SourceType {
        &self.result
    }

    /// Its `where` clauses as the declaration spells them.
    pub(in crate::checker) fn where_clauses(&self) -> &[Expr] {
        &self.where_clauses
    }
}

/// A callable [`applicable_functions`] lists, with the name a call spells
/// and the body it declares.
pub(super) struct ApplicableDeclaration<'a> {
    pub(super) name: String,
    pub(super) function: ApplicableFunction,
    pub(super) body: &'a [Stmt],
    /// The struct statement declaring a static method or an `__init__`;
    /// `None` for a module `def`.
    pub(super) owner: Option<&'a Stmt>,
    /// The method it is, a synthesized `__init__` none.
    method: Option<&'a mojito_ast::ast::Method>,
    /// Its own compile-time parameters as it declares them.
    type_params: &'a [mojito_ast::ast::TypeParam],
}

/// The rejection of a raw parameter list (`KGENParamList[T]`, a pack's
/// `vs.values`) read as a runtime value, which the pin materializes and
/// Mojito does not.
pub(super) fn raw_list_runtime_read(meta: &mojito_types::param_expr::MetaTy) -> TypeError {
    let spelled = meta
        .value_ty()
        .map_or_else(|| meta.to_string(), |ty| ty.to_string());
    TypeError::Unsupported(format!(
        "a raw parameter list ('{spelled}') read at run time is not supported; \
         read it through 'ParameterList[...]'"
    ))
}

/// The rejection of `s.value` over a value-less `StringLiteral` (a bare
/// run-time `s: StringLiteral` argument), whose text no type carries, where
/// the pin's argument is a `StringLiteral[_]` whose parameter it reads.
pub(super) fn value_less_literal_value() -> TypeError {
    TypeError::Unsupported(
        "the 'value' of a bare 'StringLiteral' argument is not supported".to_string(),
    )
}

/// The value and type of the attribute `` __mlir_attr.`text` ``.
///
/// An untyped attribute is upstream's `!kgen.deferred` constant
/// ([`CtValue::MlirAttr`]). A typed one, whose type follows a colon outside
/// the attribute's brackets, is a constant of that type: a
/// `` `#kgen.simd<…> : !kgen.scalar<dt>` `` (or `!kgen.simd<n, dt>`) lane
/// constant of the storage type, held as its vector, and an
/// `` `7 : index` `` builtin `index`. Any other typed attribute is outside
/// the subset.
pub(super) fn mlir_attribute(text: &str) -> Result<(CtValue, Ty), TypeError> {
    let mut depth = 0_usize;
    let colon = text.char_indices().find_map(|(at, c)| {
        match c {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        (c == ':' && depth == 0).then_some(at)
    });
    let Some(colon) = colon else {
        return Ok((
            CtValue::MlirAttr(text.to_string()),
            TyKind::KgenDeferred.into(),
        ));
    };
    let (body, ty) = (text[..colon].trim(), text[colon + 1..].trim());
    typed_mlir_attribute(body, ty).ok_or_else(|| {
        TypeError::Unsupported(format!(
            "the typed attribute '__mlir_attr.`{text}`' is outside the subset; only an untyped \
             '{}' attribute, a '#kgen.simd<...>' constant of a '!kgen.scalar<dtype>' or \
             '!kgen.simd<width, dtype>' type, an integer of type 'index', and a \
             '#kgen.target<...>' or '#kgen.param.expr<current_target>' of type \
             '!kgen.target' are read",
            mojito_types::types::KGEN_DEFERRED_TYPE_SPELLING
        ))
    })
}

/// [`mlir_attribute`] as a parameter expression, beside its type: KGEN's
/// `#kgen.param.expr<current_target>` is the compilation target's
/// application ([`ParamContext::current_target`]), which the elaborator
/// answers, and any other attribute is the constant it spells.
pub(super) fn mlir_attribute_expr(
    context: &ParamContext,
    text: &str,
) -> Result<(ParamExpr, Ty), TypeError> {
    let shape: String = text.split_whitespace().collect();
    if shape == "#kgen.param.expr<current_target>:!kgen.target" {
        return Ok((context.current_target(), TyKind::KgenTarget.into()));
    }
    let (value, ty) = mlir_attribute(text)?;
    let value = context.constant(value).map_err(param_error)?;
    Ok((value, ty))
}

/// The `name` of an `` __mlir_type.`name` `` spelling.
pub(super) fn mlir_source_name(ty: &SourceType) -> Option<&str> {
    match ty {
        SourceType::Assoc { base, name, args }
            if args.is_empty()
                && matches!(base.as_ref(), SourceType::Named(base, base_args)
                    if base == "__mlir_type" && base_args.is_empty()) =>
        {
            Some(name)
        }
        _ => None,
    }
}

/// The rejection of an untyped `__mlir_attr` read at run time outside a
/// body that returns `!kgen.deferred`, which the pin elides and Mojito does
/// not hold.
pub(super) fn kgen_deferred_runtime_read() -> TypeError {
    TypeError::Unsupported(format!(
        "a '{}' value read at run time is not supported; only a compile-time position or a \
         function returning one reads it",
        mojito_types::types::KGEN_DEFERRED_TYPE_SPELLING
    ))
}

/// The rejection of a run-time call whose result is `!kgen.deferred`,
/// which the pin cannot lower.
pub(super) fn kgen_deferred_runtime_call(callee: &str) -> TypeError {
    TypeError::Unsupported(format!(
        "failed to convert call result type: '{callee}' returns '{}', which only a \
         compile-time position may call",
        mojito_types::types::KGEN_DEFERRED_TYPE_SPELLING
    ))
}

/// The callables a compile-time call applies by name
/// ([`Checker::called_application`]), every overload under the name a call
/// spells: `h` for a module `def`, `S.f` for a static method.
pub(super) fn applicable_functions(
    stmts: &[Stmt],
    overloads: &impl mojito_symbol::symbol::OverloadLookup,
    constants: &dyn mojito_symbol::symbol::ModuleNames,
) -> HashMap<String, Vec<ApplicableFunction>> {
    let mut functions = HashMap::<String, Vec<ApplicableFunction>>::new();
    for declaration in applicable_declarations(stmts, overloads, constants) {
        functions
            .entry(declaration.name)
            .or_default()
            .push(declaration.function);
    }
    functions
}

/// [`applicable_functions`]' callables in source order, each with its body.
pub(super) fn applicable_declarations<'a>(
    stmts: &'a [Stmt],
    overloads: &impl mojito_symbol::symbol::OverloadLookup,
    constants: &dyn mojito_symbol::symbol::ModuleNames,
) -> Vec<ApplicableDeclaration<'a>> {
    let mut declarations = Vec::new();
    for statement in stmts {
        match &statement.kind {
            StmtKind::Def {
                name,
                decorators,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures: None,
                raises,
                ret: Some(ret),
                where_clauses,
                body,
                ..
            } => {
                let shape = ApplicableShape {
                    decorators,
                    type_params,
                    params,
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    ret,
                    where_clauses,
                };
                let symbol = mojito_symbol::symbol::lowered_def_name(
                    name,
                    type_params,
                    params,
                    overloads,
                    constants,
                );
                declarations.extend(shape.applicable(symbol, false, SelfSlot::None).map(
                    |function| ApplicableDeclaration {
                        name: name.clone(),
                        function,
                        body,
                        owner: None,
                        method: None,
                        type_params,
                    },
                ));
            }
            StmtKind::Struct {
                name,
                type_params,
                methods,
                fields,
                fieldwise_init,
                ..
            } => {
                let on_instance = !type_params.is_empty();
                // A construction calls its `__init__` under the struct's
                // name, its `out self` the result.
                let constructed = SourceType::Named(name.clone(), Vec::new());
                let mut list =
                    |source: String,
                     function: Option<ApplicableFunction>,
                     method: Option<&'a mojito_ast::ast::Method>| {
                        declarations.extend(function.map(|function| ApplicableDeclaration {
                            name: source,
                            function,
                            body: method.map_or(&[], |method| method.body.as_slice()),
                            owner: Some(statement),
                            method,
                            type_params: method.map_or(&[], |method| method.type_params.as_slice()),
                        }));
                    };
                if mojito_checked::unit::synthesizes_fieldwise_constructor(
                    *fieldwise_init,
                    methods.iter().map(|method| method.name.as_str()),
                ) {
                    let params: Vec<_> = fields
                        .iter()
                        .map(|field| mojito_ast::ast::FnParam {
                            name: field.name.clone(),
                            ty: field.ty.clone(),
                            default: None,
                            kind: mojito_ast::ast::ParamKind::Regular,
                            convention: None,
                            origin: None,
                        })
                        .collect();
                    let shape = ApplicableShape {
                        decorators: &[],
                        type_params: &[],
                        params: &params,
                        positional_only: None,
                        keyword_only: None,
                        raises: false,
                        ret: &constructed,
                        where_clauses: &[],
                    };
                    let symbol = format!("{name}.__init__");
                    list(
                        name.clone(),
                        shape.applicable(symbol, on_instance, SelfSlot::Constructed),
                        None,
                    );
                }
                for method in methods {
                    if method.name == "__init__"
                        && method.self_convention == Some(ArgConvention::Out)
                    {
                        let shape = ApplicableShape {
                            decorators: &method.decorators,
                            type_params: &method.type_params,
                            params: &method.params,
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            ret: &constructed,
                            where_clauses: &method.where_clauses,
                        };
                        let symbol = mojito_symbol::symbol::lowered_method_name(
                            &format!("{name}.__init__"),
                            type_params,
                            mojito_symbol::symbol::MethodShape::of(method),
                            overloads,
                            constants,
                        );
                        let function = shape
                            .applicable(symbol, on_instance, SelfSlot::Constructed)
                            .map(|function| ApplicableFunction {
                                refined_self: method.self_type.clone(),
                                ..function
                            });
                        list(name.clone(), function, Some(method));
                        continue;
                    }
                    // A method with `self` applies only when builtin, its
                    // receiver taken `imm`, as the pin folds it, or when it
                    // returns `!kgen.deferred` and so runs only at compile
                    // time (`FastMathFlag._mlir_attr`); an ordinary one is a
                    // run-time call.
                    let Some(ret) = method.ret.as_ref().filter(|ret| {
                        !method.has_self
                            || ((is_mlir_source(ret, "!kgen.deferred")
                                || matches!(
                                    mojito_checked::declaration::InlineLevel::of(
                                        &method.decorators
                                    ),
                                    Ok(mojito_checked::declaration::InlineLevel::Builtin)
                                ))
                                && matches!(
                                    method.self_convention,
                                    None | Some(ArgConvention::Imm)
                                ))
                    }) else {
                        continue;
                    };
                    let shape = ApplicableShape {
                        decorators: &method.decorators,
                        type_params: &method.type_params,
                        params: &method.params,
                        positional_only: method.positional_only,
                        keyword_only: method.keyword_only,
                        raises: method.raises,
                        ret,
                        where_clauses: &method.where_clauses,
                    };
                    let source = format!("{name}.{}", method.name);
                    let symbol = mojito_symbol::symbol::lowered_method_name(
                        &source,
                        type_params,
                        mojito_symbol::symbol::MethodShape::of(method),
                        overloads,
                        constants,
                    );
                    list(
                        source,
                        shape.applicable(
                            symbol,
                            on_instance,
                            if method.has_self {
                                SelfSlot::Receiver
                            } else {
                                SelfSlot::None
                            },
                        ),
                        Some(method),
                    );
                }
            }
            _ => {}
        }
    }
    declarations
}

impl Checker {
    /// A `comptime if` condition must be a compile-time `Bool`: a generic
    /// constraint over the parameters in scope (`T == Int`, `n == 0`,
    /// `conforms_to(T, Copyable)`, a `TypeList` proposition, a predicate
    /// alias), or an ordinary `Bool` expression over compile-time bindings
    /// (a `comptime for` variable, a `Bool` parameter). It is typed, never
    /// evaluated: selection is the elaborator's. Every leaf under
    /// `not`/`and`/`or`, and the whole condition, is recorded on its span
    /// (`SemanticAdjustment::ComptimeCondition`) for the MIR branch: a leaf
    /// the constraint compiler does not close — an application, a `Bool`
    /// binding — is its parameter expression compared with `True`
    /// ([`Self::evaluated_condition`]). The recorded constraint, its
    /// operands bound, when the whole condition closes.
    pub(super) fn check_comptime_condition(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        let _position = self.comptime_position();
        let inlined = self.inline_local_comptime_values(cond);
        Ok(self.check_ct_bool(&inlined, test)?.map(|constraint| {
            self.record_comptime_condition(cond, &constraint);
            self.bind_constraint(&constraint, &[])
        }))
    }

    /// A `comptime assert`: its condition is checked as a `comptime if`
    /// condition is, never judged here — the elaborator judges it per
    /// reached instance. The message is a string literal. The condition's
    /// bound reading, when it closes, for the knowledge it introduces. A
    /// condition over no parameter that folds to `False` is recorded as a
    /// failing assertion, which ends every path through it.
    pub(super) fn check_comptime_assert(
        &self,
        cond: &Expr,
        message: Option<&Expr>,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        if message.is_some_and(|message| !matches!(message.kind, ExprKind::Str(_))) {
            return Err(TypeError::Unsupported(
                "a 'comptime assert' message other than a string literal".to_string(),
            ));
        }
        let constraint = self.check_comptime_condition(cond, ComptimeTest::Assert)?;
        if constraint.as_ref().is_some_and(|constraint| {
            matches!(
                self.fold_closed_constraint(constraint.clone()),
                GenericConstraint::Bool(false)
            )
        }) {
            self.record_at(cond.source_span()).failing_assertion = true;
        }
        Ok(constraint)
    }

    /// Whether the condition of a conditional expression is a parameter
    /// value, which upstream's `IfElseOpNode` lowers as `hlcf.comptime.if`
    /// (`getIfPValue`): a `Bool` literal, a `Bool` compile-time binding, a
    /// `comptime(e)`, a conformance or trivial-type predicate, a `TypeList`
    /// proposition, `not` of one, or a comparison of compile-time scalars or
    /// of types. `and`/`or` and a nested conditional are run-time `hlcf.if`s, a call of
    /// an ordinary function a run-time value, so none of those is one.
    pub(super) fn parameter_valued_condition(&self, cond: &Expr) -> bool {
        match &cond.kind {
            ExprKind::Bool(_) => true,
            ExprKind::Prefix(PrefixOp::Not, inner) => self.parameter_valued_condition(inner),
            ExprKind::Identifier(_) | ExprKind::Member { .. } => self
                .parameter_value_ty(cond)
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Bool)),
            ExprKind::Call { name, args, .. } => {
                (name == "comptime" || name == "conforms_to") && !args.is_empty()
            }
            ExprKind::TypeApply { .. } | ExprKind::Index { .. } => {
                self.predicate_alias_application(cond).is_some()
                    || predicate_spelling(cond).is_some_and(|name| {
                        mojito_types::types::trivial_predicate_name(name).is_some()
                    })
            }
            ExprKind::Invoke { .. } => {
                matches!(self.compile_typelist_proposition(cond), Ok(Some(_)))
            }
            ExprKind::Infix(
                InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge,
                left,
                right,
            ) => {
                (self.parameter_valued_scalar(left) && self.parameter_valued_scalar(right))
                    || [left, right]
                        .iter()
                        .all(|operand| matches!(self.comptime_type_operand(operand), Ok(Some(_))))
            }
            _ => false,
        }
    }

    /// The type of the parameter value `operand` names: a value binder, a
    /// `comptime for` variable, a local or module `comptime` binding, or the
    /// enclosing struct's value parameter (`Self.flag`).
    pub(super) fn parameter_value_ty(&self, operand: &Expr) -> Option<&Ty> {
        match &operand.kind {
            ExprKind::Identifier(name)
                if self.is_compile_time_binding(name)
                    || self.lookup_owner(name).is_some_and(|owner| {
                        self.value_parameter_owners.contains(&owner)
                            || self.is_comptime_binding_owner(owner)
                    }) =>
            {
                self.lookup(name)
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                self.self_decls.iter().find_map(|decl| match decl {
                    ParamDecl::Value {
                        name,
                        ty,
                        variadic: false,
                        ..
                    } if name == field => Some(ty),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Record `constraint` as the compile-time reading of `cond`, each
    /// operand naming its binder by identity: the innermost open scope's,
    /// else the enclosing struct's.
    fn record_comptime_condition(&self, cond: &Expr, constraint: &GenericConstraint) {
        let bound = self.bind_constraint(constraint, &[]);
        self.record_at(cond.source_span()).comptime_condition = Some(bound);
    }

    /// Whether `operand` of a comparison is a parameter value whose builtin
    /// operators fold at compile time: an integer or `Bool` literal, or an
    /// `Int`, `UInt`, or `Bool` parameter value, under arithmetic. A float's
    /// comparison is a run-time value, as the pin has it.
    fn parameter_valued_scalar(&self, operand: &Expr) -> bool {
        match &operand.kind {
            ExprKind::Int(_) | ExprKind::Bool(_) => true,
            ExprKind::Identifier(_) | ExprKind::Member { .. } => {
                self.parameter_value_ty(operand).is_some_and(|ty| {
                    matches!(
                        ty.kind(),
                        TyKind::INT | TyKind::UINT | TyKind::Bool | TyKind::IntLiteral
                    )
                })
            }
            ExprKind::Prefix(PrefixOp::Neg | PrefixOp::Invert | PrefixOp::Pos, inner) => {
                self.parameter_valued_scalar(inner)
            }
            ExprKind::Infix(
                InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Shl
                | InfixOp::Shr
                | InfixOp::BitAnd
                | InfixOp::BitOr
                | InfixOp::BitXor,
                left,
                right,
            ) => self.parameter_valued_scalar(left) && self.parameter_valued_scalar(right),
            _ => false,
        }
    }

    /// The recursive form of [`Self::check_comptime_condition`]: a
    /// connective recurses so a concrete conformance fact can sit beside a
    /// symbolic constraint; a leaf is a generic constraint over the
    /// parameters in scope, a conformance of a concrete type, or a `Bool`
    /// value expression. The constraint the condition compiles to, when every
    /// leaf does.
    fn check_ct_bool(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        match &cond.kind {
            ExprKind::Bool(value) => return Ok(Some(GenericConstraint::Bool(*value))),
            ExprKind::Prefix(PrefixOp::Not, inner) => {
                return Ok(self
                    .check_ct_bool(inner, test)?
                    .map(|inner| GenericConstraint::Not(Box::new(inner))));
            }
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right) => {
                let left = self.check_ct_bool(left, test)?;
                let right = self.check_ct_bool(right, test)?;
                return Ok(left.zip(right).map(|(left, right)| {
                    if *op == InfixOp::And {
                        GenericConstraint::And(Box::new(left), Box::new(right))
                    } else {
                        GenericConstraint::Or(Box::new(left), Box::new(right))
                    }
                }));
            }
            // `conforms_to(MaybeUninit[Int], RegisterPassable)`: a fact about
            // a type that is not a parameter, which the constraint compiler
            // reserves for parameters.
            ExprKind::Call { name, args, .. }
                if name == "conforms_to"
                    && args.len() == 2
                    && self
                        .comptime_type_operand(&args[0])?
                        .is_some_and(|operand| !matches!(operand.kind(), TyKind::Param { .. })) =>
            {
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Err(TypeError::Unsupported(
                        "conforms_to takes a trait name, or a '&' conjunction of trait names, \
                         as its second argument"
                            .to_string(),
                    ));
                };
                trait_names
                    .iter()
                    .try_for_each(|trait_name| self.check_trait_name(trait_name))?;
                // A closed type's conformance is a fact decided here. An
                // open one's (a symbolic struct, a reflected field type at a
                // loop index) is, where unknown, the conjunction of its
                // conformance propositions, which the elaborator below MIR
                // decides once the type is closed.
                let Some(operand) = self.comptime_type_operand(&args[0])? else {
                    return Ok(None);
                };
                let mut refuted = mojito_types::conformance::reflects_no_conformance(&operand);
                let mut propositions = Vec::new();
                for trait_name in trait_names {
                    let unknown = match self.conformance(&operand, trait_name) {
                        Conformance::Proven => continue,
                        Conformance::Refuted => None,
                        Conformance::Unknown => self.conformance_subject(&operand),
                    };
                    let Some(subject) = unknown else {
                        refuted = true;
                        continue;
                    };
                    let conforms = self
                        .param_context
                        .conforms(&subject, trait_name)
                        .map_err(param_error)?;
                    propositions.push(GenericConstraint::Eq(
                        ConstraintOperand::Expr(conforms),
                        ConstraintOperand::Value(mojito_types::ct::CtValue::Bool(true)),
                    ));
                }
                let constraint = if refuted {
                    GenericConstraint::Bool(false)
                } else {
                    propositions
                        .into_iter()
                        .reduce(|all, proposition| {
                            GenericConstraint::And(Box::new(all), Box::new(proposition))
                        })
                        .unwrap_or(GenericConstraint::Bool(true))
                };
                self.record_comptime_condition(cond, &constraint);
                return Ok(Some(constraint));
            }
            // An application of an undeclared name (`TriviallyCopyable[Int]`)
            // is neither a predicate nor a type.
            ExprKind::TypeApply { name, .. }
                if mojito_types::types::trivial_predicate_name(name).is_none()
                    && self.comptime_name_resolves(name).is_err() =>
            {
                return Err(TypeError::UnknownType(name.clone()));
            }
            _ => {}
        }
        let constraint = self
            .compile_generic_constraint(cond)
            .and_then(|constraint| {
                self.constraint_operands_resolve(&constraint)
                    .map(|()| constraint)
            });
        match constraint {
            // A local `comptime` binding the elaborator evaluates (`comptime
            // e = L[1]`) is no parameter: the condition runs with it. A
            // runtime local compiles to a parameter no binder declares.
            Ok(constraint)
                if self.names_evaluated_binding(&constraint)
                    || self.names_unbound_parameter(&constraint) =>
            {
                self.expect_evaluated_bool(cond, test)?;
                self.evaluated_condition(cond, test).map(Some)
            }
            Ok(constraint) => {
                let constraint = self.fold_closed_constraint(constraint);
                self.record_comptime_condition(cond, &constraint);
                Ok(Some(constraint))
            }
            Err(constraint_error) => match self.expect_evaluated_bool(cond, test) {
                Ok(()) => self.evaluated_condition(cond, test).map(Some),
                // A condition over types or a `TypeList` has no value
                // reading; its constraint diagnosis is the one that names
                // the problem.
                Err(_) if self.condition_is_compile_time_shaped(cond) => Err(constraint_error),
                Err(error) => Err(error),
            },
        }
    }

    /// A leaf of a compile-time test that compiles to no constraint:
    /// `h(n) > 4`, `b`, `Self.flag`, `t[0] > 1`, or `n` over an `Int`. A
    /// `Bool` leaf is the one parameter expression a bracket argument
    /// spelling it is, compared with `True`: built structurally where the
    /// parameter domain expresses it, a call it cannot express applying the
    /// function the check lifts for that call (`lt(2, $c0(n))` for
    /// `abs(n) > 2`), and otherwise the application of the function lifted
    /// for the whole leaf. An integer leaf is its `Bool(x)`, which
    /// `Int.__bool__` spells `x != 0`; any other leaf that converts is the
    /// application of the function lifted for its conversion. The
    /// elaborator decides it per instance.
    fn evaluated_condition(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<GenericConstraint, TypeError> {
        use mojito_types::ct::CtValue;
        self.reject_dynamic_condition(cond, test)?;
        let ty = self.infer(cond)?;
        let structural = || {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(cond).ok()
        };
        let operand = |expression: ParamExpr| match expression.as_constant() {
            Some(value) => ConstraintOperand::Value(value.clone()),
            None => expression.as_decl_ref().map_or_else(
                || ConstraintOperand::Expr(expression.clone()),
                |binder| ConstraintOperand::Param(binder.clone()),
            ),
        };
        let integer = matches!(ty.kind(), TyKind::INT | TyKind::UINT | TyKind::IntLiteral);
        let constraint = match ty.kind() {
            TyKind::Bool => {
                structural()
                    .or_else(|| self.lifted_application(cond))
                    .map(|expression| {
                        GenericConstraint::Eq(
                            operand(expression),
                            ConstraintOperand::Value(CtValue::Bool(true)),
                        )
                    })
            }
            _ if integer && let Some(expression) = structural() => Some(GenericConstraint::Ne(
                operand(expression),
                ConstraintOperand::Value(CtValue::Int(0)),
            )),
            _ => self.converted_condition(cond).map(|expression| {
                GenericConstraint::Eq(
                    operand(expression),
                    ConstraintOperand::Value(CtValue::Bool(true)),
                )
            }),
        }
        .ok_or_else(|| {
            TypeError::Unsupported(format!(
                "a {} that compiles to no parameter expression",
                test.context()
            ))
        })?;
        let constraint = self.fold_closed_constraint(constraint);
        self.record_comptime_condition(cond, &constraint);
        Ok(constraint)
    }

    /// The application of the function lifted for the `Bool(x)` conversion
    /// of the condition leaf `cond`, which MIR lowers as the leaf's
    /// truthiness (`comptime if y >= 1.5:` over a `Float32`, `comptime if
    /// alias:` over an `Optional`).
    fn converted_condition(&self, cond: &Expr) -> Option<ParamExpr> {
        let application = self.named_application(cond, &names_read(cond), Ty::bool())?;
        self.lifted_expressions
            .borrow_mut()
            .insert(cond.source_span(), application.clone());
        Some(application)
    }

    /// A condition over no parameter is a fact: the closed verdict of
    /// `constraint`, as the pin folds a constant parameter expression, or
    /// the constraint itself while a leaf stays open for the elaborator.
    fn fold_closed_constraint(&self, constraint: GenericConstraint) -> GenericConstraint {
        if !constraint_is_closed(&constraint) {
            return constraint;
        }
        match self.constraint_verdict(&constraint, &ConstraintEnvironment::default()) {
            Ok(mojito_types::param_expr::ConstraintVerdict::Proven) => {
                GenericConstraint::Bool(true)
            }
            Ok(mojito_types::param_expr::ConstraintVerdict::Disproven) => {
                GenericConstraint::Bool(false)
            }
            _ => constraint,
        }
    }

    /// Require the leaf `cond` of a compile-time test, which the
    /// elaborator runs, to be a `Bool` that calls no raising function: a
    /// raising call (a `raises` `__eq__`) is the pin's rejection.
    fn expect_evaluated_bool(&self, cond: &Expr, test: ComptimeTest) -> Result<(), TypeError> {
        let (checked, raised) = self.observing_raises(|| self.expect_bool(cond, test.context()));
        checked?;
        if raised {
            return Err(TypeError::Unsupported(test.raising_call().to_string()));
        }
        Ok(())
    }

    /// A `comptime if` condition the constraint compiler does not close
    /// runs in a function the elaborator evaluates, so it reads compile-time
    /// bindings alone; one over a runtime local is the pin's rejection.
    fn reject_dynamic_condition(&self, cond: &Expr, test: ComptimeTest) -> Result<(), TypeError> {
        if self.reads_compile_time_alone(cond) {
            return Ok(());
        }
        Err(TypeError::Unsupported(test.dynamic_value().to_string()))
    }

    /// The conformances a `comptime if` condition proves of its operands for
    /// the arm it guards, as the pinned Mojo licenses them: every
    /// `conforms_to(X, A & B)` atom under `and`, keyed by what `X` denotes — a
    /// parameter's binder, a dependent element's canonical expression — so
    /// the proof reaches exactly that element, and only below the condition.
    /// `or`, `not`, and any other leaf prove nothing.
    pub(super) fn conformance_arm_assumptions(
        &self,
        cond: &Expr,
    ) -> Result<Vec<ParamExpr>, TypeError> {
        let cond = self.inline_local_comptime_values(cond);
        let mut proved = Vec::new();
        self.collect_arm_assumptions(&cond, &mut proved)?;
        Ok(proved)
    }

    fn collect_arm_assumptions(
        &self,
        cond: &Expr,
        proved: &mut Vec<ParamExpr>,
    ) -> Result<(), TypeError> {
        match &cond.kind {
            ExprKind::Infix(InfixOp::And, left, right) => {
                self.collect_arm_assumptions(left, proved)?;
                self.collect_arm_assumptions(right, proved)
            }
            ExprKind::Call { name, args, .. } if name == "conforms_to" && args.len() == 2 => {
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Ok(());
                };
                let Some(operand) = self.comptime_type_operand(&args[0])? else {
                    return Ok(());
                };
                let subject = match operand.kind() {
                    TyKind::Param { .. } => self.param_context.type_shape(operand.clone()),
                    TyKind::Dependent(dependent) => dependent.expr().clone(),
                    _ => return Ok(()),
                };
                for trait_name in trait_names {
                    proved.push(
                        self.param_context
                            .conforms(&subject, trait_name)
                            .map_err(param_error)?,
                    );
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Check a `comptime for` by upstream's iterator protocol
    /// ([`Self::check_comptime_iterator_for`]) over its iterable with every
    /// local compile-time value inlined.
    pub(super) fn check_comptime_for(
        &mut self,
        stmt: &Stmt,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret: Option<&Ty>,
    ) -> Result<(), TypeError> {
        let inlined = self.inline_local_comptime_values(iter);
        self.check_comptime_iterator_for(stmt, var, (iter, &inlined), body, ret)
    }

    /// Whether every local `expr` names is a compile-time binding: a value
    /// binder, a `comptime for` variable, or a local `comptime` constant or
    /// display. Such an expression can run in a function the elaborator
    /// evaluates per instance.
    pub(super) fn reads_compile_time_alone(&self, expr: &Expr) -> bool {
        if reads_comptime_interpreter_query(expr) {
            return false;
        }
        let mut names = value_names_read(expr);
        for receiver in self.type_only_receivers(expr) {
            if let Some(position) = names.iter().position(|name| *name == receiver) {
                names.swap_remove(position);
            }
        }
        names.iter().all(|name| {
            self.binding_scope(name).is_none_or(|scope| scope == 0)
                || self.is_compile_time_binding(name)
                || self.lookup_owner(name).is_some_and(|owner| {
                    self.value_parameter_owners.contains(&owner)
                        || self.is_comptime_binding_owner(owner)
                })
        })
    }

    /// The bindings `expr` reads only for their type, once per read: a
    /// collector's `args.__len__()` is its pack's length, and a value's
    /// parameter member (`x.dtype`) is its type's parameter.
    fn type_only_receivers(&self, expr: &Expr) -> Vec<String> {
        struct Receivers<'c> {
            checker: &'c Checker,
            names: Vec<String>,
        }
        impl mojito_ast::visit::Visitor for Receivers<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                let receiver = match &expr.kind {
                    ExprKind::MethodCall { object, .. }
                        if self.checker.pack_length_binder(expr).is_some() =>
                    {
                        object
                    }
                    ExprKind::Member { object, field }
                        if matches!(&object.kind, ExprKind::Identifier(name)
                        if self.checker.lookup(name).is_some_and(|ty| {
                            self.checker.is_parameter_member(ty, field)
                        })) =>
                    {
                        object
                    }
                    _ => return,
                };
                if let ExprKind::Identifier(name) = &receiver.kind {
                    self.names.push(name.clone());
                }
            }
        }
        let mut receivers = Receivers {
            checker: self,
            names: Vec::new(),
        };
        mojito_ast::visit::walk_expr(&mut receivers, expr);
        receivers.names
    }

    /// [`Self::lifted_application`] where the check stands in a lifting
    /// position ([`Self::lifting_position`]).
    pub(super) fn positioned_application(&self, expr: &Expr) -> Option<ParamExpr> {
        (self.lifting_positions.get() > 0)
            .then(|| self.lifted_application(expr))
            .flatten()
    }

    /// The parameter expression of a compile-time `Int`, `UInt`, `Bool`,
    /// `Float64`, or `String` expression that compiles to none (`h(L[0])`,
    /// `n > 2`, `min(L[0], L[1])`, `half(n)`, `pick(n) + "!"`), written in a
    /// type or a parameter argument or bound by a local `comptime`.
    ///
    /// A call of a module function is the application of that function to
    /// what its arguments denote ([`Self::called_application`]), wherever
    /// it is spelled: a signature, a field type, or a body. A call of a
    /// builtin function is its body's expression instead. Any other
    /// expression of a body is the application of the function MIR lifts
    /// for it, named here, to the binders it reads
    /// (`SemanticAdjustment::ComptimeApplication`, [`Self::binders_read`]).
    /// Such an expression reads compile-time bindings alone. Source
    /// validation lifts one that names a binder or a local binding, and
    /// leaves a closed one to the elaborator.
    ///
    /// Two occurrences of one lifted expression over the same bindings
    /// denote one application, as the pin identifies them by structure, and
    /// it is not the value it computes: `h(n)` is not `n * 2`, unless `h`
    /// is builtin. `None` for any other expression.
    pub(super) fn lifted_application(&self, expr: &Expr) -> Option<ParamExpr> {
        let called = self.called_application(expr);
        // A signature or a field type has no body to lift a function from,
        // and no compile-time argument calls a raising function.
        if self.outside_bodies() || self.calls_raising_application(expr).is_some() {
            return called;
        }
        let names = names_read(expr);
        if called.is_none() && !self.reads_compile_time_alone(expr) {
            return None;
        }
        // The expression's own arguments are typed where they stand. One
        // that raises (`M["a"]` over a dictionary) lifts nothing: its
        // binding reports the pin's rejection ([`Self::requested_binding`]).
        let lifting = self.lifting_positions.replace(0);
        let (ty, raised) = self.observing_raises(|| {
            let _position = self.comptime_position();
            self.infer(expr)
        });
        self.lifting_positions.set(lifting);
        if raised {
            return None;
        }
        let ty = ty.ok()?;
        let ty = match ty.kind() {
            TyKind::INT | TyKind::IntLiteral => Ty::int(),
            TyKind::UINT => Ty::uint(),
            TyKind::Bool => Ty::bool(),
            TyKind::FLOAT64 | TyKind::FloatLiteral => Ty::float64(),
            // A struct-, sized-scalar-, or attribute-valued call is an
            // application the elaborator below MIR runs and materializes
            // (an attribute only where an op's attribute takes it), and so
            // is any `String` expression, which it freezes to a
            // compile-time string, and any `!kgen.string` expression; no
            // other struct expression lifts.
            TyKind::Struct(..)
            | TyKind::Simd { .. }
            | TyKind::Index
            | TyKind::I1
            | TyKind::Dtype
            | TyKind::KgenDeferred
            | TyKind::KgenTarget
                if called.is_some() =>
            {
                ty
            }
            TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name) =>
            {
                ty
            }
            TyKind::KgenString => ty,
            _ => return None,
        };
        let application = match called {
            Some(application) => application,
            None => self.named_application(expr, &names, ty)?,
        };
        self.lifted_expressions
            .borrow_mut()
            .insert(expr.source_span(), application.clone());
        Some(application)
    }

    /// Record on `expr`, a compile-time `Int` or `Bool` expression of a body,
    /// the parameter expression the check compiled it to when that applies
    /// a function (`add(list_length(L), 1)` for `len(L) + 1`, `lt(4, h(n))`
    /// for `h(n) > 4`): MIR reads it wherever the expression's value is needed, as
    /// upstream reads sema's `kgen.param.constant`, and lifts nothing of its
    /// own. Arithmetic over binders alone (`n + 1`, a `comptime for`
    /// variable's `paramfor_next_value` among them) MIR builds from the
    /// expression's node. A lifted function's application is recorded only
    /// where the function is lifted ([`Self::lifted_application`]): MIR
    /// lifts it from the node that carries it.
    pub(super) fn record_compiled_expression(&self, expr: &Expr, expression: &ParamExpr) {
        if let mojito_types::param_expr::ParamKind::Apply { function, .. } = expression.kind()
            && mojito_symbol::symbol::lifted_owner(function).is_some()
        {
            return;
        }
        let stub = super::builtin_stub("");
        let mut applies = false;
        expression.visit(&mut |node| {
            applies |= matches!(
                node.kind(),
                mojito_types::param_expr::ParamKind::Apply { function, .. }
                    if !function.starts_with(&stub)
            );
        });
        let scalar = expression
            .meta()
            .as_value()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::INT | TyKind::Bool));
        if self.outside_bodies() || !applies || !scalar || expression.as_constant().is_some() {
            return;
        }
        self.lifted_expressions
            .borrow_mut()
            .insert(expr.source_span(), expression.clone());
    }

    /// The application a call of a module function, a static method, or a
    /// struct's `__init__` denotes when every argument is a compile-time
    /// value of its parameter's type (`h(n)`, `h(Self.n + 1)`,
    /// `twice[n]()`, `S.f(n)`, `d(n, m=4)`, `Extent(n, 2)`, `G[2](n)`,
    /// `idt(n)` over `idt[T: ImplicitlyCopyable](x: T)`, `mylen(Foo(n))`,
    /// `ln(s)` over a `String` parameter, a construction's result the
    /// instance it constructs, `Box(5)`'s the one its run-time construction
    /// infers): the callable applied to its
    /// compile-time parameters as the call binds them
    /// ([`Self::applied_arguments`]), then to its runtime arguments in
    /// declared order, a default standing where the call omits one, as the
    /// pin's call node is. It
    /// names no declaration's binders, so a caller that binds them spells
    /// the same value (`h(2)` for `h(n)` at `n = 2`, `d(1, 3)` for `d(1)`),
    /// and the elaborator runs the callable on the argument values. A call
    /// of an `@always_inline("builtin")` callable whose body folds is that
    /// body's expression over the arguments instead, as upstream's
    /// `foldInlineBuiltinFunction` clones the body with the call's operands
    /// substituted, so `twice(n)` is `n + n`, and a static method's body
    /// reads its struct's parameters, which the call binds from the
    /// instance, so `G[1].plus(n)` over `return n + Self.k` is `n + 1`. A
    /// builtin `__init__`'s call is the struct value of the fields its body
    /// stores, spelled as the application, so `L(n).v` over `self.v = v` is
    /// `n`. A builtin method's call on a compile-time struct value binds
    /// `self` to the receiver, so `L(n).doubled()` over `return self.v +
    /// self.v` is `2 * n`, and a struct value its body builds is spelled as
    /// the method's call.
    /// `None` for a callee that is
    /// no such callable ([`applicable_functions`]), a call that selects no
    /// one overload, or an argument that denotes no parameter expression.
    pub(super) fn called_application(&self, expr: &Expr) -> Option<ParamExpr> {
        if let Some(construction) = self.type_member_construction(expr) {
            return self.called_application(&construction);
        }
        let selected = self.selected_application(expr)?;
        if self.application_constraint_failure(&selected).is_some() {
            return None;
        }
        self.selected_call(selected)
    }

    /// The application of the `@implicit` `__init__` of the struct `ty` that
    /// converts a parameter argument of type `source` (`f[3]()` over `def
    /// f[w: W]()`), as a run-time argument's conversion selects it
    /// ([`Self::implicit_conversion_constructor`]): the constructor's call
    /// node over the parameter expression `argument` builds at its
    /// parameter's type, closed or symbolic, so `T[3]` is `T[W(3)]` and a
    /// builtin constructor folds. `None` when no constructor converts
    /// `source` or `argument` builds nothing, an error for a conversion no
    /// compile-time call applies, or one its `where` clause or `raises`
    /// rejects.
    pub(super) fn implicit_parameter_application(
        &self,
        source: &Ty,
        ty: &Ty,
        argument: impl FnOnce(&Ty) -> Option<ParamExpr>,
    ) -> Result<Option<ParamExpr>, TypeError> {
        let TyKind::Struct(owner, _) = ty.kind() else {
            return Ok(None);
        };
        let name = template_name(owner).to_string();
        let overloads = self.applicable_functions.get(&name);
        let function = match self.implicit_conversion_constructor(source, ty)? {
            Some(conversion) => overloads
                .and_then(|overloads| {
                    overloads.iter().find(|function| {
                        function.constructs() && function.symbol() == conversion.target
                    })
                })
                .ok_or_else(|| {
                    TypeError::Unsupported(format!(
                        "a parameter argument converted by '{}' is not supported yet: its \
                         parameter type is not one a compile-time call applies",
                        conversion.target
                    ))
                })?,
            // A builtin body folds before the struct's initializers
            // register: its `@implicit` one taking `source` converts.
            None if self
                .structs
                .get(owner)
                .is_none_or(|info| !info.methods.contains_key("__init__")) =>
            {
                let Some(function) = overloads.and_then(|overloads| {
                    overloads.iter().find(|function| {
                        function.constructs()
                            && function.implicit
                            && function.refined_self.is_none()
                            && matches!(function.params.as_slice(),
                                [param] if param.ty.resolved(Some(ty)) == Some(source))
                    })
                }) else {
                    return Ok(None);
                };
                function
            }
            None => return Ok(None),
        };
        let Some(argument) = function
            .params
            .first()
            .and_then(|param| param.ty.resolved(Some(ty)))
            .and_then(argument)
        else {
            return Ok(None);
        };
        let selected = Selected {
            name,
            function,
            instance: function.on_instance.then(|| ty.clone()),
            arguments: vec![argument],
            result: None,
        };
        if let Some(error) = self.application_constraint_failure(&selected) {
            return Err(error);
        }
        if function.raises {
            return Err(TypeError::BadCall {
                func: function.symbol.clone(),
                reason: "cannot call raising function in type parameter".to_string(),
            });
        }
        Ok(self.selected_call(selected))
    }

    /// Whether `expr` calls a `raises` callable a compile-time call would
    /// otherwise apply, which the pin rejects in a type or a parameter
    /// argument.
    pub(super) fn calls_raising_application(&self, expr: &Expr) -> Option<&str> {
        let function = self.selected_application(expr)?.function;
        function.raises.then_some(function.symbol.as_str())
    }

    /// Compile the `where` clauses of each callable `stmts` declare that a
    /// compile-time call applies, over its argument slots and its struct's
    /// binders, as [`Self::fold_builtin_bodies`] folds a builtin body. A
    /// callable whose clauses do not compile is applied by no call.
    pub(super) fn compile_application_constraints(&mut self, stmts: &[Stmt]) {
        let declarations = self.listed_declarations(stmts);
        for declaration in declarations {
            let clauses = declaration.function.where_clauses();
            if clauses.is_empty() {
                continue;
            }
            let compiled = self
                .in_argument_scope(&declaration, |checker, _| {
                    clauses
                        .iter()
                        .map(|clause| {
                            checker
                                .compile_where_clause(clause)
                                .map(|constraint| checker.bind_constraint(&constraint, &[]))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                })
                .flatten();
            if let Some(entry) = self.applicable_entry(&declaration) {
                entry.constraints = compiled;
            }
        }
    }

    /// Classify the own compile-time parameters of each callable `stmts`
    /// declare that a compile-time call applies, as its own check does and
    /// under the binder owner its check names them by, and resolve its
    /// `DType`- or binder-typed runtime parameters and its result over them,
    /// in its struct's member scope ([`ApplicableFunction::signature`]). A
    /// callable whose parameter default applies another waits for that one
    /// to classify; one that classifies in no round stays unclassified, and
    /// a later run retries it.
    pub(super) fn classify_applicable_signatures(&mut self, stmts: &[Stmt]) {
        let declarations = self.listed_declarations(stmts);
        let mut pending: Vec<_> = declarations
            .iter()
            .filter(|declaration| declaration.function.signature.is_none())
            .collect();
        loop {
            let before = pending.len();
            pending.retain(|declaration| {
                let Some((signature, params)) = self
                    .in_owner_scope(declaration, |checker| {
                        checker.applied_signature(declaration)
                    })
                    .flatten()
                else {
                    return true;
                };
                if let Some(entry) = self.applicable_entry(declaration) {
                    entry.signature = Some(signature);
                    for (param, ty) in entry.params.iter_mut().zip(params) {
                        if let SlotType::Declared(_, declared) = &mut param.ty {
                            *declared = ty;
                        }
                    }
                }
                false
            });
            if pending.is_empty() || pending.len() == before {
                return;
            }
        }
    }

    /// [`applicable_declarations`] of `stmts`, each callable as
    /// [`Self::applicable_functions`] holds it, its signature classified and
    /// its body folded so far.
    pub(super) fn listed_declarations<'a>(
        &self,
        stmts: &'a [Stmt],
    ) -> Vec<ApplicableDeclaration<'a>> {
        let mut declarations =
            applicable_declarations(stmts, &*self.overload_sets, &*self.program_constants);
        for declaration in &mut declarations {
            if let Some(listed) =
                self.applicable_functions
                    .get(&declaration.name)
                    .and_then(|overloads| {
                        overloads
                            .iter()
                            .find(|function| function.symbol() == declaration.function.symbol())
                    })
            {
                declaration.function = listed.clone();
            }
        }
        declarations
    }

    /// The entry [`applicable_functions`] lists for `declaration`.
    pub(super) fn applicable_entry(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<&mut ApplicableFunction> {
        self.applicable_functions
            .get_mut(&declaration.name)?
            .iter_mut()
            .find(|function| function.symbol() == declaration.function.symbol())
    }

    /// `declaration`'s own compile-time parameters classified, its result
    /// over them, and the type of each runtime parameter its signature
    /// declares rather than spells ([`SlotType::Declared`]).
    fn applied_signature(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<(AppliedSignature, Vec<Option<Ty>>)> {
        let function = &declaration.function;
        let owner = match (
            declaration.method,
            declaration.owner.map(|owner| &owner.kind),
        ) {
            (Some(method), Some(StmtKind::Struct { name, .. })) => {
                self.method_binder_owner(name, method)
            }
            _ => function.symbol().to_string(),
        };
        let decls = self.classify_params(&owner, declaration.type_params).ok()?;
        if decls.len() != function.binders.len() {
            return None;
        }
        self.push_param_scope(&decls);
        let params = function
            .params
            .iter()
            .map(|param| match &param.ty {
                SlotType::Declared(source, _) => self.ty_from_anno(source).ok().map(Some),
                SlotType::Scalar(_) | SlotType::Owner => Some(None),
            })
            .collect::<Option<Vec<_>>>();
        let result = match &function.result {
            _ if function.constructs() => Some(None),
            SourceType::SelfType => Some(None),
            result => self.ty_from_anno(result).ok().map(Some),
        };
        let refined_self = match &function.refined_self {
            Some(refined) => self.ty_from_anno(refined).ok().map(Some),
            None => Some(None),
        };
        self.tparams.pop();
        Some((
            AppliedSignature {
                decls,
                result: result?,
                refined_self: refined_self?,
            },
            params?,
        ))
    }

    /// The rejection of a compile-time call whose callee's `where` clause
    /// its arguments violate, or which they leave unproven where the
    /// enclosing declaration's own clauses do not prove it, as a runtime
    /// call's are judged (`h[0](n)` over `where k > 0`).
    pub(super) fn violated_application_constraint(&self, expr: &Expr) -> Option<TypeError> {
        self.application_constraint_failure(&self.selected_application(expr)?)
    }

    /// The callable a compile-time call applies, the struct instance a
    /// static method is called on, or a method's receiver and its type,
    /// and the parameter expressions of its arguments: the one overload
    /// under the spelled name the arguments bind and type, whose `where`
    /// clauses have compiled. `Self(...)` in a struct's member is the
    /// construction of the struct over its own parameters, as the check
    /// spells it ([`Self::self_instance_spelling`]).
    fn selected_application(&self, expr: &Expr) -> Option<Selected<'_>> {
        let self_spelling;
        let (name, instance, receiver, param_args, args, kwargs) = match &expr.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "Self" && self.lookup("Self").is_none() => {
                let (owner, spelled) = self.self_instance_spelling()?;
                self_spelling = spelled;
                let param_args = if param_args.is_empty() {
                    self_spelling.as_slice()
                } else {
                    param_args.as_slice()
                };
                (owner, None, None, param_args, args, kwargs)
            }
            // A local of the name shadows the module function.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if self.binding_scope(name).is_none_or(|scope| scope == 0) => (
                name.clone(),
                None,
                None,
                param_args.as_slice(),
                args,
                kwargs,
            ),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let (name, instance, receiver) = self.applied_receiver(object, method)?;
                (name, Some(instance), receiver, [].as_slice(), args, kwargs)
            }
            // A method given its own compile-time arguments (`S.t[n]()`,
            // `l.scaled[2]()`).
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return None;
                };
                let (name, instance, receiver) = self.applied_receiver(object, field)?;
                (
                    name,
                    Some(instance),
                    receiver,
                    param_args.as_slice(),
                    args,
                    kwargs,
                )
            }
            _ => return None,
        };
        let mut selected = self
            .applicable_functions
            .get(&name)?
            .iter()
            .filter(|function| function.takes_self() == receiver.is_some())
            .filter_map(|function| {
                // A generic struct's construction is bracketed by the
                // instance it constructs (`G[2](n)`), not by binders of its
                // `__init__`; spelled without them (`G(n)`), it constructs
                // the instance its run-time construction infers.
                let (instance, param_args) = match (function.constructs(), function.on_instance) {
                    (true, true) if !param_args.is_empty() => (
                        self.ty_from_anno(&SourceType::Named(name.clone(), param_args.to_vec()))
                            .ok(),
                        [].as_slice(),
                    ),
                    (true, true) => (self.run_time_type(expr), param_args),
                    _ => (instance.clone(), param_args),
                };
                if (function.on_instance && instance.is_none()) || function.constraints.is_none() {
                    return None;
                }
                // A `Self` slot takes the instance a call is made on, or the
                // struct a non-generic one constructs.
                let owner = instance.clone().or_else(|| {
                    function
                        .constructs()
                        .then(|| {
                            self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))
                                .ok()
                        })
                        .flatten()
                });
                let (mut arguments, result) = self.applied_arguments(
                    &name,
                    function,
                    owner.as_ref(),
                    param_args,
                    args,
                    kwargs,
                )?;
                if let Some(receiver) = &receiver {
                    arguments.insert(function.binders.len(), receiver.clone());
                }
                Some(Selected {
                    name: name.clone(),
                    function,
                    instance,
                    arguments,
                    result,
                })
            });
        let application = selected.next()?;
        let mut tied: Vec<_> = selected.collect();
        if tied.is_empty() {
            return Some(application);
        }
        // Several overloads take the arguments' values (`W(3)` over
        // `__init__(IntLiteral)` and `__init__(Int)`): the call applies the
        // one its run-time call selects, as the pin ranks them.
        let target = self.run_time_overload(expr)?;
        tied.push(application);
        let mut ranked = tied
            .into_iter()
            .filter(|selected| selected.function.symbol == target);
        let application = ranked.next()?;
        ranked.next().is_none().then_some(application)
    }

    /// The callable `expr`'s run-time call resolves to
    /// (its `overload_target` fact), typing it first where the check
    /// has not. `None` while that typing is under way, as
    /// [`Self::run_time_type`]'s.
    fn run_time_overload(&self, expr: &Expr) -> Option<String> {
        let span = expr.source_span();
        let recorded = || {
            self.node_facts
                .borrow()
                .get(&span)
                .and_then(|facts| facts.overload_target.clone())
        };
        recorded().or_else(|| {
            if !self.instance_inferences.borrow_mut().insert(span.clone()) {
                return None;
            }
            let typed = self.run_time_typing(expr);
            self.instance_inferences.borrow_mut().remove(&span);
            typed.ok()?;
            recorded()
        })
    }

    /// The struct `expr`'s run-time construction builds, as its run-time
    /// typing infers it (`Box[Int]` for `Box(5)`). `None` while that typing
    /// is under way, which asks for no application of `expr` itself.
    fn run_time_type(&self, expr: &Expr) -> Option<Ty> {
        let span = expr.source_span();
        if !self.instance_inferences.borrow_mut().insert(span.clone()) {
            return None;
        }
        let typed = self.run_time_typing(expr);
        self.instance_inferences.borrow_mut().remove(&span);
        typed
            .ok()
            .filter(|ty| matches!(ty.kind(), TyKind::Struct(..)))
    }

    /// `expr` typed as a run-time expression in a compile-time position,
    /// lifting nothing.
    fn run_time_typing(&self, expr: &Expr) -> Result<Ty, TypeError> {
        let lifting = self.lifting_positions.replace(0);
        let typed = {
            let _position = self.comptime_position();
            self.infer(expr)
        };
        self.lifting_positions.set(lifting);
        typed
    }

    /// The application `selected` denotes ([`Self::called_application`]):
    /// the callable applied to its arguments, or a builtin body folded over
    /// them. `None` for a result no parameter expression holds or a
    /// `raises` callable.
    fn selected_call(&self, selected: Selected<'_>) -> Option<ParamExpr> {
        let Selected {
            function,
            instance,
            arguments,
            result,
            ..
        } = selected;
        let result = match &instance {
            Some(instance) if function.constructs() || function.result == SourceType::SelfType => {
                Some(instance.clone())
            }
            _ => result.or_else(|| self.ty_from_anno(&function.result).ok()),
        }
        .filter(|ty| {
            matches!(
                ty.kind(),
                TyKind::INT
                    | TyKind::UINT
                    | TyKind::Index
                    | TyKind::I1
                    | TyKind::Bool
                    | TyKind::FLOAT64
                    | TyKind::Dtype
                    | TyKind::Simd { .. }
                    | TyKind::Struct(..)
                    | TyKind::KgenDeferred
                    | TyKind::KgenTarget
                    | TyKind::KgenString
            )
        })?;
        if function.raises {
            return None;
        }
        let mut applied = Vec::with_capacity(arguments.len() + 1);
        if function.on_instance {
            applied.push(self.param_context.type_shape(instance.clone()?));
        }
        applied.extend(arguments.iter().cloned());
        let application = self.param_context.apply(
            &function.symbol,
            &applied,
            mojito_types::param_expr::MetaTy::value(result),
        );
        if let Some(body) = &function.folded_body {
            let mut bindings = mojito_types::param_expr::ParamBindings::new();
            for (slot, argument) in arguments.iter().enumerate() {
                let id = super::annotations::builtin_body_binder(&function.symbol, slot, "").id;
                // A type slot binds the type its argument's shape names, so
                // the folded body is typed at the instance (`Int`, not `T`).
                if let ParamKind::TypeShape(ty) | ParamKind::Constant(CtValue::Type(ty)) =
                    argument.kind()
                {
                    bindings.bind_type(id.clone(), ty.clone());
                }
                bindings.bind(id, argument.clone());
            }
            let nominal = instance
                .as_ref()
                .and_then(mojito_types::types::nominal_parts);
            let declared = match &nominal {
                Some((owner, instance_args)) => {
                    let decls = &self.structs.get(template_name(owner))?.decls;
                    let declared =
                        mojito_types::types::declared_pack_arguments(owner, decls, instance_args);
                    self.bind_declared(&mut bindings, decls, &declared).ok()?;
                    Some((decls, declared))
                }
                None => None,
            };
            let replaced = |expr: &ParamExpr| {
                self.param_context
                    .replace(expr, &bindings)
                    .ok()
                    .filter(|folded| {
                        declared.as_ref().is_none_or(|(decls, instance_args)| {
                            !reads_rebound_binder(folded, decls, instance_args)
                        })
                    })
            };
            let folded = match body {
                FoldedBody::Value(value) => replaced(value).map(|folded| {
                    respelled_struct_value(&self.param_context, folded, &application)
                }),
                // A vector is its `_mlir_value` storage.
                FoldedBody::Fields(fields)
                    if nominal.as_ref().is_some_and(|(owner, _)| {
                        *owner == mojito_types::types::SIMD_TYPE_NAME
                    }) =>
                {
                    match fields.as_slice() {
                        [(_, storage)] => replaced(storage),
                        _ => None,
                    }
                }
                FoldedBody::Fields(fields) => fields
                    .iter()
                    .map(|(name, value)| Some((name.clone(), replaced(value)?)))
                    .collect::<Option<Vec<_>>>()
                    .map(|fields| self.param_context.struct_value(&fields, &application)),
            };
            if folded.is_some() {
                return folded;
            }
        }
        Some(application)
    }

    /// The name a call of `method` on `object` spells, the struct instance
    /// it is called on, and the receiver it binds to `self`: a static
    /// method of a type `object` names ([`Self::applied_owner`]), or a
    /// method with `self` of a compile-time struct value `object` builds
    /// (`L(n).doubled()`, `l.doubled()` over a binder `l: L`), as the pin
    /// calls a method on a parameter value.
    fn applied_receiver(
        &self,
        object: &Expr,
        method: &str,
    ) -> Option<(String, Ty, Option<ParamExpr>)> {
        if let Some((owner, instance)) = self.applied_owner(object, method) {
            return Some((format!("{owner}.{method}"), instance, None));
        }
        let receiver = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(object).ok()?
        };
        let instance = receiver.meta().as_value()?.clone();
        let TyKind::Struct(owner, _) = instance.kind() else {
            return None;
        };
        let name = format!("{}.{method}", template_name(owner));
        self.applicable_functions
            .get(&name)?
            .iter()
            .any(ApplicableFunction::takes_self)
            .then_some((name, instance, Some(receiver)))
    }

    /// The rejection of `selected`'s call by a `where` clause of its
    /// callee, its own compile-time and runtime parameters bound to the
    /// call's arguments and its struct's to the instance it is called on.
    fn application_constraint_failure(&self, selected: &Selected<'_>) -> Option<TypeError> {
        let constraints = selected.function.constraints.as_deref()?;
        if constraints.is_empty() {
            return None;
        }
        let slots = selected
            .function
            .argument_slots(selected.instance.as_ref())?;
        let arguments: Vec<TyArg> = slots
            .iter()
            .zip(&selected.arguments)
            .map(|(slot, argument)| match (slot, argument.kind()) {
                (ParamDecl::Type { .. }, ParamKind::TypeShape(ty)) => TyArg::Ty(ty.clone()),
                (ParamDecl::Type { .. }, ParamKind::Constant(CtValue::Type(ty))) => {
                    TyArg::Ty(ty.clone())
                }
                _ => TyArg::Val(argument.clone().into_value()),
            })
            .collect();
        let instance = match selected.instance.as_ref().map(Ty::kind) {
            Some(TyKind::Struct(owner, instance_args)) => {
                let decls = &self.structs.get(template_name(owner))?.decls;
                let declared =
                    mojito_types::types::declared_pack_arguments(owner, decls, instance_args);
                Some((decls, declared))
            }
            _ => None,
        };
        let mut environment = ConstraintEnvironment::declared(&slots, &arguments);
        if let Some((decls, declared)) = &instance {
            environment.bind_declared(decls, declared);
        }
        let callee = selected
            .name
            .rsplit_once('.')
            .map_or(selected.name.as_str(), |(_, method)| method);
        constraints.iter().find_map(|constraint| {
            self.validate_constraint_in_environment(callee, constraint, &environment)
                .err()
        })
    }

    /// The struct whose static method `method` a call on `object` names
    /// (`S.f(n)`, `G[n].f()`, `Self.f()`), with the instance `object`
    /// spells; `None` for a receiver that is a value, or a struct with no
    /// applicable `method`.
    pub(super) fn applied_owner(&self, object: &Expr, method: &str) -> Option<(String, Ty)> {
        let applicable = |owner: &str| {
            self.applicable_functions
                .contains_key(&format!("{owner}.{method}"))
        };
        let named = |name: &String, args: Vec<ParamArg>| {
            (self.binding_scope(name).is_none() && applicable(name))
                .then(|| {
                    self.ty_from_anno(&SourceType::Named(name.clone(), args))
                        .ok()
                })
                .flatten()
        };
        let instance = match &object.kind {
            ExprKind::Identifier(name) if name == "Self" => self.self_ty.clone().filter(
                |ty| matches!(ty.kind(), TyKind::Struct(owner, _) if applicable(template_name(owner))),
            ),
            ExprKind::Identifier(name) => named(name, Vec::new()),
            ExprKind::TypeApply { name, args } => named(name, args.clone()),
            // A lone value argument parses as a subscript (`G[n]`).
            ExprKind::Index { object, index } => match (&object.kind, &index.kind) {
                (ExprKind::Identifier(name), ExprKind::TupleLit(elements)) => named(
                    name,
                    elements.iter().cloned().map(ParamArg::Value).collect(),
                ),
                (ExprKind::Identifier(name), _) => {
                    named(name, vec![ParamArg::Value((**index).clone())])
                }
                _ => None,
            },
            _ => None,
        }?;
        match instance.kind() {
            TyKind::Struct(owner, _) => Some((template_name(owner).to_string(), instance.clone())),
            _ => None,
        }
    }

    /// The parameter expressions a call's arguments give `function`, its
    /// compile-time parameters first, a `Self` parameter typed `owner`, and
    /// its result under them. Its compile-time parameters bind as a runtime
    /// call's do ([`Self::resolve_use_params`]): from the bracket arguments
    /// the call spells, from the runtime arguments their declared types
    /// read, or from their defaults. A type parameter is its type's shape.
    /// `None` when they do not bind its parameters or one is not a
    /// compile-time value of its parameter's type.
    fn applied_arguments(
        &self,
        name: &str,
        function: &ApplicableFunction,
        owner: Option<&Ty>,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<(Vec<ParamExpr>, Option<Ty>)> {
        use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
        let signature = function.signature.as_ref()?;
        let names: Vec<String> = function
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect();
        let required: Vec<bool> = function
            .params
            .iter()
            .map(|param| param.default.is_none())
            .collect();
        let keywords: Vec<&str> = kwargs.iter().map(|kwarg| kwarg.name.as_str()).collect();
        let matched = match_call_slots(
            &names,
            &required,
            function.positional_only,
            function.keyword_only,
            args.len(),
            &keywords,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .ok()?;
        // Each argument the call supplies is a compile-time value, built
        // once; one typed by a declared parameter binds that parameter.
        let supplied = matched
            .slots
            .iter()
            .map(|slot| {
                let argument = match slot {
                    ArgSlot::Positional(index) => &args[*index],
                    ArgSlot::Keyword(index) => &kwargs[*index].value,
                    ArgSlot::Default => return Some(None),
                };
                // An initializer list is the construction its run-time
                // typing spelled for the slot (`{1, 2}` at a `Box`).
                let spelled = self.spelled_construction(argument);
                let _lifting = self.lifting_position();
                self.compile_dependent_ct_expr(spelled.as_ref().unwrap_or(argument))
                    .ok()
                    .map(Some)
            })
            .collect::<Option<Vec<_>>>()?;
        // A slot typed by its struct's parameters (`v: Self.T`) is typed at
        // the instance.
        let at_owner = |ty: &Ty| match owner.and_then(mojito_types::types::nominal_parts) {
            Some((name, instance_args)) => self.structs.get(template_name(name)).map_or_else(
                || ty.clone(),
                |info| substitute_at(ty, info, &instance_args),
            ),
            None => ty.clone(),
        };
        let mut patterns = Vec::new();
        let mut actuals = Vec::new();
        // A refined initializer's `out self` is the instance it constructs.
        if let (Some(refined), Some(owner)) = (&signature.refined_self, owner) {
            patterns.push(at_owner(refined));
            actuals.push(owner.clone());
        }
        for (argument, param) in supplied.iter().zip(&function.params) {
            if let (SlotType::Declared(_, declared), Some(argument)) = (&param.ty, argument) {
                patterns.push(at_owner(declared.as_ref()?));
                actuals.push(
                    argument
                        .meta()
                        .as_value()
                        .filter(|ty| parameter_value_type(ty))?
                        .clone(),
                );
            }
        }
        let bound = self
            .resolve_use_params(name, &signature.decls, param_args, &patterns, &actuals)
            .ok()?;
        let values = Self::value_argument_environment(&signature.decls, &bound.arguments);
        let resolve = |ty: &Ty| {
            self.resolve_dependent_ty(&substitute(ty, &bound.subst), &values)
                .ok()
        };
        if let (Some(refined), Some(owner)) = (&signature.refined_self, owner)
            && resolve(&at_owner(refined)).as_ref() != Some(owner)
        {
            return None;
        }
        let mut arguments = Vec::with_capacity(signature.decls.len() + function.params.len());
        for (decl, argument) in signature.decls.iter().zip(&bound.arguments) {
            arguments.push(match (decl, argument) {
                (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                    self.param_context.type_shape(ty.clone())
                }
                (ParamDecl::Value { ty, .. }, TyArg::Val(value)) => {
                    self.applied_value(value.clone(), &resolve(ty)?)?
                }
                _ => return None,
            });
        }
        for (argument, param) in supplied.into_iter().zip(&function.params) {
            let ty = match &param.ty {
                SlotType::Declared(_, declared) => {
                    resolve(&at_owner(declared.as_ref()?)).filter(parameter_value_type)?
                }
                slot_type => slot_type.resolved(owner)?.clone(),
            };
            arguments.push(match argument {
                Some(argument) => self.applied_value(argument.into_value(), &ty)?,
                None => self.applied_default(param.default.as_ref()?, &ty)?,
            });
        }
        let result = match &signature.result {
            Some(result) => Some(resolve(result)?),
            None => None,
        };
        Some((arguments, result))
    }

    /// The parameter expression of a call argument for a parameter of type
    /// `ty`.
    pub(super) fn applied_argument(&self, argument: &Expr, ty: &Ty) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        let argument = self.compile_dependent_ct_expr(argument).ok()?;
        self.applied_value(argument.into_value(), ty)
    }

    /// The parameter expression of a declared default a call omits, for a
    /// parameter of type `ty`. The default is its declaration's, so it
    /// stands only where every name it reads means what it means there: no
    /// local and no binder of the calling declaration.
    fn applied_default(&self, default: &Expr, ty: &Ty) -> Option<ParamExpr> {
        names_read(default)
            .iter()
            .all(|name| {
                name != "Self"
                    && self.binding_scope(name).is_none_or(|scope| scope == 0)
                    && self.value_parameter_in_scope(name).is_none()
            })
            .then(|| self.applied_argument(default, ty))
            .flatten()
    }

    /// `value` as the argument of a parameter of type `ty`. A literal is the
    /// value the parameter holds, so that `h(2)` is `h(n)` at `n = 2`. A
    /// value of another type converts to a struct `ty` through its
    /// `@implicit` constructor, as the run-time argument does, so `L(4) + 1`
    /// over `__add__(self, rhs: Self)` applies `__add__` to `L(4)` and
    /// `L(1)` ([`Self::implicit_parameter_application`]).
    pub(super) fn applied_value(&self, value: CtValue, ty: &Ty) -> Option<ParamExpr> {
        let value = self.string_parameter_value(value, ty);
        let argument = match &value {
            CtValue::Expr(argument) => Some(argument.clone()),
            closed => closed
                .clone()
                .materialize_as(ty)
                .and_then(|closed| self.param_context.constant(closed).ok()),
        };
        if let Some(argument) = argument.filter(|argument| argument.meta().as_value() == Some(ty)) {
            return Some(argument);
        }
        let source = self.ct_value_ty(&value, ty).filter(|source| source != ty)?;
        self.implicit_parameter_application(&source, ty, |slot| self.applied_value(value, slot))
            .ok()
            .flatten()
    }

    /// The application of the function MIR lifts for `expr`, an expression
    /// of a body of type `ty` that reads `names`: one function per
    /// expression over the same binders and bindings.
    pub(super) fn named_application(
        &self,
        expr: &Expr,
        names: &[String],
        ty: Ty,
    ) -> Option<ParamExpr> {
        // The function is over the binders the expression reads, so that it
        // is one function under a `comptime for` and outside it.
        let mut binders = self.binders_in_scope();
        if let Some(mut read) = self.binders_read(expr, names) {
            // A binder typed over another (a loop's iterator over `T`)
            // brings that one too.
            loop {
                let typed_over: Vec<_> = binders
                    .iter()
                    .filter_map(ParamExpr::as_decl_ref)
                    .filter(|candidate| {
                        !read.contains(&candidate.id)
                            && binders.iter().any(|kept| {
                                kept.as_decl_ref()
                                    .is_some_and(|kept| read.contains(&kept.id))
                                    && meta_names_binder(kept.meta(), candidate)
                            })
                    })
                    .map(|candidate| candidate.id.clone())
                    .collect();
                if typed_over.is_empty() {
                    break;
                }
                read.extend(typed_over);
            }
            binders.retain(|binder| {
                binder
                    .as_decl_ref()
                    .is_some_and(|reference| read.contains(&reference.id))
            });
        }
        let bindings: Vec<_> = names.iter().map(|name| self.lookup_owner(name)).collect();
        let owner = self.lifted_function_owner();
        let known = self
            .lifted_applications
            .borrow()
            .iter()
            .find(|known| {
                known.owner == owner
                    && known.expression == *expr
                    && known.binders == binders
                    && known.bindings == bindings
                    && known.application.meta().as_value() == Some(&ty)
            })
            .map(|known| known.application.clone());
        known.or_else(|| {
            let name = self.lifted_function_name(&owner);
            let application = self.param_context.apply(
                &name,
                &binders,
                mojito_types::param_expr::MetaTy::value(ty),
            );
            self.lifted_applications
                .borrow_mut()
                .push(LiftedApplication {
                    owner,
                    expression: expr.clone(),
                    binders,
                    bindings,
                    application: application.clone(),
                });
            Some(application)
        })
    }

    /// The declaration that owns a function the check lifts here: the one
    /// whose body is being checked, or `__toplevel__` at module scope.
    fn lifted_function_owner(&self) -> String {
        self.lifting_owner
            .clone()
            .unwrap_or_else(|| "__toplevel__".to_string())
    }

    /// A new function `owner` lifts, named by `owner` and the count of
    /// those it lifted before, so the name is the same in every
    /// compilation.
    fn lifted_function_name(&self, owner: &str) -> String {
        let mut counts = self.lifted_counts.borrow_mut();
        let index = counts.entry(owner.to_string()).or_default();
        let name = mojito_symbol::symbol::lifted_check_name(owner, *index);
        *index += 1;
        name
    }

    /// The binders `expr` reads through the `names` it spells: each binder
    /// it names, those a local display binding or a local `comptime`
    /// parameter expression it names is over, and the enclosing struct's
    /// for `Self`. `None` when the binders are not known (a type-shaped
    /// argument, a local value that denotes no parameter expression), where
    /// every binder in scope stands in.
    fn binders_read(
        &self,
        expr: &Expr,
        names: &[String],
    ) -> Option<HashSet<mojito_types::param_expr::ParamId>> {
        struct Types(bool);
        impl mojito_ast::visit::Visitor for Types {
            fn visit_type(&mut self, _ty: &SourceType) {
                self.0 = true;
            }
        }
        let mut types = Types(false);
        mojito_ast::visit::walk_expr(&mut types, expr);
        if types.0 {
            return None;
        }
        let mut read = HashSet::new();
        for name in names {
            if name == "Self" {
                read.extend(self.self_decls.iter().map(|decl| decl.id().clone()));
                continue;
            }
            // A bare type argument (`width[T]()`) names its binder.
            let type_parameter = self.tparams.iter().rev().find_map(|scope| scope.get(name));
            match type_parameter.map(Ty::kind) {
                Some(TyKind::Param { binder, .. }) => {
                    read.insert(binder.id.clone());
                    continue;
                }
                Some(_) => return None,
                None => {}
            }
            let denoted = match self.display_named(name) {
                Some(display) => Some(display.sequence.clone()),
                None => self
                    .value_parameter_in_scope(name)
                    .or_else(|| self.pack_parameter_in_scope(name)),
            };
            match denoted {
                Some(denoted) => {
                    // A type it embeds over a binder (the instance an
                    // `__init__` application constructs) is type-shaped.
                    let mut typed = false;
                    denoted.visit(&mut |node| {
                        typed |= node
                            .embedded_types()
                            .into_iter()
                            .any(mojito_types::types::has_free_parameters);
                        if let mojito_types::param_expr::ParamKind::DeclRef(reference)
                        | mojito_types::param_expr::ParamKind::PackQuery {
                            pack: reference,
                            ..
                        } = node.kind()
                        {
                            read.insert(reference.id.clone());
                        }
                    });
                    if typed {
                        return None;
                    }
                }
                None if self.binding_scope(name).is_none_or(|scope| scope == 0) => {}
                None => return None,
            }
        }
        Some(read)
    }

    /// Record on each expression the executable check lifted the
    /// application it denotes, beside the expression's own checked
    /// operation (`SemanticAdjustment::ComptimeApplication`).
    pub(super) fn record_lifted_applications(&self) {
        use mojito_checked::checked::SemanticAdjustment;
        for (span, application) in self.lifted_expressions.borrow().iter() {
            let mut record = self.record_at(span.clone());
            let operation = match record.adjustment.take().map(|operation| *operation) {
                Some(SemanticAdjustment::ComptimeApplication { operation, .. }) => operation,
                operation => operation.map(Box::new),
            };
            record.adjustment = Some(Box::new(SemanticAdjustment::ComptimeApplication {
                operation,
                application: application.clone(),
            }));
        }
    }

    /// Whether `element` is a nominal `Tuple`, or a non-generic fieldwise
    /// struct, whose elements or fields are numbers, booleans, strings, or
    /// such aggregates: a parameter value a loop binder holds, which a
    /// backend materializes as a closed constant, or constructs where a
    /// string leaf makes it no constant.
    fn parameter_aggregate_element(&self, element: &Ty) -> bool {
        let leaf = |ty: &Ty| {
            matches!(
                ty.kind(),
                TyKind::INT
                    | TyKind::FLOAT64
                    | TyKind::Bool
                    | TyKind::IntLiteral
                    | TyKind::FloatLiteral
                    | TyKind::StringLiteral(_)
            ) || matches!(ty.kind(), TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name))
                || self.parameter_aggregate_element(ty)
        };
        if let Some(elements) = mojito_types::types::tuple_elements(element) {
            return !elements.is_empty() && elements.into_iter().all(leaf);
        }
        matches!(element.kind(), TyKind::Struct(name, args) if args.is_empty()
        && self.structs.get(name).is_some_and(|info| {
            info.fieldwise_init
                && info.decls.is_empty()
                && !info.fields.is_empty()
                && info.fields.iter().all(|(_, ty)| leaf(ty))
        }))
    }

    /// Whether `iter` is a display the elaborator evaluates per instance
    /// when the check does not close it: a collection display of scalars
    /// ([`evaluated_display`]), or a list display of parameter aggregates
    /// ([`Self::parameter_aggregate_element`]).
    fn evaluated_display(&self, iter: &Expr, element: &Ty) -> bool {
        evaluated_display(iter, element)
            || (matches!(&iter.kind, ExprKind::ListLit(items) if !items.is_empty())
                && self.parameter_aggregate_element(element))
    }

    /// The value-parameter scope beside the innermost open type-parameter
    /// scope. A level past it is dead: `tparams.pop()` closes both.
    pub(super) fn innermost_value_scope(&mut self) -> Option<&mut HashMap<String, ParamExpr>> {
        self.tparams
            .len()
            .checked_sub(1)
            .and_then(|level| self.vparams.get_mut(level))
    }

    /// What a `comptime NAME = value` with no binders in scope denotes
    /// where it is read at compile time: its parameter expression, or the
    /// application of the function the check lifts for its initializer. A
    /// module constant's lifted function is named by the constant
    /// (`$comptime$NAME$module`), the same in every pass, so a display
    /// constant is one application, evaluated once, whatever reads it.
    /// `None` when the initializer denotes nothing.
    pub(super) fn binderless_comptime_expression(
        &self,
        name: &str,
        value: &Expr,
        scope: ConstantScope<'_>,
    ) -> Result<Option<ParamExpr>, TypeError> {
        // A closed module constant is the attribute its declaration folded
        // to, which every read denotes, and a type-valued one is an alias:
        // neither requests anything.
        if matches!(scope, ConstantScope::Module(_))
            && (self.closed_module_attribute(value).is_some()
                || self
                    .compile_dependent_ct_expr(value)
                    .is_ok_and(|built| *built.meta() == mojito_types::param_expr::MetaTy::Type))
        {
            return Ok(None);
        }
        if let Some(expression) = self.comptime_value_expression(value) {
            return Ok(Some(expression));
        }
        Ok(self
            .requested_binding(value, scope)?
            .map(|application| self.module_lifted_application(name, value, application)))
    }

    /// Bind a local `comptime NAME = value` over the binders of the template
    /// body being checked, which the elaborator keeps (`comptime m = N + 1`):
    /// a type-valued one is a scoped type alias, and a value-valued one names
    /// its parameter expression wherever the body reads a compile-time value
    /// (a type argument, a `comptime if` condition), as upstream's
    /// `comptime` alias does. A type alias records no statement binding, so
    /// MIR gives it no runtime form. Returns `false` when the binding is not
    /// symbolic, or when the ordinary binding path still types its runtime
    /// reads.
    pub(super) fn bind_template_comptime(
        &mut self,
        stmt: &Stmt,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<bool, TypeError> {
        // A string literal's KGEN string, or a closed raw list read off a
        // struct's parameter, is compile-time only before any probe below
        // types it as a template's expression.
        if self.reads_raw_list(value) {
            self.check_raw_list_annotation(name, annotation, value)?;
            return self.bind_compile_time_only_value(name, annotation, value);
        }
        if self.reads_literal_value(value) || self.builds_attribute(value) {
            return self.bind_compile_time_only_value(name, annotation, value);
        }
        // A reflected field type (`comptime F = r.field_at[1].T`) is a type
        // alias where a type is read, and the `AnyType` value it spells
        // where a binder takes it.
        if !self.function_bases.is_empty()
            && let Some(ty) = self.reflected_type_operand(value)?
        {
            self.bind_local_type_alias(name, ty, value)?;
            self.local_comptime_values
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), value.clone());
            return Ok(true);
        }
        // A module generator constant is bound ahead of its statement, or,
        // reading a struct's member, by it.
        if self.function_bases.is_empty() && self.scopes.len() == 1 {
            if matches!(self.module_values.get(name), Some(CtValue::Generator(_))) {
                return Ok(true);
            }
            if let Ok(generator @ CtValue::Generator(_)) = self.eval_associated_ct(value) {
                self.module_values.insert(name.to_string(), generator);
                return Ok(true);
            }
        }
        // A parameterized declaration read unapplied (`comptime g = T.get`,
        // `comptime t = Twice`) is a generator, compile-time data with no
        // runtime form: the binding is its parameter expression alone.
        if !self.function_bases.is_empty()
            && let Ok(generator) = self.compile_dependent_ct_expr(value)
            && generator
                .meta()
                .as_value()
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Generator(_)))
        {
            if let Some(annotation) = annotation {
                let expected = self
                    .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
                let found = generator.meta().to_string();
                if !generator
                    .meta()
                    .is_generator_of(&MetaTy::value(expected.clone()))
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found,
                        context: format!("comptime '{name}'"),
                    });
                }
            }
            let level = self.tparams.len().saturating_sub(1);
            self.local_comptime_parameters
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), (level, generator));
            return Ok(true);
        }
        let Some(level) = self.tparams.len().checked_sub(1) else {
            if self.bind_compile_time_only_value(name, annotation, value)? {
                return Ok(true);
            }
            // A body with no binders keeps a binding only when it applies a
            // callable (`comptime x = f(1)`): its reads in a compile-time
            // position are the application, and the rest of the statement
            // is typed as any binding is.
            let scope = if self.function_bases.is_empty() {
                // A closed constant folds to its attribute where its
                // initializer first builds, which may read a constant bound
                // only by its own statement (`NAMES[1]`).
                if let Some(folded) = self.closed_module_attribute(value)
                    && !self.has_module_constant_entry(name)
                {
                    self.module_values.insert(name.to_string(), folded);
                }
                ConstantScope::Module(annotation)
            } else {
                ConstantScope::Local
            };
            if let Some(expression) = self.binderless_comptime_expression(name, value, scope)?
                && let Some(scope) = self.local_comptime_parameters.last_mut()
            {
                scope.insert(name.to_string(), (0, expression));
            }
            return Ok(false);
        };
        if let Some(ty) = self.comptime_type_operand(value)? {
            self.bind_local_type_alias(name, ty, value)?;
            return Ok(true);
        }
        if let Some((target, display)) = self.aliased_display(value) {
            // An alias of a display binding (`comptime A = L`) is another
            // name of that binding: it denotes the same sequence, and MIR
            // sees a read of it, and the statement, as the display's.
            self.declare_alias(name, target)?;
            self.record_statement_binding(stmt, name);
            self.local_comptime_displays
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), display);
            return Ok(true);
        }
        let denoted = self.comptime_value_expression(value);
        if let Some(expression) = denoted.clone() {
            // An annotation names the parameter expression's own type: no
            // conversion applies to one.
            if let (Some(annotation), Some(found)) = (annotation, expression.meta().as_value()) {
                let expected = self
                    .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
                // Any other annotation converts the value, as a runtime
                // binding's does (`comptime x: Float64 = i`).
                if &expected != found && !matches!(expected.kind(), TyKind::INT | TyKind::Bool) {
                    return Ok(false);
                }
                if &expected != found {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: format!("comptime '{name}'"),
                    });
                }
            }
            // A raw parameter list (`comptime raw = vs.values`) is
            // compile-time data with no runtime form: the binding is its
            // parameter expression alone, which every read names.
            let raw_list = self.value_pack_named(value).is_none()
                && expression.as_decl_ref().is_some()
                && matches!(
                    expression.meta(),
                    mojito_types::param_expr::MetaTy::ParamList(_)
                );
            let expression = self.alias_value(value, expression);
            if let Some(scope) = self.local_comptime_parameters.last_mut() {
                scope.insert(name.to_string(), (level, expression));
            }
            if raw_list {
                return Ok(true);
            }
        }
        if let Some((element, ty, construction)) = self.evaluated_display_binding(value, annotation)
        {
            // The name is typed for the compile-time expressions that read
            // it, and its statement binding is the one a lifted function
            // binds the display to.
            self.declare_immutable(name, ty.clone())?;
            self.record_statement_binding(stmt, name);
            let display = self.record_display_binding(name, value, &element, ty, construction)?;
            self.local_comptime_displays
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), display);
            return Ok(true);
        }
        if self.bind_compile_time_only_value(name, annotation, value)? {
            return Ok(true);
        }
        // A closed binding is the application it requests, else the
        // constant it builds to (`comptime s = "Lo"`), which an annotation
        // would convert.
        let expression = match denoted {
            Some(_) => None,
            None => self
                .requested_binding(value, ConstantScope::Local)?
                .or_else(|| {
                    annotation
                        .is_none()
                        .then(|| self.closed_constant_expression(value))
                        .flatten()
                }),
        };
        if let Some(expression) = expression
            && let Some(scope) = self.local_comptime_parameters.last_mut()
        {
            scope.insert(name.to_string(), (level, expression));
        }
        Ok(false)
    }

    /// Require a raw list binding's annotation, if any, to name the list's
    /// type (`comptime raw: _MLIR.KGENParamListType[Int] =
    /// ParameterList.of[4, 5].values`).
    fn check_raw_list_annotation(
        &self,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let (Some(annotation), Some(found)) = (annotation, self.member_value_type(value)) else {
            return Ok(());
        };
        let expected =
            self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
        if coerces(&found, &expected) || self.param_list_converts(&found, &expected) {
            return Ok(());
        }
        Err(TypeError::TypeMismatch {
            expected: expected.to_string(),
            found: found.to_string(),
            context: format!("comptime '{name}'"),
        })
    }

    /// Require a `TypeList` binding's annotation, if any, to accept the
    /// list's type (`comptime tl: TypeList[Trait=Movable, Ts.values] = Ts`),
    /// as a runtime binding's does.
    fn check_type_list_annotation(
        &self,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let Some(annotation) = annotation else {
            return Ok(());
        };
        let found = {
            let _position = self.comptime_position();
            self.infer(value)?
        };
        let expected =
            self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
        if coerces(&found, &expected) {
            return Ok(());
        }
        Err(TypeError::TypeMismatch {
            expected: expected.to_string(),
            found: found.to_string(),
            context: format!("comptime '{name}'"),
        })
    }

    /// Bind a reflection handle (`reflect[T]`, `r.field["x"]`), a reflected
    /// list (`r.field_names()`), a string literal's KGEN string
    /// (`"None".value`), a closed raw list (`ParameterList.of[4, 5].values`),
    /// or a `TypeList` value (`Ts`, `TypeList.of[Trait=AnyType, Int]()`,
    /// `TypeList._concat[Self.Ts.values, OtherTs.values]()`): a compile-time
    /// position reads the bound expression, and a run-time read of a
    /// `TypeList` spells it again ([`Self::infer_type_list_binding_read`]).
    /// `false` when `value` is none of these.
    fn bind_compile_time_only_value(
        &mut self,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<bool, TypeError> {
        let type_list = matches!(self.type_list_operand(value), Ok(Some(_)));
        // A handle binds as written: a bad selection
        // (`reflect[P].field["missing"]`) is reported where it is read, as
        // the pin runs an unread binding.
        if !(type_list
            || super::reflection::spells_reflection_handle(value)
            || matches!(self.reflection_handle(value), Ok(Some(_)))
            || matches!(self.reflection_list(value), Ok(Some(_)))
            || self.reads_literal_value(value)
            || self.builds_attribute(value)
            || self.reads_raw_list(value))
        {
            return Ok(false);
        }
        if type_list {
            self.check_type_list_annotation(name, annotation, value)?;
        }
        self.local_comptime_values
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker scope stack is empty".to_string())
            })?
            .insert(name.to_string(), value.clone());
        Ok(true)
    }

    /// The application a local `comptime` binding of a body denotes when
    /// its initializer applies a callable at a type no parameter expression
    /// spells (`comptime s = label(3)`, a `String`): the function MIR lifts
    /// for it, at the initializer's own type, which the elaborator below MIR
    /// demands and runs once per instance. A module constant's collection
    /// display, and a dictionary or set constructor call in either scope, is
    /// one too ([`super::lifts_comptime_value`]). `None` for any other
    /// initializer.
    fn requested_binding(
        &self,
        value: &Expr,
        scope: ConstantScope<'_>,
    ) -> Result<Option<ParamExpr>, TypeError> {
        let (lifted, annotation) = match scope {
            // A module constant the one builder does not fold closed
            // (`max(3, 4)`) is the application its initializer computes.
            ConstantScope::Module(annotation) => (
                super::lifts_comptime_value(value, self)
                    || (self.calls_builtin_function(value)
                        && self.closed_module_constant(value).is_none()),
                annotation,
            ),
            // A local display keeps its display route
            // (`evaluated_display_binding`); a constructor call is lifted.
            ConstantScope::Local => (
                matches!(value.kind, ExprKind::Call { .. })
                    && super::lifts_comptime_value(value, self),
                None,
            ),
        };
        if !(lifted || self.applies_callable(value)) {
            return Ok(None);
        }
        // A display takes its annotation as its contextual type, as a `var`
        // initializer does.
        let expected = annotation
            .filter(|_| {
                matches!(
                    value.kind,
                    ExprKind::ListLit(_) | ExprKind::BraceLit(_) | ExprKind::InitializerList { .. }
                )
            })
            .map(|annotation| {
                self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)
            })
            .transpose()?;
        if let ExprKind::Call { name, .. } = &value.kind
            && self
                .lookup(name)
                .and_then(Ty::callable)
                .is_some_and(|func| func.raises)
        {
            return Err(TypeError::Unsupported(
                "cannot call raising function in comptime initializer".to_string(),
            ));
        }
        if !self.reads_compile_time_alone(value) {
            return Err(TypeError::Unsupported(
                "cannot use a dynamic value in comptime initializer".to_string(),
            ));
        }
        // A raising operation anywhere in it (`{"a": 1}["a"]`) is the
        // pin's rejection too.
        let (ty, raised) = self.observing_raises(|| {
            let _position = self.comptime_position();
            match &expected {
                Some(expected) => self.infer_with_expected(value, expected, true),
                None => self.infer(value),
            }
        });
        if raised {
            return Err(TypeError::Unsupported(
                "cannot call raising function in comptime initializer".to_string(),
            ));
        }
        let ty = mojito_types::types::default_literal(&ty?);
        self.copy_compile_time_result(value, &ty);
        let Some(application) = self.named_application(value, &names_read(value), ty) else {
            return Ok(None);
        };
        self.lifted_expressions
            .borrow_mut()
            .insert(value.source_span(), application.clone());
        Ok(Some(application))
    }

    /// The attribute a closed module constant other than a collection
    /// display folds to ([`Self::closed_module_constant`]); a display is
    /// the application the check lifts.
    fn closed_module_attribute(&self, value: &Expr) -> Option<CtValue> {
        if super::lifts_comptime_value(value, self) {
            return None;
        }
        self.closed_module_constant(value)
    }

    /// Whether `value` calls a builtin function the check types itself
    /// (`max(3, 4)`, `abs(n)`): an application the elaborator does not
    /// fold, as upstream's interpreter runs the library function. The
    /// queries the elaborator folds (`len`, `range`, `reflect`,
    /// `conforms_to`, `materialize`) are not.
    fn calls_builtin_function(&self, value: &Expr) -> bool {
        struct Finder<'c> {
            checker: &'c Checker,
            found: bool,
        }
        impl mojito_ast::visit::Visitor for Finder<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                if let ExprKind::Call { name, .. } = &expr.kind {
                    self.found |= !matches!(
                        name.as_str(),
                        "len" | "range" | "reflect" | "conforms_to" | "materialize"
                    ) && self.checker.binding_scope(name).is_none()
                        && super::CalleeOracle::callee(self.checker, name) == super::Callee::Other;
                }
            }
        }
        let mut finder = Finder {
            checker: self,
            found: false,
        };
        mojito_ast::visit::walk_expr(&mut finder, value);
        finder.found
    }

    /// Run `check`, reporting whether it typed a raising operation that no
    /// `try` inside it handles.
    fn observing_raises<T>(&self, check: impl FnOnce() -> T) -> (T, bool) {
        self.raise_observation_frames
            .borrow_mut()
            .push((self.handled_raise_depth, false));
        let checked = check();
        let raised = self
            .raise_observation_frames
            .borrow_mut()
            .pop()
            .is_some_and(|(_, raised)| raised);
        (checked, raised)
    }

    /// Whether `value` applies a callable ([`super::applies_callable`]), as
    /// the elaborator keeps a binding of one for the check, or reads a
    /// module constant or a local display binding that does. A module
    /// constant folded to a scalar or a string is an attribute, which
    /// applies nothing.
    fn applies_callable(&self, value: &Expr) -> bool {
        super::applies_callable(value, self)
            || names_read(value).iter().any(|name| {
                (self.binding_scope(name) == Some(0)
                    && self.module_constant_value(name).is_some_and(|expression| {
                        !expression.as_constant().is_some_and(is_scalar_attribute)
                    }))
                    || self.display_named(name).is_some()
            })
    }

    /// `application`, the function lifted for the module constant `name`'s
    /// initializer `value`, renamed by the constant: a body's facts carried
    /// from an earlier pass name it, and MIR lifts it from the toplevel
    /// under that name. A function-local binding keeps its declaration's
    /// name.
    fn module_lifted_application(
        &self,
        name: &str,
        value: &Expr,
        application: ParamExpr,
    ) -> ParamExpr {
        if !self.function_bases.is_empty() {
            return application;
        }
        let mojito_types::param_expr::ParamKind::Apply { args, .. } = application.kind() else {
            return application;
        };
        // A display constant is a parameter list, which a subscript or a
        // loop header reads element by element; one its annotation types as
        // another collection (`comptime L: List[Int] = [1, 2]`) is that
        // collection's value.
        let annotated = matches!(application.meta().as_value().map(Ty::kind),
            Some(TyKind::Struct(collection, _)) if collection != mojito_types::types::ARRAY_TYPE_NAME);
        let meta = match &value.kind {
            ExprKind::ListLit(_) | ExprKind::TupleLit(_) if !annotated => {
                self.display_listed_element(value).map_or_else(
                    || application.meta().clone(),
                    |element| {
                        mojito_types::param_expr::MetaTy::ParamList(Box::new(
                            mojito_types::param_expr::MetaTy::value(string_element_binder(element)),
                        ))
                    },
                )
            }
            _ => application.meta().clone(),
        };
        let renamed = self.param_context.apply(
            &mojito_symbol::symbol::lifted_constant_name(name),
            args,
            meta,
        );
        self.lifted_expressions
            .borrow_mut()
            .insert(value.source_span(), renamed.clone());
        renamed
    }

    /// What the value of a local `comptime` binding over a template body's
    /// binders denotes wherever the body reads it at compile time: its
    /// parameter expression, or the application of the function lifted for
    /// an `Int` or a `Bool` that compiles to none (`comptime e = h(n)`).
    /// `None` for a constant, or a value that denotes nothing.
    fn comptime_value_expression(&self, value: &Expr) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        let expression = self
            .compile_dependent_ct_expr(value)
            .ok()
            .or_else(|| self.lifted_application(value))
            .filter(|expression| expression.as_constant().is_none())?;
        self.record_compiled_expression(value, &expression);
        Some(expression)
    }

    /// The value a local `comptime` binding of `value` holds, given the
    /// `expression` it denotes: a struct value keeps its fields, its
    /// identity, but is spelled by the binding's lifted application, as
    /// upstream's alias reference stands for the folded value. A binder is
    /// then never solved through the construction the alias hides
    /// (`comptime L = SIMDLength(n)` does not solve `w` of `SIMDLength(w)`).
    /// Any other expression is held as it is.
    fn alias_value(&self, value: &Expr, expression: ParamExpr) -> ParamExpr {
        let mojito_types::param_expr::ParamKind::StructValue { fields, .. } = expression.kind()
        else {
            return expression;
        };
        let Some(alias) = expression
            .meta()
            .as_value()
            .and_then(|ty| self.named_application(value, &names_read(value), ty.clone()))
        else {
            return expression;
        };
        self.lifted_expressions
            .borrow_mut()
            .insert(value.source_span(), alias.clone());
        self.param_context.struct_value(fields, &alias)
    }

    /// The constant value a closed local `comptime` binding builds to
    /// (`"Lo"`, `2.5`, `n > 2` over a closed `n`). `None` for a type, and
    /// for an expression the builder does not fold.
    fn closed_constant_expression(&self, value: &Expr) -> Option<ParamExpr> {
        self.compile_dependent_ct_expr(value)
            .ok()
            .filter(|expression| {
                expression.as_constant().is_some() && expression.meta().as_value().is_some()
            })
    }

    /// Type the display a local `comptime` binding of a template body holds
    /// (`comptime L = [n, n + 1]`): a collection display of scalar elements,
    /// or a list display of closed aggregates (`[(1, n), (n, 2)]`),
    /// which the elaborator evaluates per instance. Gives the element its
    /// sequence lists, the display's own type, and its checked construction;
    /// `None` for any other binding, which the ordinary path types. A brace
    /// annotated with any type but a set or dictionary is that type's
    /// initializer list (`comptime t: Tuple[Int, Int] = {1, 2}`), so the
    /// ordinary path types it too.
    fn evaluated_display_binding(
        &self,
        value: &Expr,
        annotation: Option<&SourceType>,
    ) -> Option<(Ty, Ty, mojito_checked::checked::SemanticAdjustment)> {
        if matches!(value.kind, ExprKind::BraceLit(_))
            && let Some(annotation) = annotation
            && self
                .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)
                .ok()
                .is_none_or(|ty| {
                    mojito_types::types::set_element(&ty).is_none()
                        && mojito_types::types::dict_elements(&ty).is_none()
                })
        {
            return None;
        }
        let element = self.display_listed_element(value)?;
        if !self.evaluated_display(value, &element) {
            return None;
        }
        let ty = self.infer(value).ok()?;
        let construction = self.fact(&value.source_span(), |facts| facts.adjustment.as_deref())?;
        Some((string_element_binder(element), ty, construction))
    }

    /// Record the display the declared local `comptime` binding `name` holds
    /// for MIR to lift (`SemanticAdjustment::ComptimeDisplay`). It is a
    /// compile-time value with no runtime form: a type or a parameter
    /// argument reads an element or the length as a parameter expression
    /// over its sequence, any other compile-time expression (a `comptime
    /// for`'s `L.__iter__()` among them) reads it in a function the
    /// elaborator runs, and
    /// `materialize[L]()` builds it where it is written.
    ///
    /// The sequence is the application of the function MIR lifts for the
    /// display, named here by the binding, to every binder in scope: the pin
    /// keeps a read of the binding symbolic the same way, so `L[0]` is not
    /// the element the display spells.
    fn record_display_binding(
        &self,
        name: &str,
        value: &Expr,
        element: &Ty,
        ty: Ty,
        construction: mojito_checked::checked::SemanticAdjustment,
    ) -> Result<BoundDisplay, TypeError> {
        let owner = self.lookup_owner(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("comptime binding '{name}' has no owner"))
        })?;
        let sequence = self.display_sequence(owner, element);
        self.record_at(value.source_span()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ComptimeDisplay {
                construction: Box::new(construction),
                sequence: sequence.clone(),
            },
        ));
        Ok(BoundDisplay {
            ty,
            sequence,
            positional: matches!(value.kind, ExprKind::ListLit(_)),
        })
    }

    /// The sequence the display binding `identity` denotes, a list of
    /// `element`s: the application of a function its declaration lifts to
    /// every binder in scope, the one the binding was first given.
    fn display_sequence(&self, identity: mojito_types::origin::OwnerId, element: &Ty) -> ParamExpr {
        use mojito_types::param_expr::MetaTy;
        let owner = self.lifted_function_owner();
        let key = (owner, identity);
        if let Some(sequence) = self.lifted_displays.borrow().get(&key) {
            return sequence.clone();
        }
        let sequence = self.param_context.apply(
            &self.lifted_function_name(&key.0),
            &self.binders_in_scope(),
            MetaTy::ParamList(Box::new(MetaTy::value(element.clone()))),
        );
        self.lifted_displays
            .borrow_mut()
            .insert(key, sequence.clone());
        sequence
    }

    /// The display binding a local `comptime` alias's `value` names
    /// (`comptime A = L`), which the alias denotes whole, with its name.
    fn aliased_display<'v>(&self, value: &'v Expr) -> Option<(&'v str, BoundDisplay)> {
        let ExprKind::Identifier(target) = &value.kind else {
            return None;
        };
        Some((target, self.display_named(target)?.clone()))
    }

    /// Every binder in scope as the reference a parameter expression names
    /// it by, outermost declaration first: the enclosing struct's
    /// parameters, then each open declaration's type parameters and
    /// non-callable value parameters, a `comptime for` index among them.
    fn binders_in_scope(&self) -> Vec<ParamExpr> {
        use mojito_types::param_expr::MetaTy;
        let mut binders: Vec<ParamExpr> = self
            .self_decls
            .iter()
            .filter_map(|decl| match decl {
                ParamDecl::Type { name, variadic, .. } => Some(self.param_context.decl_ref(
                    decl.id().clone(),
                    name.trim_start_matches('*'),
                    if *variadic {
                        MetaTy::type_list()
                    } else {
                        MetaTy::Type
                    },
                )),
                ParamDecl::Value {
                    ty,
                    callable_default: None,
                    ..
                } if !decl.binds_callables() => {
                    Some(super::annotations::value_parameter_expr(decl, ty))
                }
                ParamDecl::Value { .. } => None,
            })
            .collect();
        for (level, types) in self.tparams.iter().enumerate() {
            let packs = self.pack_params.get(level);
            let mut types: Vec<ParamExpr> = types
                .values()
                .filter_map(|ty| match ty.kind() {
                    TyKind::Param { binder, .. } => Some(
                        packs
                            .and_then(|packs| packs.get(binder.name.trim_start_matches('*')))
                            .cloned()
                            .unwrap_or_else(|| {
                                self.param_context.decl_ref(
                                    binder.id.clone(),
                                    &binder.name,
                                    MetaTy::Type,
                                )
                            }),
                    ),
                    _ => None,
                })
                .collect();
            types.sort();
            binders.extend(types);
            let mut values: Vec<ParamExpr> = self
                .vparams
                .get(level)
                .into_iter()
                .flat_map(HashMap::values)
                .filter(|value| value.as_decl_ref().is_some())
                .cloned()
                .collect();
            values.sort();
            binders.extend(values);
        }
        let mut seen = HashSet::new();
        binders.retain(|binder| {
            binder
                .as_decl_ref()
                .is_some_and(|reference| seen.insert(reference.id.clone()))
        });
        binders
    }

    /// The local binding of an evaluated display `expr` names: the binding
    /// its name resolves to, when that one is a display's.
    pub(super) fn bound_display(&self, expr: &Expr) -> Option<&BoundDisplay> {
        let ExprKind::Identifier(name) = &expr.kind else {
            return None;
        };
        self.display_named(name)
    }

    /// The local binding of an evaluated display `name` resolves to.
    pub(super) fn display_named(&self, name: &str) -> Option<&BoundDisplay> {
        self.local_comptime_displays
            .get(self.binding_scope(name)?)?
            .get(name)
    }

    /// Reject a read of a local display binding outside every compile-time
    /// position: it would materialize the whole collection, which is not
    /// implicitly copyable, as the pin rejects it.
    pub(super) fn reject_display_crossing(&self, expr: &Expr) -> Result<(), TypeError> {
        if !self.crosses_to_runtime() {
            return Ok(());
        }
        // A field read of a compile-time value is a projection, as at the
        // pin: the value itself never crosses.
        if self.projected_object.borrow().as_ref() == Some(&expr.source_span()) {
            return Ok(());
        }
        if let Some(display) = self.bound_display(expr) {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            ));
        }
        // A collection a request builds (`comptime l = mk(n)`) is never
        // implicitly copyable: it has no runtime form to read.
        let ExprKind::Identifier(name) = &expr.kind else {
            return Ok(());
        };
        // A display constant the check lifted is a parameter list, read
        // whole at its declared type.
        let requested = self
            .local_comptime_parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_comptime_parameter(name))
            .and_then(|(_, expression)| match expression.meta() {
                mojito_types::param_expr::MetaTy::ParamList(_) => self.lookup(name).cloned(),
                meta => meta.as_value().cloned(),
            })
            .filter(|ty| {
                self.lookup(name) == Some(ty)
                    && (mojito_types::types::list_element(ty).is_some()
                        || mojito_types::types::set_element(ty).is_some()
                        || mojito_types::types::dict_elements(ty).is_some()
                        || !self.is_implicitly_copyable(ty))
            });
        match requested {
            Some(ty) => Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&ty),
            )),
            None => Ok(()),
        }
    }

    /// The type an expression denotes in a compile-time position, as the
    /// one builder reads it ([`Self::type_operand_expression`]); `None` for
    /// a value, or for an expression that denotes nothing at compile time.
    /// An unknown type argument (`List[Foo]`) is the diagnostic.
    pub(super) fn comptime_type_operand(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            // A reflection handle is a compile-time value, not a type, and a
            // `comptime` alias or a trivial predicate applies to a `Bool`.
            ExprKind::TypeApply { name, .. }
                if self.comptime_aliases.contains_key(name)
                    || mojito_types::types::trivial_predicate_name(name).is_some() =>
            {
                Ok(None)
            }
            _ if matches!(expr.bracket_application(), Some(("reflect", _))) => Ok(None),
            _ if let Some(reflected) = self.reflected_type_operand(expr).transpose() => {
                reflected.map(Some)
            }
            _ => match self.type_operand_expression(expr) {
                Ok(Some(ty)) => Ok(Some(ty)),
                // An element of a compile-time `TypeList` value past its
                // end is the read's own failure.
                Err(error)
                    if matches!(error, TypeError::UnknownType(_))
                        || matches!(
                            expr.kind,
                            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. }
                        )
                        || matches!(&expr.kind, ExprKind::Call { name, .. } if name == "type_of")
                        || matches!(&expr.kind, ExprKind::Index { object, .. }
                            if matches!(self.type_list_operand(object), Ok(Some(_)))) =>
                {
                    Err(error)
                }
                Ok(None) | Err(_) => Ok(None),
            },
        }
    }

    /// Type a pack element's default construction while the pack is still a
    /// parameter: `Self.Ts[i]()`, or a `def`'s own `Ts[i]()`, is the
    /// dependent element type, and the element must be `Defaultable` by the
    /// pack's bound or a `where` clause. `None` when `pack` (read through
    /// `Self` when `through_self`) names no unbound pack or the call is not a
    /// nullary construction at one index.
    pub(super) fn infer_pack_element_construction(
        &self,
        span: SourceSpan,
        pack: &str,
        through_self: bool,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let [ParamArg::Value(index)] = param_args else {
            return None;
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return None;
        }
        let (base, spelled, reference) = if through_self {
            let declared = self.self_decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Type { name, variadic: true, .. }
                    if name.trim_start_matches('*') == pack)
            });
            let reference = self.pack_reference(pack).filter(|_| declared)?;
            (
                SourceType::SelfParam(pack.to_string()),
                format!("Self.{pack}"),
                reference,
            )
        } else {
            let reference = self
                .pack_parameter_in_scope(pack)
                .filter(|_| self.lookup(pack).is_none())?;
            (
                SourceType::Named(pack.to_string(), Vec::new()),
                pack.to_string(),
                reference,
            )
        };
        Some(
            self.ty_from_anno(&SourceType::IndexedProjection {
                base: Box::new(base),
                index: Box::new(index.clone()),
            })
            .and_then(|element| {
                let view = self
                    .opaque_element(&element)
                    .unwrap_or_else(|| element.clone());
                if self.conforms_to(&view, "Defaultable") {
                    // A symbolic element is the template's construction,
                    // which the elaborator resolves per instance; a closed
                    // one is an ordinary construction of its type.
                    if mojito_types::types::is_symbolic(&element)
                        && let Some(pack) = reference.as_decl_ref()
                    {
                        self.record_at(span).adjustment = Some(Box::new(
                            mojito_checked::checked::SemanticAdjustment::ConstructPackElement {
                                pack: pack.clone(),
                            },
                        ));
                        self.record_element_index(index);
                    }
                    Ok(element)
                } else {
                    Err(TypeError::TraitNotSatisfied {
                        param: spelled,
                        ty: element.to_string(),
                        trait_name: "Defaultable".to_string(),
                        reason: self.trait_failure_reason(&view, "Defaultable"),
                    })
                }
            }),
        )
    }

    /// Record the parameter expression a pack element construction's index
    /// denotes on the index (`i` of `Ts[i]()`, a local `comptime` binding,
    /// or the alias a `comptime for` over an iterator binds), the element
    /// the elaborator selects per instance.
    fn record_element_index(&self, index: &Expr) {
        let Ok(value) = self.compile_dependent_ct_expr(index) else {
            return;
        };
        let mut record = self.record_at(index.source_span());
        if record.adjustment.is_none() && value.as_constant().is_none() {
            record.adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value,
                    materialized: None,
                },
            ));
        }
    }

    /// `materialize[X]()` in the executable check: the crossing pass folds
    /// every operand it evaluates, so `X` is over a binder of a template
    /// body, and the call is the runtime value of that operand at its type.
    /// A whole field-name list (`materialize[names]()`) is the `Array` the
    /// elaborator constructs per instance. An operand reading
    /// `__is_run_in_comptime_interpreter` is the pin's rejection.
    pub(super) fn infer_template_materialize(
        &self,
        span: &SourceSpan,
        param_args: &[ParamArg],
    ) -> Result<Ty, TypeError> {
        let [ParamArg::Value(operand)] = param_args else {
            return Err(TypeError::Unsupported(
                "materialize[...]() takes one compile-time value".to_string(),
            ));
        };
        if let Some(ty) = self.materialized_reflection_list(span, operand)? {
            return Ok(ty);
        }
        if reads_comptime_interpreter_query(operand) {
            return Err(TypeError::DynamicParameterValue);
        }
        self.record_crossing_operand(operand);
        let _position = self.comptime_position();
        self.infer(operand)
    }

    /// Record the parameter expression a crossing's compile-time operand
    /// compiles to (`list_get(L, 0)` for `comptime(L[0])`,
    /// [`Self::record_compiled_expression`]), which the elaborator
    /// evaluates; a display crossing whole is built where it crosses.
    /// Whether the operand compiled.
    fn record_crossing_operand(&self, operand: &Expr) -> bool {
        if self.bound_display(operand).is_some() {
            return false;
        }
        let compiled = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(operand).ok()
        };
        compiled
            .map(|expression| self.record_compiled_expression(operand, &expression))
            .is_some()
    }

    /// `comptime(e)` in the executable check: one the crossing pass left is
    /// over a binder of a template body, and the call is the runtime value
    /// of its compile-time operand, which reads compile-time bindings alone.
    /// A display binding does not cross whole this way.
    pub(super) fn infer_template_comptime(&self, operand: &Expr) -> Result<Ty, TypeError> {
        if !self.reads_compile_time_alone(operand) {
            return Err(TypeError::NotComptime(
                "a 'comptime(...)' operand that reads a runtime value".to_string(),
            ));
        }
        // An operand that applies a callable and compiles to no parameter
        // expression is the application MIR lifts, which the elaborator
        // below MIR evaluates.
        if !self.record_crossing_operand(operand)
            && self.applies_callable(operand)
            && self.lifted_application(operand).is_none()
        {
            self.requested_binding(operand, ConstantScope::Local)?;
        }
        let ty = {
            let _position = self.comptime_position();
            self.infer(operand)?
        };
        if let Some(display) = self.bound_display(operand) {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            ));
        }
        // The runtime value of a compile-time operand is its implicit copy,
        // as the pin materializes one: a collection or a struct that is
        // not `ImplicitlyCopyable` does not cross.
        let resolved = mojito_types::types::default_literal(&ty);
        if self.crosses_to_runtime()
            && !mojito_types::types::is_symbolic(&resolved)
            && !self.is_implicitly_copyable(&resolved)
        {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&resolved),
            ));
        }
        Ok(ty)
    }

    /// The parameter-list reference of the variadic pack a spread names: a
    /// pack of an enclosing `def` or method first, then the enclosing
    /// struct's own.
    pub(super) fn pack_reference(&self, pack: &str) -> Option<ParamExpr> {
        let bare = pack.trim_start_matches('*');
        self.pack_parameter_in_scope(bare).or_else(|| {
            pack_scope(&self.self_decls)
                .remove(bare)
                .map(|reference| self.param_context.intern(&reference))
        })
    }

    /// Whether `list` is the parameter-list reference of a variadic pack in
    /// scope ([`Self::pack_reference`] of its spelling).
    pub(super) fn pack_in_scope(&self, list: &ParamExpr) -> bool {
        list.as_decl_ref().is_some_and(|reference| {
            self.pack_reference(&reference.name).is_some_and(|found| {
                found
                    .as_decl_ref()
                    .is_some_and(|found| found.id == reference.id)
            })
        })
    }

    /// The raw list type a type pack's `Ts.values` has, as upstream types
    /// it: a list of the pack's bound used as a type
    /// (`KGENParamList[AnyType]`), or of the trait-valued binder bounding it
    /// (`*Ts: Tr` lists `Tr`). `None` when `pack` names no type pack.
    pub(super) fn type_pack_list_ty(&self, pack: &ParamExpr) -> Option<Ty> {
        let element = match self.type_pack_trait(pack)? {
            CtValue::Expr(reference) => mojito_types::types::DependentType::resolve(reference),
            CtValue::Trait(set) => TyKind::Trait(mojito_types::types::trait_set(set)).into(),
            _ => return None,
        };
        Some(TyKind::ParamList(element).into())
    }

    /// The `Trait` argument of the `TypeList` a type pack is: the
    /// trait-valued binder bounding it (`*Ts: Tr`), else the trait value its
    /// declared bound composes. `None` when `pack` names no type pack.
    pub(super) fn type_pack_trait(&self, pack: &ParamExpr) -> Option<CtValue> {
        let binder = pack.as_decl_ref()?;
        if pack.meta() != &mojito_types::param_expr::MetaTy::type_list() {
            return None;
        }
        if let Some(trait_binder) = self.trait_bounded_binders.get(&binder.id) {
            let reference = self
                .value_parameter_in_scope(&trait_binder.name)
                .filter(|reference| {
                    reference
                        .as_decl_ref()
                        .is_some_and(|reference| reference.id == trait_binder.id)
                })
                .map(CtValue::Expr)
                .or_else(|| self.self_param_value(&trait_binder.name))?;
            return Some(match reference {
                CtValue::Expr(expr) => CtValue::Expr(expr),
                value => CtValue::Expr(self.param_context.constant(value).ok()?),
            });
        }
        let bounds = self
            .tparams
            .iter()
            .flat_map(HashMap::values)
            .find_map(|ty| match ty.kind() {
                TyKind::Param {
                    binder: declared,
                    bounds,
                    ..
                } if declared.id == binder.id => Some(bounds.clone()),
                _ => None,
            })
            .or_else(|| {
                self.self_decls.iter().find_map(|decl| match decl {
                    ParamDecl::Type { id, bounds, .. } if id == &binder.id => Some(bounds.clone()),
                    _ => None,
                })
            })?;
        Some(CtValue::Trait(self.canonical_trait_set(&bounds)))
    }

    /// The bundled `TypeList` a type pack is, as upstream's variadic type
    /// parameter is one: `TypeList[Trait, Ts.values]`. `None` when `pack`
    /// names no type pack.
    pub(super) fn type_pack_type_list(&self, pack: &ParamExpr) -> Option<Ty> {
        let trait_value = self.type_pack_trait(pack)?;
        Some(
            TyKind::Struct(
                TYPE_LIST.to_string(),
                vec![
                    TyArg::Val(trait_value),
                    TyArg::Val(CtValue::Expr(pack.clone())),
                ],
            )
            .into(),
        )
    }

    /// Element `index` of a type pack that is still a parameter, as upstream
    /// reads `Ts[index]`: the bundled `TypeList`'s `__getitem_param__`
    /// applied at the index.
    pub(super) fn type_pack_element(
        &self,
        pack: &ParamExpr,
        index: &ParamExpr,
    ) -> Result<Ty, TypeError> {
        let type_list = self
            .type_pack_type_list(pack)
            .ok_or_else(|| TypeError::InvariantViolation(format!("'{pack}' is not a type pack")))?;
        let index = index
            .as_constant()
            .cloned()
            .unwrap_or_else(|| CtValue::Expr(index.clone()));
        self.associated_type_from_base(&type_list, "__getitem_param__", &[TyArg::Val(index)])
    }

    /// The type pack an expression names by itself while it is still a
    /// parameter: a bare `Ts` of an enclosing `def` or method, or the
    /// enclosing struct's `Self.Ts`. The counterpart of
    /// [`Self::value_pack_named`].
    pub(super) fn type_pack_named(&self, expr: &Expr) -> Option<ParamExpr> {
        match &expr.kind {
            ExprKind::Identifier(name) if self.lookup(name).is_none() => {
                self.pack_parameter_in_scope(name)
            }
            ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                    && self.self_decls.iter().any(|decl| {
                        matches!(decl, ParamDecl::Type { name, variadic: true, .. }
                            if name.trim_start_matches('*') == field)
                    }) =>
            {
                self.pack_reference(field)
            }
            _ => None,
        }
    }

    /// The value pack an expression names while it is still a parameter: a
    /// `def`'s own `*values: Int`, typed as the pack of its element, which
    /// no instance has bound yet, or the enclosing struct's `Self.values`.
    pub(super) fn value_pack_named(&self, expr: &Expr) -> Option<ParamExpr> {
        if let ExprKind::Member { object, field } = &expr.kind
            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
        {
            return self.self_value_pack(field);
        }
        let ExprKind::Identifier(name) = &expr.kind else {
            return None;
        };
        // A signature binds no local: there the name is the binder alone.
        match self.lookup(name).map(Ty::kind) {
            Some(TyKind::VariadicPack(element))
                if mojito_types::types::pack_spread(std::slice::from_ref(element)).is_none() => {}
            Some(_) => return None,
            None => {}
        }
        self.value_pack_in_scope(name)
    }

    /// The raw parameter list a bare name denotes when it is no runtime
    /// binding and no pack: a `values: KGENParamList[T]` parameter, or a
    /// local `comptime raw = vs.values`.
    pub(super) fn raw_list_named(&self, name: &str) -> Option<ParamExpr> {
        if self.lookup(name).is_some() || self.value_pack_in_scope(name).is_some() {
            return None;
        }
        self.value_parameter_in_scope(name)
            .filter(|raw| matches!(raw.meta(), mojito_types::param_expr::MetaTy::ParamList(_)))
    }

    /// A value pack read as a runtime value (`for v in values`, `var l =
    /// values`, `values[i]` at a runtime index): the zero-sized
    /// `ParameterList[values.values]()` the pin types the read as, spelled in
    /// the read's place and checked as any construction is.
    pub(super) fn infer_value_pack_read(&self, expression: &Expr) -> Result<Ty, TypeError> {
        self.infer_spelled_pack_read(expression, PARAMETER_LIST)
    }

    /// A type pack read as a value (`var l = Ts`, `len(Ts)` at run time,
    /// `show(Self.Ts)`): the zero-sized `TypeList[Ts.values]()` the pin
    /// types the read as, spelled as a value pack read is.
    pub(super) fn infer_type_pack_read(&self, expression: &Expr) -> Result<Ty, TypeError> {
        self.infer_spelled_pack_read(expression, TYPE_LIST)
    }

    /// A run-time read of a `comptime` binding of a `TypeList` value
    /// (`comptime tl = Ts`, `comptime types = TypeList.of[...]()`): the
    /// binding's initializer spelled again in the read's place, under
    /// identities derived from the read's. The pin keeps one parameter
    /// attribute per declaration and materializes it at each read; for the
    /// zero-sized `TypeList`, building the construction again at each read
    /// is the same value. `None` when `name` binds no `TypeList` value.
    pub(super) fn infer_type_list_binding_read(
        &self,
        expression: &Expr,
        name: &str,
    ) -> Option<Result<Ty, TypeError>> {
        let bound = self.local_comptime_value(name)?;
        if !matches!(self.type_list_operand(bound), Ok(Some(_))) {
            return None;
        }
        let computed = match &bound.kind {
            ExprKind::MethodCall { .. } => true,
            ExprKind::Invoke { callee, .. } => {
                matches!(&callee.kind, ExprKind::Member { field, .. } if field == "_concat")
            }
            _ => false,
        };
        if computed {
            return Some(Err(TypeError::Unsupported(format!(
                "a computed TypeList read at run time ('{name}')"
            ))));
        }
        let mut spelled = bound.clone();
        mojito_ast::visit::walk_expr_mut(
            &mut mojito_ast::visit::DerivedIdentities {
                parent: expression.syntax_id,
                next: 1,
            },
            &mut spelled,
        );
        Some(
            self.infer(&spelled)
                .inspect(|ty| self.record_spelled_construction(expression, spelled, ty)),
        )
    }

    /// The raw parameter list a bracket argument names: the enclosing
    /// struct's own `Self.values`, or any compile-time value that is one
    /// (`vs.values`, a `raw: KGENParamList[T]` parameter).
    pub(super) fn raw_list_argument(&self, argument: &ParamArg) -> Option<ParamExpr> {
        match argument {
            ParamArg::Type(SourceType::SelfParam(name)) => self.self_raw_list(name),
            ParamArg::Value(expr) => self.compile_dependent_ct_expr(expr).ok().filter(|raw| {
                raw.as_decl_ref().is_some()
                    && matches!(raw.meta(), mojito_types::param_expr::MetaTy::ParamList(_))
            }),
            _ => None,
        }
    }

    /// The element type and raw list of a `ParameterList` construction a
    /// bracket argument spreads (`*ParameterList[Self.values]()`), which
    /// binds a variadic value parameter to that list whole, as upstream's
    /// `_ParameterListIter[*ParameterList[...]()]` does. `None` for any other
    /// argument.
    pub(super) fn parameter_list_spread(
        &self,
        argument: &ParamArg,
    ) -> Option<Result<(Ty, CtValue), TypeError>> {
        let ParamArg::Value(Expr {
            kind: ExprKind::Spread(operand),
            ..
        }) = argument
        else {
            return None;
        };
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &operand.kind
        else {
            return None;
        };
        if name != PARAMETER_LIST || !args.is_empty() || !kwargs.is_empty() {
            return None;
        }
        Some(
            self.applied_type(name, param_args)
                .and_then(|(ty, _)| match ty.kind() {
                    TyKind::Struct(_, arguments) => match arguments.as_slice() {
                        [TyArg::Ty(element), TyArg::Val(raw), ..] => {
                            Ok((element.clone(), raw.clone()))
                        }
                        _ => Err(TypeError::InvariantViolation(format!(
                            "'{ty}' binds no raw parameter list"
                        ))),
                    },
                    _ => Err(TypeError::InvariantViolation(format!(
                        "'{ty}' is not a 'ParameterList'"
                    ))),
                }),
        )
    }

    /// The value pack a bracket argument spreads whole: `*vs` of a `def`'s
    /// or method's own value pack, or `*Self.vs` of the enclosing struct's.
    pub(super) fn value_pack_spread(&self, argument: &ParamArg) -> Option<ParamExpr> {
        match argument {
            // The struct's own pack arrives here too: pack qualification
            // folds `*Self.vs` onto the bare spread.
            ParamArg::Type(SourceType::Named(name, args)) if args.is_empty() => {
                let name = name.strip_prefix('*')?;
                self.value_pack_named(&Expr::new(
                    ExprKind::Identifier(name.to_string()),
                    mojito_common::token::DUMMY_SPAN,
                ))
                .or_else(|| self.self_value_pack(name))
            }
            ParamArg::Type(SourceType::SelfParam(name)) => {
                self.self_value_pack(name.strip_prefix('*')?)
            }
            _ => None,
        }
    }

    /// The enclosing struct's own value pack or raw parameter list
    /// `Self.<field>`, as the parameter-list reference its binder declares.
    pub(super) fn self_raw_list(&self, field: &str) -> Option<ParamExpr> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Value { ty, .. }
                if decl.name().trim_start_matches('*') == field
                    && matches!(
                        decl.binder_meta(),
                        mojito_types::param_expr::MetaTy::ParamList(_)
                    ) =>
            {
                Some(
                    self.param_context
                        .intern(&super::annotations::value_parameter_expr(decl, ty)),
                )
            }
            _ => None,
        })
    }

    /// The enclosing struct's own value pack `Self.<field>`, as the
    /// parameter-list reference its binder declares.
    pub(super) fn self_value_pack(&self, field: &str) -> Option<ParamExpr> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: true,
                ..
            } if name.trim_start_matches('*') == field => Some(
                self.param_context
                    .intern(&super::annotations::value_parameter_expr(decl, ty)),
            ),
            _ => None,
        })
    }

    /// Record a query of a pack that is still a parameter, read as a runtime
    /// value at `span`: the constant MIR carries and the elaborator folds.
    pub(super) fn record_pack_query_value(
        &self,
        span: SourceSpan,
        pack: &ParamExpr,
        query: mojito_types::param_expr::PackQuery,
    ) {
        if let Some(pack) = pack.as_decl_ref() {
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value: self.param_context.pack_query(pack, query),
                    materialized: None,
                },
            ));
        }
    }

    /// The pack query a `TypeList` proposition over a pack asks, when its
    /// operand names a type.
    pub(super) fn typelist_proposition_query(
        &self,
        proposition: GenericConstraint,
    ) -> Option<mojito_types::param_expr::PackQuery> {
        use mojito_types::param_expr::PackQuery;
        Some(match proposition {
            GenericConstraint::ConformsPack { trait_name, .. } => PackQuery::Conforms(trait_name),
            GenericConstraint::PackPredicate { predicate, all, .. } => {
                PackQuery::Predicate { predicate, all }
            }
            GenericConstraint::PackContains { element, .. } => {
                let element = match element {
                    ConstraintOperand::Type(ty) => ty,
                    ConstraintOperand::Param(param) => self
                        .ty_from_anno(&mojito_ast::ast::Type::Named(
                            param.name.to_string(),
                            Vec::new(),
                        ))
                        .ok()?,
                    _ => return None,
                };
                PackQuery::Contains(self.param_context.type_shape(element))
            }
            _ => return None,
        })
    }

    /// The type of element `index` of a variadic pack that is still a
    /// parameter: [`Self::type_pack_element`] at an index that is a
    /// compile-time expression over the parameters and `comptime for`
    /// variables in scope.
    pub(super) fn pack_element_type(&self, pack: &Ty, index: &Expr) -> Result<Ty, TypeError> {
        let TyKind::Param { binder, .. } = pack.kind() else {
            return Err(TypeError::InvariantViolation(format!(
                "'{pack}' is not a variadic pack parameter"
            )));
        };
        let name = &binder.name;
        let list = self
            .pack_reference(name)
            .ok_or_else(|| TypeError::SymbolicBoundary(format!("pack '{name}' is not in scope")))?;
        let index = self
            .compile_dependent_ct_expr(index)
            .map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "variadic-pack index".to_string(),
            })?;
        self.type_pack_element(&list, &index)
    }

    /// Whether a spread's operand (`*args`, `*args^`) is a binding of a pack
    /// that is still a parameter.
    pub(super) fn spreads_unbound_pack(&self, spread: &Expr) -> bool {
        self.forwarded_pack_operand(spread).is_some()
    }

    /// Note a positional collector declared `var *args` once it is bound.
    pub(super) fn record_owned_pack(
        &mut self,
        name: &str,
        kind: mojito_ast::ast::ParamKind,
        convention: Option<ArgConvention>,
    ) {
        if kind == mojito_ast::ast::ParamKind::Variadic
            && matches!(convention, Some(ArgConvention::Var))
            && let Some(owner) = self.lookup_owner(name)
        {
            self.owned_packs.insert(owner);
        }
    }

    /// Note a function's positional collector as it is declared (`None` when
    /// it has no collector): its parameter name and whether it is `var`.
    /// Overloads of one name that disagree leave the name unrecorded.
    pub(super) fn record_positional_collector(
        &mut self,
        name: &str,
        collector: Option<(&str, bool)>,
    ) {
        let Some((parameter, owned)) = collector else {
            return;
        };
        record_agreed(&mut self.owned_collectors, name, owned);
        record_agreed(&mut self.collector_names, name, parameter.to_string());
    }

    /// A callee with no pack collector cannot take a forwarded pack: the
    /// binding's own diagnostics say why (no collector, or a homogeneous
    /// one).
    pub(super) fn reject_forwarded_pack(
        &self,
        callee: &str,
        args: &[Expr],
        collector: Option<&Ty>,
    ) -> Result<(), TypeError> {
        if let Some((position, forwarded)) =
            self.forwarded_pack_argument(callee, args, collector.is_some())?
            && let Some(collector) = collector
        {
            self.bind_forwarded_pack(callee, &args[position], &forwarded, collector, None)?;
        }
        Ok(())
    }

    /// The pack a call argument forwards whole (`*args`, `*args^`) when that
    /// pack is still a parameter, or `None` for any other argument.
    pub(super) fn forwarded_pack(&self, argument: &Expr) -> Option<ForwardedPack> {
        match &argument.kind {
            ExprKind::Spread(spread) => self.forwarded_pack_operand(spread),
            _ => None,
        }
    }

    /// [`Self::forwarded_pack`] for the spread's operand (`args`, `args^`).
    fn forwarded_pack_operand(&self, spread: &Expr) -> Option<ForwardedPack> {
        let (source, transferred) = match &spread.kind {
            ExprKind::Transfer(inner) => (&**inner, true),
            _ => (spread, false),
        };
        let ExprKind::Identifier(binding) = &source.kind else {
            return None;
        };
        let Some(TyKind::VariadicPack(element)) = self.lookup(binding).map(Ty::kind) else {
            return None;
        };
        let element = mojito_types::types::pack_spread(std::slice::from_ref(element))?;
        let TyKind::Param { binder, .. } = element.kind() else {
            return None;
        };
        // The binder in scope carries the declared bounds; a pack reached
        // through a capture keeps the bounds its binding type has.
        let pack = self
            .lookup_tparam(&binder.name)
            .unwrap_or_else(|| element.clone());
        Some(ForwardedPack {
            binding: binding.clone(),
            pack,
            transferred,
            owned: self
                .lookup_owner(binding)
                .is_some_and(|owner| self.owned_packs.contains(&owner)),
        })
    }

    /// The pack a call forwards among its positional arguments, with its
    /// place in the argument list checked: one spread, last, and only where
    /// the callee has a positional collector. `None` when no argument
    /// forwards a pack that is still a parameter.
    pub(super) fn forwarded_pack_argument(
        &self,
        callee: &str,
        args: &[Expr],
        has_collector: bool,
    ) -> Result<Option<(usize, ForwardedPack)>, TypeError> {
        let spreads: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, argument)| self.forwarded_pack(argument).is_some())
            .map(|(position, _)| position)
            .collect();
        let Some(position) = mojito_ast::call::spread_position(&spreads, args.len())
            .map_err(|error| error.into_type_error(callee))?
        else {
            return Ok(None);
        };
        if !has_collector {
            return Err(mojito_ast::call::MatchError::SpreadOutsideVariadic.into_type_error(callee));
        }
        let forwarded = self
            .forwarded_pack(&args[position])
            .expect("selected among the forwarded arguments");
        Ok(Some((position, forwarded)))
    }

    /// Bind a forwarded pack to a callee's positional collector: the
    /// collector is itself a pack (`*args: *Us`), its ownership agrees with
    /// the caller's (`var` needs the `^`, a read pack cannot be transferred,
    /// and a collector known to be `var` or read — `collector_owned` — takes
    /// the same kind of pack), and every bound it declares holds of the
    /// caller's pack. The whole pack is the collector's one actual, and
    /// `argument` (the spread) is typed as it so a later reading of the
    /// call's arguments finds the binding rather than the bare spread.
    pub(super) fn bind_forwarded_pack(
        &self,
        callee: &str,
        argument: &Expr,
        forwarded: &ForwardedPack,
        collector: &Ty,
        collector_owned: Option<bool>,
    ) -> Result<Ty, TypeError> {
        let TyKind::Param {
            bounds: expected, ..
        } = collector.kind()
        else {
            return Err(TypeError::TypeMismatch {
                expected: format!("VariadicList[{collector}]"),
                found: format!("a variadic pack ('{}')", forwarded.binding),
                context: format!("argument to '{callee}'"),
            });
        };
        if !forwarded.owned && forwarded.transferred {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: format!(
                    "cannot transfer out of the read pack '{}'",
                    forwarded.binding
                ),
            });
        }
        if collector_owned.is_some_and(|owned| owned != forwarded.owned) {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "cannot unpack a variadic pack into a call that requires a different \
                         ownership"
                    .to_string(),
            });
        }
        if forwarded.owned && !forwarded.transferred {
            return Err(TypeError::ImplicitCopy {
                ty: format!("VariadicPack[{}]", forwarded.pack),
                context: format!("argument to '{callee}'"),
                transferable: true,
                copyable: false,
            });
        }
        // The collector is the forwarded pack itself retyped by a struct's
        // pack (`*b: *Self.Ts` on `V[*Ts]`): pack types are invariant in
        // their bounds.
        let retyped = matches!((collector.kind(), forwarded.pack.kind()),
            (TyKind::Param { binder, bounds, .. }, TyKind::Param { binder: own, bounds: own_bounds, .. })
                if binder.id == own.id && !same_bounds(bounds, own_bounds));
        if retyped
            || expected
                .iter()
                .any(|bound| !self.conforms_to(&forwarded.pack, bound))
        {
            let declared = match forwarded.pack.kind() {
                TyKind::Param { bounds, .. } if !bounds.is_empty() => bounds.join(" & "),
                _ => "AnyType".to_string(),
            };
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: format!(
                    "cannot unpack a pack of type '{declared}' into a call that expects a pack of \
                     type '{}'",
                    expected.join(" & ")
                ),
            });
        }
        self.record_at(argument.source_span()).ty = Some(forwarded.pack.clone());
        Ok(forwarded.pack.clone())
    }

    /// The compiler-private storage aggregate constructed from a whole pack
    /// that is still a parameter (`__RuntimeTuple(*args^)`, in `Tuple`'s own
    /// `__init__`): storage over that pack. A declared struct's construction
    /// from a forwarded pack selects its `__init__` (`infer_construction`).
    /// `None` for every other call.
    pub(super) fn infer_unbound_pack_construction(
        &self,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let [
            Expr {
                kind: ExprKind::Spread(spread),
                ..
            },
        ] = args
        else {
            return None;
        };
        let source = match &spread.kind {
            ExprKind::Transfer(inner) => inner,
            _ => spread,
        };
        let ExprKind::Identifier(binding) = &source.kind else {
            return None;
        };
        let Some(TyKind::VariadicPack(element)) = self.lookup(binding).map(Ty::kind) else {
            return None;
        };
        let pack = mojito_types::types::pack_spread(std::slice::from_ref(element))?.clone();
        let boundary = || {
            TypeError::SymbolicBoundary(format!(
                "'{name}' called with a spread of the unbound pack '{binding}'"
            ))
        };
        let constructed = match name {
            "__RuntimeTuple" => Ty::new(TyKind::Tuple(vec![pack])),
            // Any other callee binds the forwarded pack to its own collector
            // (`forwarded_pack_argument`, `bind_forwarded_pack`), a struct
            // through its declared `__init__` (`infer_construction`).
            _ => return None,
        };
        if !kwargs.is_empty() {
            return Some(Err(boundary()));
        }
        if param_args.is_empty() {
            return Some(Ok(constructed));
        }
        Some(
            self.ty_from_anno(&SourceType::Named(name.to_string(), param_args.to_vec()))
                .and_then(|declared| {
                    if declared == constructed {
                        Ok(constructed)
                    } else {
                        Err(TypeError::TypeMismatch {
                            expected: declared.to_string(),
                            found: constructed.to_string(),
                            context: format!("spread of '{binding}'"),
                        })
                    }
                }),
        )
    }

    /// Close the pack elements a callee's type names (`Self.Ts[index]`)
    /// under a use's arguments: a value argument binds its parameter, a
    /// concrete pack its list, and a pack forwarded as a spread the caller's
    /// own pack. A type with no pack element is returned as it is. `scopes`
    /// pair declaration lists with their arguments; a later scope's binder
    /// shadows an earlier one's of the same spelling, the key the dependent
    /// type's value environment reads.
    pub(super) fn close_pack_elements(&self, ty: Ty, scopes: &[(&[ParamDecl], &[TyArg])]) -> Ty {
        let names_element = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Dependent(dependent) if dependent.pack_element().is_some())
                || mojito_types::types::list_spread(ty).is_some()
        };
        if !mojito_types::types::mentions(&ty, &names_element) {
            return ty;
        }
        let values: HashMap<String, CtValue> = scopes
            .iter()
            .flat_map(|(decls, arguments)| decls.iter().zip(arguments.iter()))
            .filter_map(|(decl, argument)| {
                let name = decl.name().trim_start_matches('*').to_string();
                match argument {
                    TyArg::Val(value) => Some((name, value.clone())),
                    TyArg::Ty(ty)
                        if let TyKind::Param { binder, .. } = ty.kind()
                            && binder.name.starts_with('*') =>
                    {
                        self.pack_reference(&binder.name)
                            .map(|reference| (name, CtValue::Expr(reference)))
                    }
                    // A pack bound to a list computed from other packs.
                    TyArg::Ty(ty) if let Some(list) = mojito_types::types::list_spread(ty) => {
                        Some((name, CtValue::Expr(list.clone())))
                    }
                    TyArg::Ty(_) | TyArg::Origin(_) => None,
                }
            })
            .collect();
        self.resolve_dependent_ty(&ty, &values).unwrap_or(ty)
    }

    /// The bounded type parameter an element of an unbound variadic pack, of
    /// a bound's raw-list member (`TypeList[Trait=AnyType, T._types]()[0]`),
    /// or a reflected field type of a symbolic subject, behaves as: the
    /// pack's declared bounds, or the trait the member's declared list holds
    /// (a field type has none), plus every trait a `where` clause guarantees
    /// of the pack's elements or a `comptime if conforms_to` arm proves of
    /// this element. The dependent type stays the element's identity; this
    /// is what its capabilities are read from.
    pub(super) fn opaque_element(&self, ty: &Ty) -> Option<Ty> {
        let TyKind::Dependent(dependent) = ty.kind() else {
            return None;
        };
        let reflected = |node: &ParamExpr| {
            matches!(
                node.kind(),
                mojito_types::param_expr::ParamKind::Reflect { .. }
            )
        };
        let list = dependent.pack_element().map(|(list, _)| list);
        let pack = list.and_then(ParamExpr::as_decl_ref);
        let member_traits = list.and_then(|list| self.member_list_traits(list));
        if pack.is_none()
            && member_traits.is_none()
            && !reflected(dependent.expr())
            && !list.is_some_and(reflected)
        {
            return None;
        }
        let pack_name = pack.map(|reference| reference.name.to_string());
        let declared = pack_name.as_deref().and_then(|pack| {
            self.lookup_tparam(&format!("*{pack}")).or_else(|| {
                self.self_decls
                    .iter()
                    .filter(|decl| {
                        matches!(decl, ParamDecl::Type { variadic: true, .. })
                            && decl.name().trim_start_matches('*') == pack
                    })
                    .find_map(type_parameter)
            })
        });
        let mut bounds = match declared.as_ref().map(Ty::kind) {
            Some(TyKind::Param { bounds, .. }) => bounds.clone(),
            _ => member_traits.unwrap_or_default(),
        };
        let spelled = dependent.expr().to_string();
        for known in self.assumptions.borrow().iter().flatten() {
            let guaranteed = match known.kind() {
                ParamKind::Conforms {
                    subject,
                    trait_name,
                } if subject == dependent.expr() => Some(trait_name.clone()),
                _ => super::traits::binder_fact(known)
                    .filter(|(id, _)| pack.is_some_and(|reference| &reference.id == id))
                    .map(|(_, trait_name)| trait_name),
            };
            if let Some(guaranteed) = guaranteed
                && !bounds.contains(&guaranteed)
            {
                bounds.push(guaranteed);
            }
        }
        // Two packs may share a spelling (`Tuple.Ts`, `Bag.Ts`); a view's name
        // is its element's alone, so the owner qualifies the later one.
        let mut views = self.pack_element_views.borrow_mut();
        let name = match (views.get(&spelled), pack) {
            (Some(viewed), Some(reference)) if viewed != ty => {
                format!("{}.{spelled}", reference.id.owner)
            }
            _ => spelled,
        };
        views.insert(name.clone(), ty.clone());
        Some(
            TyKind::Param {
                binder: pack_element_view_binder(&name),
                bounds,
                callable_bound: None,
            }
            .into(),
        )
    }

    /// Reject a type-argument list that spreads a pack beside other
    /// arguments (`Tuple[Int, *Self.Ts]`), as the pinned Mojo does.
    pub(super) fn reject_mixed_spread(name: &str, arguments: &[Ty]) -> Result<(), TypeError> {
        let spreads = arguments
            .iter()
            .any(|ty| mojito_types::types::pack_spread(std::slice::from_ref(ty)).is_some());
        if spreads && arguments.len() > 1 {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "a variadic pack spread must be the only argument it binds".to_string(),
            });
        }
        Ok(())
    }

    /// Give a type computed over bounded views of pack elements its
    /// dependent elements back: a view is how an element's capabilities are
    /// read, never a type a program's values have.
    pub(super) fn restore_pack_elements(&self, ty: Ty) -> Ty {
        let views = self.pack_element_views.borrow();
        if views.is_empty() {
            return ty;
        }
        let elements: TySubst = views
            .iter()
            .map(|(name, element)| (pack_element_view_binder(name).id, element.clone()))
            .collect();
        substitute(&ty, &elements)
    }

    /// Whether a nominal type has no checkable declaration here: unregistered
    /// (a discovery-round abstract scalar range). Its iteration and subscript
    /// contracts come from the family, not from method lookup.
    pub(super) fn is_abstract_struct(&self, name: &str) -> bool {
        !self.structs.contains_key(name)
    }

    /// Every `Param` operand of a compiled condition must name a parameter,
    /// binding, type, trait, or alias in scope: the constraint compiler
    /// reads any bare identifier as a parameter name.
    /// A pack read at run time as the construction `<list>[<pack>.values]()`
    /// of the bundled list struct `list`, spelled in the read's place under
    /// the read's derived ordinals and checked as any construction is.
    fn infer_spelled_pack_read(&self, expression: &Expr, list: &str) -> Result<Ty, TypeError> {
        if !self.structs.contains_key(list) {
            return Err(TypeError::Unsupported(format!(
                "a pack read at run time needs the bundled '{VARIADICS_MODULE}' module's '{list}'"
            )));
        }
        if !matches!(
            expression.kind,
            ExprKind::Member { .. } | ExprKind::Identifier(_)
        ) {
            return Err(TypeError::InvariantViolation(
                "a pack read names no pack".to_string(),
            ));
        }
        let pack = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::PACK_READ_OPERAND_ORDINAL,
            expression.kind.clone(),
            expression.span,
        );
        let raw = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::PACK_READ_RAW_LIST_ORDINAL,
            ExprKind::Member {
                object: Box::new(pack),
                field: "values".to_string(),
            },
            expression.span,
        );
        let construction = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::PACK_READ_ORDINAL,
            ExprKind::Call {
                name: list.to_string(),
                param_args: vec![ParamArg::Value(raw)],
                args: Vec::new(),
                kwargs: Vec::new(),
            },
            expression.span,
        );
        let ty = self.infer(&construction)?;
        self.record_spelled_construction(expression, construction, &ty);
        Ok(ty)
    }

    fn constraint_operands_resolve(&self, constraint: &GenericConstraint) -> Result<(), TypeError> {
        Self::each_constraint_parameter(constraint, &|name| self.comptime_name_resolves(name))
    }

    /// Whether a compiled condition names, as a parameter, a local
    /// `comptime` binding of the executable check that denotes no parameter
    /// expression.
    fn names_evaluated_binding(&self, constraint: &GenericConstraint) -> bool {
        Self::each_constraint_parameter(constraint, &|name| {
            let evaluated = self
                .lookup_owner(name)
                .is_some_and(|owner| self.is_comptime_binding_owner(owner))
                && !self
                    .local_comptime_parameters
                    .iter()
                    .any(|scope| scope.contains_key(name))
                && !self.installs(interface::Table::ComptimeParameters, name);
            if evaluated {
                Err(TypeError::NotComptime(name.to_string()))
            } else {
                Ok(())
            }
        })
        .is_err()
    }

    /// Whether a compiled condition names, as a parameter, a name no binder
    /// in scope declares once its operands are bound.
    fn names_unbound_parameter(&self, constraint: &GenericConstraint) -> bool {
        let bound = |reference: &ParamRef| {
            if reference.is_unbound() {
                Err(())
            } else {
                Ok(reference.clone())
            }
        };
        self.bind_constraint(constraint, &[])
            .try_map(&|operand| match operand {
                ConstraintOperand::Param(reference) => bound(reference).map(|_| operand.clone()),
                _ => Ok(operand.clone()),
            })
            .is_err()
    }

    /// Visit the name of every `Param` operand of a compiled condition,
    /// stopping at the first one `visit` rejects.
    fn each_constraint_parameter(
        constraint: &GenericConstraint,
        visit: &dyn Fn(&str) -> Result<(), TypeError>,
    ) -> Result<(), TypeError> {
        constraint
            .try_map(&|operand| match operand {
                ConstraintOperand::Param(param) => visit(&param.name).map(|()| operand.clone()),
                // An arithmetic operand resolved its names when it was
                // compiled.
                _ => Ok(operand.clone()),
            })
            .map(|_| ())
    }

    fn comptime_name_resolves(&self, name: &str) -> Result<(), TypeError> {
        let name = name.trim_start_matches('*');
        let known = self.lookup_tparam(name).is_some()
            || self.lookup(name).is_some()
            || self.comptimes.contains_key(name)
            || self.comptime_aliases.contains_key(name)
            || self.structs.contains_key(name)
            || self.traits.contains_key(name)
            || self.self_decls.iter().any(|decl| decl.name() == name)
            || self
                .enclosing_type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == name);
        if known {
            Ok(())
        } else {
            Err(TypeError::UndefinedVariable(name.to_string()))
        }
    }

    /// Whether a condition reads a type-valued operand (`T == Int`,
    /// `Self.Ts[i] == T`) or a `TypeList` value (`tl.length == 2`,
    /// `tl.contains[Int]()`), so its diagnosis belongs to the constraint
    /// compiler rather than to value typing.
    fn condition_is_compile_time_shaped(&self, cond: &Expr) -> bool {
        let compile_time = |expr: &Expr| match &expr.kind {
            ExprKind::Member { object, .. } | ExprKind::MethodCall { object, .. } => {
                matches!(self.typelist_receiver(object), Ok(Some(_)))
            }
            ExprKind::Invoke { callee, .. } => {
                matches!(&callee.kind,
                ExprKind::Member { object, .. } | ExprKind::Index { object, .. }
                    if matches!(self.typelist_receiver(object), Ok(Some(_))))
                    || matches!(&callee.kind, ExprKind::Index { object, .. }
                    if matches!(&object.kind, ExprKind::Member { object, .. }
                        if matches!(self.typelist_receiver(object), Ok(Some(_)))))
            }
            _ => matches!(self.comptime_type_operand(expr), Ok(Some(_))),
        };
        match &cond.kind {
            ExprKind::Infix(_, left, right) => {
                self.condition_is_compile_time_shaped(left)
                    || self.condition_is_compile_time_shaped(right)
            }
            ExprKind::Compare { first, rest } => {
                compile_time(first) || rest.iter().any(|(_, operand)| compile_time(operand))
            }
            ExprKind::Prefix(_, inner) => self.condition_is_compile_time_shaped(inner),
            _ => compile_time(cond),
        }
    }

    /// The element a compile-time collection display lists: a dictionary
    /// display's keys, a set display's elements, a list display's elements
    /// (a fixed-size array of literals here) at their default type. `None`
    /// for any other value.
    fn display_listed_element(&self, value: &Expr) -> Option<Ty> {
        match self.infer(value).ok()?.kind() {
            dict if let Some((key, _)) = mojito_types::types::dict_elements(dict) => {
                Some(key.clone())
            }
            set if let Some(element) = mojito_types::types::set_element(set) => {
                Some(element.clone())
            }
            TyKind::Struct(name, args)
                if (name == mojito_types::types::LIST_TYPE_NAME
                    || name == mojito_types::types::ARRAY_TYPE_NAME)
                    && let Some(TyArg::Ty(element)) = args.first() =>
            {
                Some(default_literal(element))
            }
            _ => None,
        }
    }

    /// Substitute the defining expression of every function-local
    /// compile-time value binding named in `expr`.
    fn inline_local_comptime_values(&self, expr: &Expr) -> Expr {
        if self.local_comptime_values.iter().all(HashMap::is_empty)
            && !self.installs_any(interface::Table::ComptimeValues)
        {
            return expr.clone();
        }
        let lookup = |name: &str| self.local_comptime_value(name);
        substitute_identifiers(expr, &lookup)
    }

    /// The element traits of the raw list a bound's value member is declared
    /// as (`comptime _types: _MLIR.KGENParamListType[Copyable]` read as
    /// `T._types`); `None` when `list` reads no such member.
    fn member_list_traits(&self, list: &ParamExpr) -> Option<Vec<String>> {
        let mojito_types::param_expr::ParamKind::TypeMember {
            subject,
            name,
            args,
        } = list.kind()
        else {
            return None;
        };
        let mojito_types::param_expr::ParamKind::TypeShape(subject) = subject.kind() else {
            return None;
        };
        if !matches!(subject.kind(), TyKind::Param { .. }) {
            return None;
        }
        let bounds = self.refined_bound(subject)?;
        let declared = args
            .is_empty()
            .then(|| self.lookup_trait_assoc_value_ty(&bounds, name))
            .flatten()?;
        let TyKind::ParamList(element) = declared.kind() else {
            return None;
        };
        match element.kind() {
            TyKind::Trait(traits) => Some(traits.clone()),
            _ => None,
        }
    }
}

/// Whether `constraint` names no parameter: every operand, the list a pack
/// query asks of included, is a value or a concrete type.
fn constraint_is_closed(constraint: &GenericConstraint) -> bool {
    constraint
        .try_map(&|operand| {
            let closed = match operand {
                ConstraintOperand::Param(_)
                | ConstraintOperand::PackLength(_)
                | ConstraintOperand::Expr(_) => false,
                ConstraintOperand::Value(value) => value_is_closed(value),
                ConstraintOperand::Type(ty) => !mojito_types::types::is_symbolic(ty),
            };
            closed.then(|| operand.clone()).ok_or(())
        })
        .is_ok()
}

/// Whether a constant names no parameter: no residual expression, and no
/// symbolic type, an element of a type list included.
fn value_is_closed(value: &mojito_types::ct::CtValue) -> bool {
    use mojito_types::ct::CtValue;
    match value {
        CtValue::Expr(_) => false,
        CtValue::Type(ty) => !mojito_types::types::is_symbolic(ty),
        CtValue::Tuple(elements) => elements.iter().all(value_is_closed),
        _ => true,
    }
}

/// A struct's arguments as its declarations bind them: a variadic struct
/// applied element by element binds its lone pack to the whole list (see
/// [`positional_pack_binding`]); any other argument list binds position by
/// position as it is.
pub(super) fn positional_pack_arguments<'a>(
    decls: &[ParamDecl],
    arguments: &'a [TyArg],
) -> std::borrow::Cow<'a, [TyArg]> {
    positional_pack_binding(decls, arguments)
        .map_or(std::borrow::Cow::Borrowed(arguments), |(_, pack)| {
            std::borrow::Cow::Owned(vec![pack])
        })
}

/// The pack binding of a variadic struct applied element by element
/// (`Tuple[Int, String]`, whose arguments are its element types): the pack's
/// name and the list of those types. A pack bound whole — a list value, or a
/// spread of another pack — is its own argument already.
pub(super) fn positional_pack_binding(
    decls: &[ParamDecl],
    arguments: &[TyArg],
) -> Option<(String, TyArg)> {
    let [
        ParamDecl::Type {
            name,
            variadic: true,
            ..
        },
    ] = decls
    else {
        return None;
    };
    if mojito_types::types::pack_spread_argument(arguments).is_some()
        || mojito_types::types::list_spread_argument(arguments).is_some()
    {
        return None;
    }
    arguments
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Some(CtValue::Type(ty.clone())),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|types| {
            (
                name.trim_start_matches('*').to_string(),
                TyArg::Val(CtValue::Tuple(types)),
            )
        })
}

/// The owner of a `comptime for`'s own binder, named by the loop's
/// iterable: each loop owns its binder.
pub(super) fn comptime_for_owner(iter: &Expr) -> String {
    let span = iter.source_span();
    format!(
        "{}{}:{}..{}",
        mojito_types::param_expr::COMPTIME_FOR_OWNER,
        super::overload_support::library_relative(span.source.as_deref().unwrap_or_default()),
        span.span.0,
        span.span.1
    )
}

/// The overload a compile-time call selects ([`Checker::called_application`]).
struct Selected<'a> {
    /// The name the call spells (`h`, `G.plus`).
    name: String,
    function: &'a ApplicableFunction,
    /// The struct instance a static method is called on (`G[1]` in
    /// `G[1].plus(n)`), or the type of a method's receiver.
    instance: Option<Ty>,
    /// The parameter expressions of its own compile-time arguments, then of
    /// a method's receiver, then of its runtime arguments.
    arguments: Vec<ParamExpr>,
    /// Its declared result under the parameters the call binds; `None` for
    /// one the call resolves where it stands.
    result: Option<Ty>,
}

/// An [`ApplicableFunction`]'s own compile-time parameters as its check
/// classifies them, and its result over them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AppliedSignature {
    decls: Vec<ParamDecl>,
    /// The result, unless it is `Self` or the instance an `__init__`
    /// constructs.
    result: Option<Ty>,
    /// A refined initializer's `out self` type
    /// ([`ApplicableFunction::refined_self`]).
    refined_self: Option<Ty>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ApplicableParameter {
    name: String,
    ty: SlotType,
    default: Option<Expr>,
}

/// What an [`ApplicableFunction`]'s `self` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum SelfSlot {
    /// It has none: a module `def` or a static method.
    None,
    /// An `__init__`'s `out self`, the synthesized fieldwise one's
    /// included: its result, the instance a call constructs.
    Constructed,
    /// A method's `self`, which a call binds to its receiver, after its
    /// own compile-time parameters and before its runtime ones, as its MIR
    /// declaration orders them.
    Receiver,
}

/// The type of an [`ApplicableParameter`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum SlotType {
    /// An `Int`, a `UInt`, a `Bool`, a `Float64`, an `index`, an `i1`, an
    /// `IntLiteral`, or a `StringLiteral`, a `String` parameter's included.
    Scalar(Ty),
    /// `Self`, the struct a call's instance or receiver resolves.
    Owner,
    /// Any other named type — a `DType`, a struct, a SIMD vector, its
    /// struct's own parameters, or one of the callable's own type
    /// parameters — as its signature classifies it
    /// ([`Checker::classify_applicable_signatures`]); `None` until it does,
    /// and one that resolves to no [`parameter_value_type`] takes no call.
    Declared(SourceType, Option<Ty>),
}

impl SlotType {
    /// The type, `Self` resolved to `owner`.
    const fn resolved<'a>(&'a self, owner: Option<&'a Ty>) -> Option<&'a Ty> {
        match self {
            Self::Scalar(ty) => Some(ty),
            Self::Owner => owner,
            Self::Declared(_, ty) => ty.as_ref(),
        }
    }
}

/// Whether `ty` is a value a [`SlotType::Declared`] parameter takes, the
/// types a parameter expression holds: a scalar, an `IntLiteral`, a
/// `StringLiteral`, a `DType`, a struct, or a SIMD vector. A reference, a
/// pointer, an origin, or a function is no such value.
fn parameter_value_type(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::INT
            | TyKind::UINT
            | TyKind::Bool
            | TyKind::FLOAT64
            | TyKind::Index
            | TyKind::I1
            | TyKind::IntLiteral
            | TyKind::StringLiteral(_)
            | TyKind::Dtype
            | TyKind::Struct(..)
            | TyKind::Simd { .. }
    )
}

/// The signature of a `def` or a static method, as its declaration spells
/// it.
struct ApplicableShape<'a> {
    decorators: &'a [mojito_ast::ast::Decorator],
    type_params: &'a [mojito_ast::ast::TypeParam],
    params: &'a [mojito_ast::ast::FnParam],
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    raises: bool,
    ret: &'a SourceType,
    where_clauses: &'a [Expr],
}

impl ApplicableShape<'_> {
    /// The callable under `symbol`, when a compile-time call can apply it,
    /// whose `self` is `self_slot`.
    fn applicable(
        &self,
        symbol: String,
        on_instance: bool,
        self_slot: SelfSlot,
    ) -> Option<ApplicableFunction> {
        let binders: Vec<String> = self
            .type_params
            .iter()
            .map(|binder| {
                (binder.callable_bound.is_none()
                    && binder.origin_mutability.is_none()
                    && !binder.name.starts_with('*')
                    && !binder.is_erased_origin_param(self.type_params))
                .then(|| binder.name.clone())
            })
            .collect::<Option<_>>()?;
        let slot = |ty: &SourceType| match ty {
            SourceType::Int => Some(SlotType::Scalar(Ty::int())),
            SourceType::UInt => Some(SlotType::Scalar(Ty::uint())),
            SourceType::Bool => Some(SlotType::Scalar(Ty::bool())),
            SourceType::Float64 => Some(SlotType::Scalar(Ty::float64())),
            SourceType::SelfType => Some(SlotType::Owner),
            // A string is the parameter domain's string, as a `[s:
            // String]` binder is.
            SourceType::StringLiteral => Some(SlotType::Scalar(Ty::string_literal())),
            SourceType::Named(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name) =>
            {
                Some(SlotType::Scalar(Ty::string_literal()))
            }
            ty if is_mlir_index_source(ty) => Some(SlotType::Scalar(Ty::index())),
            ty if is_mlir_source(ty, "i1") => Some(SlotType::Scalar(Ty::i1())),
            ty if super::declarations::is_int_literal_annotation(ty) => {
                Some(SlotType::Scalar(Ty::int_literal()))
            }
            SourceType::Named(..) | SourceType::SelfParam(_) | SourceType::Assoc { .. } => {
                Some(SlotType::Declared(ty.clone(), None))
            }
            _ => None,
        };
        let params = self
            .params
            .iter()
            .map(|param| {
                let passed_by_value = param.kind == mojito_ast::ast::ParamKind::Regular
                    && matches!(
                        param.convention,
                        None | Some(ArgConvention::Imm | ArgConvention::Var)
                    );
                Some(ApplicableParameter {
                    name: param.name.clone(),
                    ty: slot(&param.ty).filter(|_| passed_by_value)?,
                    default: param.default.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(ApplicableFunction {
            symbol,
            on_instance,
            self_slot,
            binders,
            signature: None,
            params,
            positional_only: self.positional_only,
            keyword_only: self.keyword_only,
            result: match self.ret {
                SourceType::Int
                | SourceType::UInt
                | SourceType::Bool
                | SourceType::Float64
                | SourceType::SelfType
                | SourceType::Named(..) => self.ret.clone(),
                ret if is_mlir_index_source(ret)
                    || is_mlir_source(ret, "!kgen.deferred")
                    || is_mlir_source(ret, "!kgen.string")
                    || is_mlir_source(ret, "!kgen.target") =>
                {
                    ret.clone()
                }
                _ => return None,
            },
            raises: self.raises,
            refined_self: None,
            implicit: self
                .decorators
                .iter()
                .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit"),
            inline: mojito_checked::declaration::InlineLevel::of(self.decorators)
                .unwrap_or_default(),
            folded_body: None,
            where_clauses: self.where_clauses.to_vec(),
            constraints: self.where_clauses.is_empty().then(Vec::new),
        })
    }
}

/// `folded`, a builtin body's value, as the value of the call
/// `application`: a struct value it builds is spelled as the call, which
/// the pin displays and the elaborator answers (`L(n) + L(1)` for the
/// `Self(...)` its `__add__` returns).
fn respelled_struct_value(
    context: &mojito_types::param_expr::ParamContext,
    folded: ParamExpr,
    application: &ParamExpr,
) -> ParamExpr {
    match folded.kind() {
        ParamKind::StructValue { fields, .. } => context.struct_value(fields, application),
        _ => folded,
    }
}

/// Whether a folded builtin method body still reads a struct parameter
/// its instance binds to anything but the parameter itself, by a reference
/// or by a pack query the replacement did not resolve: such a fold is no
/// value of the call, which stays its application.
fn reads_rebound_binder(folded: &ParamExpr, decls: &[ParamDecl], arguments: &[TyArg]) -> bool {
    let rebound: HashSet<&ParamId> = decls
        .iter()
        .zip(arguments)
        .filter(|(decl, argument)| match argument {
            TyArg::Val(CtValue::Expr(value)) => {
                !matches!(value.kind(), ParamKind::DeclRef(reference) if reference.id == *decl.id())
            }
            TyArg::Ty(ty) => {
                !matches!(ty.kind(), TyKind::Param { binder, .. } if binder.id == *decl.id())
            }
            TyArg::Val(_) | TyArg::Origin(_) => true,
        })
        .map(|(decl, _)| decl.id())
        .collect();
    let mut reads = false;
    folded.visit(&mut |node| {
        reads |= matches!(
            node.kind(),
            ParamKind::DeclRef(reference) | ParamKind::PackQuery { pack: reference, .. }
                if rebound.contains(&reference.id)
        );
    });
    reads
}

/// The name a struct was declared under, whichever instance clone `name`
/// spells.
fn template_name(name: &str) -> &str {
    mojito_symbol::symbol::specialization_template(name).unwrap_or(name)
}

/// The spelling of a compile-time collection's type in the rejection of its
/// runtime read, as the pin spells it: a list display is a fixed-size
/// `Array[Int, Int(2)]`, a set `Set[Int]`, a dictionary `Dict[String, Int]`.
fn materialized_collection_spelling(ty: &Ty) -> String {
    fn scalar(element: &Ty) -> String {
        match element.kind() {
            TyKind::INT | TyKind::IntLiteral => "Int".to_string(),
            TyKind::FLOAT64 | TyKind::FloatLiteral => "Float64".to_string(),
            TyKind::StringLiteral(_) => "String".to_string(),
            _ => match mojito_types::types::tuple_elements(element) {
                Some(elements) => format!(
                    "Tuple[{}]",
                    elements
                        .into_iter()
                        .map(scalar)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => mojito_types::types::unqualified_type_name(element),
            },
        }
    }
    if let Some((key, value)) = mojito_types::types::dict_elements(ty) {
        return format!("Dict[{}, {}]", scalar(key), scalar(value));
    }
    if let Some(element) = mojito_types::types::list_element(ty) {
        return format!("List[{}]", scalar(element));
    }
    if let Some(element) = mojito_types::types::set_element(ty) {
        return format!("Set[{}]", scalar(element));
    }
    if let TyKind::Struct(name, args) = ty.kind()
        && name == mojito_types::types::ARRAY_TYPE_NAME
    {
        let mut arguments = args.iter();
        if let (Some(TyArg::Ty(element)), Some(TyArg::Val(length))) =
            (arguments.next(), arguments.next())
        {
            return format!("Array[{}, Int({length})]", scalar(element));
        }
    }
    mojito_types::types::unqualified_type_name(ty)
}

/// Clone `expr` with every identifier that `lookup` binds replaced by its
/// binding, recursing through the expression forms a compile-time condition
/// or iterable can take.
fn substitute_identifiers<'a>(expr: &Expr, lookup: &dyn Fn(&str) -> Option<&'a Expr>) -> Expr {
    let sub = |inner: &Expr| substitute_identifiers(inner, lookup);
    let sub_box = |inner: &Expr| Box::new(sub(inner));
    let kind = match &expr.kind {
        ExprKind::Identifier(name) => match lookup(name) {
            Some(bound) => return bound.clone(),
            None => return expr.clone(),
        },
        ExprKind::Prefix(op, inner) => ExprKind::Prefix(*op, sub_box(inner)),
        ExprKind::Infix(op, left, right) => ExprKind::Infix(*op, sub_box(left), sub_box(right)),
        ExprKind::Compare { first, rest } => ExprKind::Compare {
            first: sub_box(first),
            rest: rest
                .iter()
                .map(|(op, operand)| (*op, sub(operand)))
                .collect(),
        },
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => ExprKind::Call {
            name: name.clone(),
            param_args: param_args.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => ExprKind::Invoke {
            callee: sub_box(callee),
            param_args: param_args.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Member { object, field } => ExprKind::Member {
            object: sub_box(object),
            field: field.clone(),
        },
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => ExprKind::MethodCall {
            object: sub_box(object),
            method: method.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Index { object, index } => ExprKind::Index {
            object: sub_box(object),
            index: sub_box(index),
        },
        ExprKind::TupleLit(elements) => ExprKind::TupleLit(elements.iter().map(sub).collect()),
        ExprKind::ListLit(elements) => ExprKind::ListLit(elements.iter().map(sub).collect()),
        _ => return expr.clone(),
    };
    // The rebuilt node is the occurrence it was rebuilt from, so a fact
    // recorded on it is that occurrence's.
    let mut rewritten = Expr::new(kind, expr.span);
    rewritten.source.clone_from(&expr.source);
    rewritten.syntax_id = expr.syntax_id;
    rewritten
}

/// The binder of a bounded pack-element view ([`Checker::opaque_element`]):
/// the view's qualified spelling is its whole identity, so
/// [`Checker::restore_pack_elements`] finds it again.
fn pack_element_view_binder(name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&format!("$view:{name}"), 0),
        name: name.into(),
    }
}

/// Whether `iter` is a collection display of scalar `element`s a loop binder
/// takes (`Int`, `Bool`, `Float64`, `String`), which the elaborator evaluates per
/// instance when the check does not close it.
fn evaluated_display(iter: &Expr, element: &Ty) -> bool {
    let display = match &iter.kind {
        ExprKind::ListLit(items) => !items.is_empty(),
        ExprKind::BraceLit(entries) => !entries.is_empty(),
        _ => false,
    };
    display
        && (matches!(element.kind(), TyKind::INT | TyKind::Bool | TyKind::FLOAT64)
            || matches!(element.kind(), TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name)))
}

/// The element a display of `element`s binds: a string element binds as a
/// `String` value parameter does.
pub(super) fn string_element_binder(element: Ty) -> Ty {
    match element.kind() {
        TyKind::Struct(name, args)
            if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name) =>
        {
            Ty::string_literal()
        }
        _ => element,
    }
}

/// Whether a type `meta` holds names the declared parameter `binder` (a
/// `*vs: T` pack's element type naming `T`).
fn meta_names_binder(
    meta: &mojito_types::param_expr::MetaTy,
    binder: &mojito_types::param_expr::ParamRef,
) -> bool {
    use mojito_types::param_expr::MetaTy;
    match meta {
        MetaTy::Value(ty) => mojito_types::types::names_binder(ty, binder),
        MetaTy::Type => false,
        MetaTy::ParamList(element) => meta_names_binder(element, binder),
        MetaTy::Tuple(elements) | MetaTy::List(elements) | MetaTy::Set(elements) => elements
            .iter()
            .any(|element| meta_names_binder(element, binder)),
        MetaTy::Dict(entries) => entries
            .iter()
            .any(|(key, value)| meta_names_binder(key, binder) || meta_names_binder(value, binder)),
    }
}

/// Record `value` for `name` unless a different value is already recorded,
/// in which case the disagreement leaves `name` unrecorded.
fn record_agreed<V: PartialEq>(
    recorded: &mut declaration_table::DeclarationTable<V>,
    name: &str,
    value: V,
) {
    match recorded.get(name) {
        Some(known) if *known != value => {
            recorded.remove(name);
        }
        Some(_) => {}
        None => {
            recorded.insert(name.to_string(), value);
        }
    }
}

/// Every name `expr` reads, in source order.
pub(super) fn names_read(expr: &Expr) -> Vec<String> {
    struct Names(Vec<String>);
    impl mojito_ast::visit::Visitor for Names {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::Identifier(name) = &expr.kind {
                self.0.push(name.clone());
            }
        }
    }
    let mut names = Names(Vec::new());
    mojito_ast::visit::walk_expr(&mut names, expr);
    names.0
}

/// Whether `expr` reads `__is_run_in_comptime_interpreter`, a run-time
/// value: a compile-time position reading it is the pin's "dynamic value"
/// rejection.
pub(super) fn reads_comptime_interpreter_query(expr: &Expr) -> bool {
    struct Query(bool);
    impl mojito_ast::visit::Visitor for Query {
        fn visit_expr(&mut self, expr: &Expr) {
            self.0 |= matches!(expr.kind, ExprKind::IsRunInComptimeInterpreter);
        }
    }
    let mut query = Query(false);
    mojito_ast::visit::walk_expr(&mut query, expr);
    query.0
}

/// Whether `ty` spells the MLIR builtin `__mlir_type.index`.
fn is_mlir_index_source(ty: &SourceType) -> bool {
    is_mlir_source(ty, "index")
}

/// Whether `ty` spells `` __mlir_type.`name` ``.
fn is_mlir_source(ty: &SourceType, mlir_name: &str) -> bool {
    mlir_source_name(ty) == Some(mlir_name)
}

/// Whether `value` is a scalar or a string attribute.
const fn is_scalar_attribute(value: &CtValue) -> bool {
    matches!(
        value,
        CtValue::Int(_)
            | CtValue::UInt(_)
            | CtValue::Index(_)
            | CtValue::Float(_)
            | CtValue::IntLiteral(_)
            | CtValue::FloatLiteral(_)
            | CtValue::Bool(_)
            | CtValue::Str(_)
    )
}

/// The predicate name a bracket application spells (`IsTriviallyCopyable`
/// in `IsTriviallyCopyable[T]`, which may parse as an index).
fn predicate_spelling(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::TypeApply { name, .. } => Some(name),
        ExprKind::Index { object, .. } => match &object.kind {
            ExprKind::Identifier(name) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

/// Whether two bound lists name the same traits, in any order.
fn same_bounds(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|bound| right.contains(bound))
}

/// The names `expr` reads as values: every name it spells but one under a
/// `type_of` operand, which reads only its operand's type.
fn value_names_read(expr: &Expr) -> Vec<String> {
    struct Names {
        values: Vec<(*const Expr, String)>,
        typed: HashSet<*const Expr>,
    }
    impl Names {
        fn type_operand(&mut self, operand: &Expr) {
            struct Identifiers<'n>(&'n mut HashSet<*const Expr>);
            impl mojito_ast::visit::Visitor for Identifiers<'_> {
                fn visit_expr(&mut self, expr: &Expr) {
                    if matches!(expr.kind, ExprKind::Identifier(_)) {
                        self.0.insert(std::ptr::from_ref(expr));
                    }
                }
            }
            mojito_ast::visit::walk_expr(&mut Identifiers(&mut self.typed), operand);
        }
    }
    impl mojito_ast::visit::Visitor for Names {
        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Identifier(name) => {
                    self.values.push((std::ptr::from_ref(expr), name.clone()));
                }
                ExprKind::Call { name, args, .. } if name == "type_of" => {
                    for operand in args {
                        self.type_operand(operand);
                    }
                }
                _ => {}
            }
        }
        fn visit_type(&mut self, ty: &SourceType) {
            if let SourceType::TypeOf(operand) = ty {
                self.type_operand(operand);
            }
        }
    }
    let mut names = Names {
        values: Vec::new(),
        typed: HashSet::new(),
    };
    mojito_ast::visit::walk_expr(&mut names, expr);
    names
        .values
        .into_iter()
        .filter(|(identifier, _)| !names.typed.contains(identifier))
        .map(|(_, name)| name)
        .collect()
}

const VARIADICS_MODULE: &str = "std.builtin.variadics";

/// The constant `` `body : ty` `` spells and its type, for the typed
/// attributes the subset reads ([`mlir_attribute`]).
fn typed_mlir_attribute(body: &str, ty: &str) -> Option<(CtValue, Ty)> {
    if ty == "index" {
        return Some((CtValue::Index(body.parse().ok()?), Ty::index()));
    }
    if ty == "!kgen.target" {
        let target = body
            .strip_prefix("#kgen.target<")?
            .strip_suffix('>')
            .and_then(mojito_types::ct::KgenTarget::parse)?;
        return Some((
            CtValue::KgenTarget(Box::new(target)),
            TyKind::KgenTarget.into(),
        ));
    }
    let storage = mojito_types::types::mlir_type_named(ty).or_else(|| {
        let (width, dtype) = ty
            .strip_prefix("!kgen.simd<")?
            .strip_suffix('>')?
            .split_once(',')?;
        let dtype = mojito_ast::ast::Dtype::from_kgen_mnemonic(dtype.trim())?;
        Some(mojito_types::types::kgen_simd_ty(
            dtype,
            width.trim().parse().ok()?,
        ))
    })?;
    let (SimdDtype::Known(dtype), SimdWidth::Known(width)) =
        mojito_types::types::kgen_simd_slots(storage.kind())?
    else {
        return None;
    };
    let lanes = body
        .strip_prefix("#kgen.simd<")?
        .strip_suffix('>')?
        .split(',')
        .map(|lane| mojito_types::ct::CtLane::parse_attribute(lane, dtype))
        .collect::<Option<Vec<_>>>()?;
    (i64::try_from(lanes.len()).ok() == Some(width))
        .then_some((CtValue::Simd { dtype, lanes }, storage))
}
