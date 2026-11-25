//! Resolution of source type annotations into checked `Ty` values:
//! type-argument-bearing builtins (List/Set/Dict/Pointer/Tuple/SIMD/Variant),
//! dependent/associated type projection, and type-parameter lookup.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::FunctionTypeExpr;
use mojito_checked::checked::BracketBound as BoundAttribute;
use mojito_types::origin::CallableEnvironment;
use mojito_types::types::{FuncTy, GenericFuncTy, TransferSet};
use mojito_types::types::{TYPE_OF_ALL_TYPES, TyKind, is_simd_length};

/// Current Mojo treats a bare `def(...)` in a general type position (struct
/// field, collection element) as a trait, not a storable callable-value type;
/// callable values are limited to parameters and local bindings.
pub(super) fn reject_stored_callable_type(ty: &Ty, position: &str) -> Result<(), TypeError> {
    if matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) {
        return Err(TypeError::Unsupported(format!(
            "a 'def(...)' type names a trait in {position} in current Mojo, not a storable \
             callable value; callable values are limited to parameters and 'def(...) thin' \
             local bindings"
        )));
    }
    Ok(())
}

/// Reject invoking a value whose type is a non-`thin` `def(...)`: that
/// spelling names a trait, so the value has upstream's
/// `AnyTrait[def(...)]` type, which its `__call__` requirement's `Self`
/// cannot take.
pub(super) fn reject_trait_callable_call(ty: &Ty) -> Result<(), TypeError> {
    match ty.as_func() {
        Some(func) if func.environment == CallableEnvironment::Default => Err(TypeError::BadCall {
            func: "__call__".to_string(),
            reason: format!(
                "parameter '_Self' has '{ty}' type, but value has type 'AnyTrait[{ty}]'"
            ),
        }),
        _ => Ok(()),
    }
}

/// What each bracket argument of one application bound to, in source
/// order: `Some` for an argument with no run-time form (a type, an origin,
/// or a compile-time value), as upstream's op carries each argument as the
/// parameter attribute its binding produced.
pub(super) type BracketAttributes = Vec<Option<TyArg>>;

/// What [`Checker::resolve_use_params`] bound a use site's parameters to:
/// the type substitution, the full argument list in declaration order (the
/// identity), and the attributes its bracket arguments bound to, keyed by
/// declaration: a struct application's over the struct's whole declared
/// list, each origin argument in its declared slot.
pub(super) struct BoundParameters {
    pub(super) subst: TySubst,
    pub(super) arguments: Vec<TyArg>,
    pub(super) attributes: DeclaredAttributes,
    /// The origin arguments the bracket bound, each in its binder's
    /// declared slot, which the use substitutes into its signature.
    pub(super) origins: Vec<BoundOrigin>,
}

/// The origin and `mut=` arguments one bracket binds over its callee's
/// whole declared list ([`Checker::bound_bracket_origins`]).
#[derive(Default)]
pub(super) struct BracketOrigins {
    pub(super) origins: Vec<BoundOrigin>,
    pub(super) mutabilities: Vec<BoundMutability>,
}

/// What a bracket applies: a callee, whose origin arguments bind its
/// signature, or a struct type at the given strictness, whose origin
/// arguments are part of the type's identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BracketUse {
    Call,
    Type(super::StorageStrictness),
}

/// One `mut=` argument a bracket binds: the declared slot of an origin's
/// mutability binder, its identity, the argument's source position, and
/// the `Bool` it binds ([`CtValue::Deferred`] where it leaves the binder to
/// its origin).
#[derive(Clone)]
pub(super) struct BoundMutability {
    pub(super) slot: usize,
    pub(super) id: mojito_types::param_expr::ParamId,
    pub(super) position: usize,
    pub(super) value: CtValue,
}

/// The explicit arguments of a call's bracket, resolved once before its
/// run-time arguments are typed: the substitution an argument display
/// solves under, the bracket's origin arguments bound in their declared
/// slots, and each resolved type argument, which the call's binder
/// ([`Checker::resolve_explicit_use_params`]) reads instead of resolving
/// it again, as upstream binds each bracket argument once.
#[derive(Default)]
pub(super) struct ExplicitTypes {
    pub(super) subst: TySubst,
    /// Each resolved argument with its declared slot and its source
    /// position in the bracket.
    resolved: Vec<(usize, usize, BoundArgument)>,
    /// The origin arguments, each bound in its binder's declared slot.
    origins: Vec<BoundOrigin>,
    /// The `mut=` arguments, each bound in its binder's declared slot.
    mutabilities: Vec<BoundMutability>,
}

impl ExplicitTypes {
    /// Record `argument`, the bracket argument at `position`, bound to the
    /// parameter declared at `index`.
    pub(super) fn insert(&mut self, index: usize, position: usize, argument: BoundArgument) {
        self.resolved.push((index, position, argument));
    }

    /// The argument at `position` resolved for the parameter declared at
    /// `index`.
    pub(super) fn resolved(&self, index: usize, position: usize) -> Option<BoundArgument> {
        self.resolved
            .iter()
            .find(|(slot, at, _)| (*slot, *at) == (index, position))
            .map(|(_, _, argument)| argument.clone())
    }

    /// The origin argument at `position` bound to the origin binder
    /// declared at `slot`.
    pub(super) fn origin(&self, slot: usize, position: usize) -> Option<&BoundOrigin> {
        self.origins
            .iter()
            .find(|bound| (bound.slot, bound.position) == (slot, position))
    }

    /// The origin arguments the bracket bound.
    pub(super) fn origins(&self) -> &[BoundOrigin] {
        &self.origins
    }

    /// The `mut=` argument at `position` bound to the mutability binder
    /// declared at `slot`.
    pub(super) fn mutability(&self, slot: usize, position: usize) -> Option<&BoundMutability> {
        self.mutabilities
            .iter()
            .find(|bound| (bound.slot, bound.position) == (slot, position))
    }

    /// The `mut=` arguments the bracket bound.
    pub(super) fn mutabilities(&self) -> &[BoundMutability] {
        &self.mutabilities
    }

    /// Record the origin and `mut=` arguments the bracket bound.
    pub(super) fn set_origins(&mut self, bound: BracketOrigins) {
        self.origins = bound.origins;
        self.mutabilities = bound.mutabilities;
    }
}

/// What [`Checker::resolve_param_arg`] bound one bracket argument to: its
/// value, and whether binding made a value-shaped argument an attribute
/// beyond what [`bracket_attribute`] reads off the value (a converted
/// constructor application, a `SIMDLength`'s lane count, a generator, a
/// trait, a display's literal application, a closed raw value list).
#[derive(Clone)]
pub(super) struct BoundArgument {
    pub(super) value: TyArg,
    attribute: bool,
}

impl BoundArgument {
    /// The attribute `argument` carries, bound as `self`.
    pub(super) fn bracket_attribute(&self, argument: &mojito_ast::ast::ParamArg) -> Option<TyArg> {
        bracket_attribute(argument, &self.value)
            .or_else(|| self.attribute.then(|| self.value.clone()))
    }

    const fn attribute(value: TyArg) -> Self {
        Self {
            value,
            attribute: true,
        }
    }
}

impl From<TyArg> for BoundArgument {
    fn from(value: TyArg) -> Self {
        Self {
            value,
            attribute: false,
        }
    }
}

/// The value parameters a use binds, for [`Checker::resolve_dependent_ty`]:
/// replacement bindings, plus each bound list as it was bound, which a pack
/// element selects from whether or not its types are closed. A callee's own
/// arguments ([`Self::own`]) bind its declarations by identity, as upstream
/// substitutes a signature's own parameter declarations, so a caller binder
/// of the same spelling an argument mentions is never rebound.
#[derive(Debug, Default)]
pub(super) struct DependentValues {
    bindings: mojito_types::param_expr::ParamBindings,
    lists_by_id: HashMap<mojito_types::param_expr::ParamId, Vec<CtValue>>,
    lists_by_name: HashMap<String, Vec<CtValue>>,
}

impl DependentValues {
    /// `decls`' value arguments, bound by their declarations' identities.
    pub(super) fn own(
        context: &mojito_types::param_expr::ParamContext,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Self {
        let mut values = Self::default();
        for (decl, argument) in ParamDecl::aligned_pairs(decls, arguments) {
            let TyArg::Val(value) = argument else {
                continue;
            };
            if decl.is_type_pack() {
                continue;
            }
            if let CtValue::Tuple(elements) = value {
                values
                    .lists_by_id
                    .insert(decl.id().clone(), elements.clone());
            }
            if let Ok(bound) = context.constant(value.clone()) {
                values.bindings.bind(decl.id().clone(), bound);
            }
        }
        values
    }

    /// A name → value map's bindings, each reference bound by its spelling.
    pub(super) fn named(
        context: &mojito_types::param_expr::ParamContext,
        values: &HashMap<String, CtValue>,
    ) -> Self {
        Self {
            bindings: mojito_types::param_expr::ParamBindings::from_named_values(context, values),
            lists_by_id: HashMap::new(),
            lists_by_name: values
                .iter()
                .filter_map(|(name, value)| match value {
                    CtValue::Tuple(elements) => {
                        Some((name.trim_start_matches('*').to_string(), elements.clone()))
                    }
                    _ => None,
                })
                .collect(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.lists_by_id.is_empty() && self.lists_by_name.is_empty()
    }

    fn list(&self, reference: &mojito_types::param_expr::ParamRef) -> Option<&[CtValue]> {
        self.lists_by_id
            .get(&reference.id)
            .or_else(|| {
                self.lists_by_name
                    .get(reference.name.trim_start_matches('*'))
            })
            .map(Vec::as_slice)
    }
}

/// Whether a bound argument is still symbolic: an expression over open
/// parameters, or an aggregate holding one.
pub(super) fn tyarg_is_symbolic(argument: &TyArg) -> bool {
    match argument {
        TyArg::Val(CtValue::Expr(_) | CtValue::Deferred(_)) => true,
        TyArg::Val(CtValue::Tuple(values) | CtValue::List(values)) => values
            .iter()
            .any(|value| matches!(value, CtValue::Expr(_) | CtValue::Deferred(_))),
        TyArg::Val(_) => false,
        TyArg::Origin(origin) => matches!(
            origin.projection_base(),
            mojito_types::origin::Origin::Param(_) | mojito_types::origin::Origin::SelfParam
        ),
        TyArg::Ty(ty) => type_is_symbolic(ty),
    }
}

/// The attribute bracket argument `argument` carries once binding resolved
/// it to `bound`: a pack's spread always has one, and any other argument
/// only when it bound to a type or an origin, a run-time value having none.
pub(super) fn bracket_attribute(
    argument: &mojito_ast::ast::ParamArg,
    bound: &TyArg,
) -> Option<TyArg> {
    use mojito_ast::ast::ParamArg;
    match argument {
        ParamArg::Named { value, .. } => bracket_attribute(value, bound),
        argument
            if argument
                .spread_operand()
                .and_then(Expr::spread_pack_name)
                .is_some() =>
        {
            Some(bound.clone())
        }
        // A trait, a type list, or a KGEN string has no run-time form, so
        // such an argument is an attribute like a type. So is a compile-time
        // string an application computes, which is the elaborator's frozen
        // result rather than the argument expression's own value.
        ParamArg::Value(_) => matches!(bound, TyArg::Ty(_) | TyArg::Origin(_))
            .then(|| bound.clone())
            .or_else(|| match bound {
                TyArg::Val(value)
                    if value.is_compile_time_only() || computed_string_literal(value) =>
                {
                    Some(bound.clone())
                }
                _ => None,
            }),
    }
}

/// The attributes of bracket arguments `args` that each bound to the type
/// beside it in `types`.
pub(super) fn types_attributes(
    args: &[mojito_ast::ast::ParamArg],
    types: &[Ty],
) -> BracketAttributes {
    args.iter()
        .zip(types)
        .map(|(argument, ty)| bracket_attribute(argument, &TyArg::Ty(ty.clone())))
        .collect()
}

/// The attribute of a dtype bracket argument that bound to `dtype`.
pub(super) fn dtype_attribute(
    argument: &mojito_ast::ast::ParamArg,
    dtype: &SimdDtype,
) -> Option<TyArg> {
    bracket_attribute(
        argument,
        &TyArg::Val(match dtype {
            SimdDtype::Known(dtype) => mojito_types::types::dtype_value(*dtype),
            SimdDtype::Expr(expr) => CtValue::Expr(expr.clone()),
        }),
    )
}

/// What a declared-parameter binder bound each bracket argument to, keyed
/// by declaration as upstream's binder fills the op's parameter list: per
/// declared parameter, in declaration order, the attributes of the bracket
/// arguments bound to it (none for an omitted or inferred one, one per
/// element for a pack), each at its source position, which the checked
/// call's node-building reads to build its declared parameters
/// ([`mojito_checked::checked::CheckedParameter`]).
#[derive(Clone, Debug, Default)]
pub(super) struct DeclaredAttributes {
    slots: Vec<Vec<BoundAttribute>>,
    /// The name of each slot's declared parameter, `None` for a slot the
    /// binder keyed by no declaration.
    declarations: Vec<Option<String>>,
}

impl DeclaredAttributes {
    /// No attribute yet for any of the declared parameters `decls`.
    pub(super) fn new<P: mojito_ast::call::CompileTimeParam>(decls: &[P]) -> Self {
        Self {
            slots: vec![Vec::new(); decls.len()],
            declarations: decls
                .iter()
                .map(|decl| Some(decl.keyword().to_string()))
                .collect(),
        }
    }

    /// Bind the argument at source `position` of the parameter declared at
    /// `index` to `attribute`, replacing what it was bound to.
    pub(super) fn bind(&mut self, index: usize, position: usize, attribute: Option<TyArg>) {
        if self.slots.len() <= index {
            self.slots.resize(index + 1, Vec::new());
        }
        let slot = &mut self.slots[index];
        match slot.iter_mut().find(|bound| bound.position == position) {
            Some(bound) => bound.attribute = attribute,
            None => slot.push(BoundAttribute {
                position,
                attribute,
            }),
        }
    }

    /// Name the parameter declared at `index` `declaration`.
    pub(super) fn declare(&mut self, index: usize, declaration: &str) {
        if self.declarations.len() <= index {
            self.declarations.resize(index + 1, None);
        }
        self.declarations[index] = Some(declaration.to_string());
    }

    /// Set each attribute `folded` carries, keeping every one it leaves
    /// `None`.
    pub(super) fn merge(&mut self, folded: Self) {
        for (index, slot) in folded.slots.into_iter().enumerate() {
            for bound in slot {
                if bound.attribute.is_some() {
                    self.bind(index, bound.position, bound.attribute);
                }
            }
        }
        for (index, declaration) in folded.declarations.into_iter().enumerate() {
            if let Some(declaration) = declaration
                && self.declarations.get(index).is_none_or(Option::is_none)
            {
                self.declare(index, &declaration);
            }
        }
    }

    /// The binding the checked node records
    /// ([`mojito_checked::checked::NodeFacts::parameter_attributes`]).
    pub(super) fn binding(&self) -> mojito_checked::checked::BracketBinding {
        mojito_checked::checked::BracketBinding {
            slots: self.slots.clone(),
            declarations: self.declarations.clone(),
        }
    }

    /// The attributes laid out by source position.
    pub(super) fn source_order(&self) -> BracketAttributes {
        self.binding().source_order()
    }

    /// The source-ordered `attributes` a compiler builtin's binder bound
    /// `args` to, each keyed by the declaration of `params` its argument
    /// binds by [`mojito_ast::call::match_parameter_slots`]' rule.
    pub(super) fn of_builtin(
        name: &str,
        params: &[BuiltinParam],
        args: &[mojito_ast::ast::ParamArg],
        attributes: BracketAttributes,
    ) -> Result<Self, TypeError> {
        let keywords = mojito_ast::call::bracket_keywords(args);
        let slots = mojito_ast::call::match_parameter_slots(params, &keywords)
            .map_err(|error| error.into_type_error(name))?;
        let mut declared = Self::new(params);
        for (position, (index, attribute)) in slots.into_iter().zip(attributes).enumerate() {
            declared.bind(index, position, attribute);
        }
        Ok(declared)
    }
}

/// One compile-time parameter of a compiler builtin, as upstream's library
/// declares it (`List[T: AnyType, /]`, `external_call[callee, return_type,
/// *types, num_fixed_args=]`): the declaration the builtin's binder keys
/// each bracket argument's attribute by ([`DeclaredAttributes::of_builtin`]).
#[derive(Clone, Copy)]
pub(super) struct BuiltinParam {
    /// The declared name, a pack's with its `*`.
    name: &'static str,
    /// Whether the parameter is declared before `//`.
    infer_only: bool,
    /// Whether the parameter is declared after a bare `*`.
    keyword_only: bool,
}

impl BuiltinParam {
    /// A parameter declared after any `//`.
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            infer_only: false,
            keyword_only: false,
        }
    }

    /// A parameter declared before `//`, bound by keyword or inferred.
    const fn infer_only(name: &'static str) -> Self {
        Self {
            name,
            infer_only: true,
            keyword_only: false,
        }
    }

    /// A parameter declared after a bare `*`, bound by keyword alone.
    const fn keyword_only(name: &'static str) -> Self {
        Self {
            name,
            infer_only: false,
            keyword_only: true,
        }
    }
}

impl mojito_ast::call::CompileTimeParam for BuiltinParam {
    fn keyword(&self) -> &str {
        self.name.trim_start_matches('*')
    }

    fn infer_only(&self) -> bool {
        self.infer_only
    }

    fn variadic(&self) -> bool {
        self.name.starts_with('*')
    }

    fn keyword_only(&self) -> bool {
        self.keyword_only
    }
}

/// A `Pointer[...]` bracket bound over upstream's declared parameter list
/// ([`Checker::pointer_bracket`]): each slot the bracket spells, the origin
/// already passed at its `mut=` argument's type.
#[derive(Default)]
pub(super) struct PointerBracket {
    /// A `mut=` literal; `None` when absent or spelled by a value.
    pub(super) mutability: Option<bool>,
    pub(super) element: Option<Ty>,
    /// The `origin` argument with the `mut` it is held at, canonical
    /// ([`mojito_types::origin::Origin::canonical_pointer_parts`]).
    pub(super) origin: Option<(
        mojito_types::origin::Origin,
        mojito_types::origin::Mutability,
    )>,
    pub(super) address_space: Option<mojito_types::types::AddressSpaceSlot>,
    /// The attribute each source argument carries.
    pub(super) attributes: DeclaredAttributes,
}

/// A one-type list: `reflect[T: AnyType]`, `Variant.isa[T: AnyType]`, a
/// one-type-argument trait test (`IsTriviallyMovable[T: AnyType]`), and
/// the compiler-private `__UninitStorage[T]`.
pub(super) const T_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("T")];
/// A one-type list spelled `type`: `_unqualified_type_name[type:
/// AnyType]`, `TypeList.contains[type: AnyType]`.
pub(super) const TYPE_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("type")];
/// `Variant.replace[Tin: Movable & Deinitable, Tout: Movable]` and
/// `unsafe_replace`.
pub(super) const VARIANT_REPLACE_PARAMS: &[BuiltinParam] =
    &[BuiltinParam::new("Tin"), BuiltinParam::new("Tout")];
/// `external_call[callee: StaticString, return_type: RegisterPassable,
/// *types: AnyType, num_fixed_args: Optional[Int] = None]`.
pub(super) const EXTERNAL_CALL_PARAMS: &[BuiltinParam] = &[
    BuiltinParam::new("callee"),
    BuiltinParam::new("return_type"),
    BuiltinParam::new("*types"),
    BuiltinParam::new("num_fixed_args"),
];
/// A value pack: the compiler-private `__param_list_address[*values]`.
pub(super) const VALUE_PACK_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("*values")];

/// One origin argument a bracket binds: its origin binder's declared slot
/// and the [`Origin::Param`](mojito_types::origin::Origin::Param) id a
/// signature names that binder by, the argument's source position and
/// attribute, and the origin it binds.
#[derive(Clone)]
pub(super) struct BoundOrigin {
    pub(super) slot: usize,
    pub(super) id: mojito_types::origin::OriginParamId,
    pub(super) position: usize,
    pub(super) attribute: Option<TyArg>,
    pub(super) origin: mojito_types::origin::Origin,
    /// The argument's capability when known (`None` for a symbolic one):
    /// an `ImmOrigin(o)` cast binds the slot immutably whatever `o` allows.
    pub(super) mutability: Option<mojito_types::origin::Mutability>,
}

/// A struct application's bracket bound over the struct's whole declared
/// list ([`Checker::struct_bracket`]): the explicit arguments its binder
/// reads back, and the origin and `mut=` arguments the application fixes.
pub(super) struct StructBracket {
    pub(super) explicit: ExplicitTypes,
    /// One origin per origin binder, in declared order, `Origin::Unbound`
    /// where the application leaves it to inference.
    pub(super) origins: Vec<mojito_types::origin::Origin>,
    /// The origin arguments that bind an origin, which a construction's
    /// arguments are checked against.
    pub(super) bound: Vec<BoundOrigin>,
    /// The value each `mut=` argument gives its binder, by identity.
    pub(super) mutabilities: Vec<(mojito_types::param_expr::ParamId, CtValue)>,
}

impl StructBracket {
    /// The application a construction's origin binding checks the selected
    /// initializer against, beside `own`, the initializer's own origin
    /// binders its instantiation bound.
    pub(super) fn application<'a>(
        &'a self,
        own: &'a [(
            mojito_types::origin::OriginParamId,
            mojito_types::origin::Origin,
        )],
    ) -> super::origins::StructApplication<'a> {
        super::origins::StructApplication {
            origins: &self.bound,
            mutabilities: &self.mutabilities,
            own,
        }
    }
}

impl Checker {
    /// The type a function-type or `__generator_type` expression spells,
    /// at a nesting level of its own, so an application spelled in its
    /// signature is one nested in the annotation. Any other expression is
    /// an invariant violation.
    pub(super) fn callable_type(&self, expr: &Expr) -> Result<Ty, TypeError> {
        let depth = self.annotation_depth.get();
        self.annotation_depth.set(depth + 1);
        let ty = match &expr.kind {
            ExprKind::FunctionType(callable) => self.function_type(callable),
            ExprKind::GeneratorType { params, result } => self.generator_type(params, result),
            _ => Err(TypeError::InvariantViolation(
                "a callable type is a function type or a generator type".to_string(),
            )),
        };
        self.annotation_depth.set(depth);
        ty
    }

    /// The type `annotation` denotes, upstream's `emitExprType`: the one
    /// compile-time builder in the type context, where the `None` value is
    /// the none type, at the annotation's nesting level
    /// ([`Self::at_annotation_depth`]), so only the outermost application
    /// may leave its origin slots to inference.
    pub(super) fn emit_expr_type(&self, annotation: &Expr) -> Result<Ty, TypeError> {
        if matches!(annotation.kind, ExprKind::None) {
            return Ok(Ty::none());
        }
        self.at_annotation_depth(annotation, || self.annotation_type(annotation))
    }

    /// Run `emit` for the type expression `annotation` at its nesting
    /// level: one that is not itself a name or a bracket application opens
    /// a level, as an application opens its own ([`Self::applied_type`]).
    pub(super) fn at_annotation_depth<R>(&self, annotation: &Expr, emit: impl FnOnce() -> R) -> R {
        if matches!(annotation.kind, ExprKind::Identifier(_))
            || annotation.bracket_application().is_some()
        {
            return emit();
        }
        let depth = self.annotation_depth.get();
        self.annotation_depth.set(depth + 1);
        let built = emit();
        self.annotation_depth.set(depth);
        built
    }

    /// The type an annotation slot's expression denotes ([`Self::emit_expr_type`]).
    pub(super) fn ty_from_anno(&self, annotation: &Expr) -> Result<Ty, TypeError> {
        self.emit_expr_type(annotation)
    }

    /// The function type `callable` spells, its signature resolved in the
    /// scopes already open; `decls` is the classified list its origin
    /// reference signatures (`ref[o] T`) read, its own followed by any
    /// context.
    /// The type a struct field stores: its annotation's type, or for a kept
    /// direct `ref` field ([`mojito_ast::ast::Param::reference`]) the
    /// reference to it ([`Self::reference_field_ty`]).
    pub(super) fn field_ty(&self, field: &mojito_ast::ast::Param) -> Result<Ty, TypeError> {
        let referent = self.ty_from_anno(&field.ty)?;
        match &field.reference {
            Some(spec) => self.reference_field_ty(spec, referent),
            None => Ok(referent),
        }
    }

    /// The reference a direct `ref[spec]` field stores to `referent`: one
    /// named origin parameter of the struct (or an untracked immutable
    /// origin), plus at most one address space. An `UnsafeAnyOrigin` cannot
    /// hide in a stored field.
    pub(super) fn reference_field_ty(
        &self,
        spec: &mojito_ast::ast::OriginSpec,
        referent: Ty,
    ) -> Result<Ty, TypeError> {
        let (origins, address_space) =
            super::origins::split_ref_clause(spec, &self.enclosing_param_decls, self)?;
        let [origin_expr] = &*origins else {
            return Err(TypeError::Unsupported(
                "reference-valued fields currently require one origin parameter".to_string(),
            ));
        };
        let Some(origin_name) = super::origins::origin_binder_name(origin_expr) else {
            return Err(TypeError::Unsupported(
                "reference-valued fields require a named origin parameter".to_string(),
            ));
        };
        let origin_name = origin_name.to_string();
        let declared = self
            .origin_binder(&self.enclosing_type_params, &origin_name)
            .is_none()
            .then(|| super::origins::OriginDecls::value(self, origin_expr))
            .flatten();
        match declared {
            Some((mojito_types::origin::Origin::UnsafeAny { .. }, _)) => {
                return Err(TypeError::Unsupported(
                    "an UnsafeAnyOrigin reference cannot be hidden in a stored reference field"
                        .to_string(),
                ));
            }
            Some((origin @ mojito_types::origin::Origin::Untracked { mutable: false }, _)) => {
                return Ok(TyKind::Ref(mojito_types::origin::RefTy {
                    referent,
                    origin,
                    mutability: mojito_types::origin::Mutability::Immutable,
                    address_space,
                })
                .into());
            }
            _ => {}
        }
        let (decl, id) = self
            .origin_binder(&self.enclosing_type_params, &origin_name)
            .filter(|(decl, _)| !decl.is_origin_set())
            .ok_or_else(|| TypeError::UndefinedVariable(origin_name.clone()))?;
        Ok(TyKind::Ref(mojito_types::origin::RefTy {
            referent,
            origin: mojito_types::origin::Origin::Param(id),
            mutability: super::generics::origin_binder_mutability(&decl, id),
            address_space,
        })
        .into())
    }

    pub(super) fn function_signature_type(
        &self,
        callable: &FunctionTypeExpr,
        decls: &[ParamDecl],
    ) -> Result<Ty, TypeError> {
        let FunctionTypeExpr {
            params,
            ret,
            ret_reference,
            thin,
            raises,
            raises_type,
            ..
        } = callable;
        let environment = if *thin {
            CallableEnvironment::Thin
        } else {
            CallableEnvironment::Default
        };
        let function_params: Vec<FnParam> = params
            .iter()
            .enumerate()
            .map(|(index, parameter)| FnParam {
                name: parameter
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("arg{index}")),
                ty: parameter.ty.clone(),
                default: None,
                kind: parameter.kind,
                convention: parameter.convention,
                origin: parameter.origin.clone(),
            })
            .collect();
        let regular: Vec<&FnParam> = function_params
            .iter()
            .filter(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Regular)
            .collect();
        let parameter_types = regular
            .iter()
            .map(|parameter| self.ty_from_anno(&parameter.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let variadic = function_params
            .iter()
            .find(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Variadic)
            .map(|parameter| self.ty_from_anno(&parameter.ty))
            .transpose()?;
        let kw_variadic = function_params
            .iter()
            .find(|parameter| parameter.kind == mojito_ast::ast::ParamKind::KwVariadic)
            .map(|parameter| self.ty_from_anno(&parameter.ty))
            .transpose()?;
        let (return_type, ref_return) = match ret_reference {
            Some(origin) => (
                self.emit_expr_type(ret)?,
                Some(Box::new(
                    self.lower_callable_ref_sig(origin, decls, &regular)?,
                )),
            ),
            None => (self.ty_from_anno(ret)?, None),
        };
        Ok(TyKind::Func(Box::new(FuncTy {
            environment,
            params: parameter_types,
            names: function_params
                .iter()
                .filter(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Regular)
                .map(|parameter| parameter.name.clone())
                .collect(),
            ret: return_type,
            required: vec![true; regular.len()],
            variadic,
            kw_variadic,
            positional_only: None,
            keyword_only: None,
            raises: *raises,
            error: if *raises {
                Some(match raises_type {
                    Some(error) => self.ty_from_anno(error)?,
                    None => Ty::error(),
                })
            } else {
                None
            },
            conventions: function_params
                .iter()
                .filter(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Regular)
                .map(|parameter| parameter.convention)
                .collect(),
            ref_params: self.lower_callable_ref_param_sigs(decls, &regular)?,
            ref_return,
            transfers: TransferSet::default(),
        }))
        .into())
    }

    /// The argument `expr` of the callable-value parameter `decl`, named
    /// `name` and typed `ty`, part of the instance's identity for a struct
    /// and a `def` alike: a function is its symbol, as the pin's
    /// `S[#kgen.func.symbol<@f>]`, and an enclosing callable binder (`f`,
    /// `Self.f`) or its application (`f[4]`) its reference, which the elaborator substitutes per
    /// instance. A capture-free nested `def` or lambda is the symbol of
    /// its generator applied to the binders in scope where it is declared
    /// ([`Self::nested_callable_symbol`]). A run-time local is the pin's
    /// dynamic-value rejection. A nested body declared where no open body
    /// names its binders, a static read through a parametric struct, and
    /// any argument at a generic callable slot keep the deferred marker, which
    /// the elaborator binds from the argument's register.
    fn resolve_callable_param_arg(
        &self,
        decl: &ParamDecl,
        name: &str,
        ty: &Ty,
        expr: &Expr,
    ) -> Result<TyArg, TypeError> {
        // A callee's binder substitution leaves its projections on the
        // caller's concrete types open (`def(var List[Int].IteratorType[o]
        // .Element)`); the argument matches the type they close to.
        let ty = &self.resolve_assoc_ty(ty);
        // An enclosing callable binder, or a bracket applying one (`f[4]`,
        // upstream's `#kgen.bind_params`), is a compile-time reference, in
        // a signature as in a body, where no run-time binding names it.
        if let Ok(CtValue::Expr(binder)) = self.eval_associated_ct(expr)
            && (binder.as_decl_ref().is_some()
                || matches!(binder.kind(), ParamKind::BindParams { .. }))
            && let Some(actual) = binder.meta().as_value()
        {
            if !self.value_coerces(actual, ty) {
                return Err(TypeError::TypeMismatch {
                    expected: ty.to_string(),
                    found: actual.to_string(),
                    context: format!("callable-value parameter '{name}'"),
                });
            }
            return Ok(TyArg::Val(CtValue::Expr(binder)));
        }
        let actual = self.infer(expr)?;
        // A capture-free generic lambda is its generator's symbol with its
        // own binders open, a thin value as at the pin; the check types it
        // `capturing[]` only because it gives a generic lambda no thin type.
        if matches!(ty.kind(), TyKind::GenericFunc(_))
            && matches!(expr.kind, ExprKind::Lambda { .. })
            && !super::is_closure_storage(&actual)
            && let Some(symbol) = self.nested_callable_symbol(expr)
        {
            let thin = super::with_callable_environment(
                actual,
                mojito_types::origin::CallableEnvironment::Thin,
            );
            if !self.value_coerces(&thin, ty) {
                return Err(TypeError::TypeMismatch {
                    expected: ty.to_string(),
                    found: thin.to_string(),
                    context: format!("callable-value parameter '{name}'"),
                });
            }
            return Ok(TyArg::Val(CtValue::FuncSymbol(symbol)));
        }
        // A closure's storage is a run-time value, and so is a capturing
        // lambda's closure, as at the pin.
        let captures = super::is_closure_storage(&actual)
            || actual
                .callable()
                .and_then(|f| f.environment.captures())
                .is_some();
        if matches!(expr.kind, ExprKind::Lambda { .. }) && captures {
            return Err(TypeError::CapturingLambdaParameter);
        }
        if super::is_closure_storage(&actual) {
            return Err(TypeError::DynamicParameterValue);
        }
        // A parametric function binds none of its parameters, so it is no
        // value of a non-parametric function type: upstream infers no
        // specialization from the slot.
        if matches!(ty.kind(), TyKind::Func(_)) && super::is_unbound_parametric_function(&actual) {
            return Err(TypeError::ParamValueType {
                callee: mojito_symbol::symbol::declaration_owner_name(&decl.id().owner)
                    .unwrap_or(&decl.id().owner)
                    .to_string(),
                param: name.to_string(),
                expected: ty.to_string(),
                found: actual.to_string(),
            });
        }
        if !self.value_coerces(&actual, ty) {
            return Err(TypeError::TypeMismatch {
                expected: ty.to_string(),
                found: actual.to_string(),
                context: format!("callable-value parameter '{name}'"),
            });
        }
        let symbolic = matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_));
        if symbolic && let Some(symbol) = self.nested_callable_symbol(expr) {
            return Ok(TyArg::Val(CtValue::FuncSymbol(symbol)));
        }
        if let ExprKind::Identifier(local) = &expr.kind
            && self.is_local_binding(local)
        {
            if self.compile_time_callee(local) {
                return Ok(TyArg::Val(CtValue::Deferred(decl.binder())));
            }
            if !self.reads_compile_time_binding(local) {
                return Err(TypeError::DynamicParameterValue);
            }
        }
        if symbolic && let Some((_, symbol)) = self.callable_value_symbol(expr, ty)? {
            return Ok(TyArg::Val(CtValue::FuncSymbol(symbol)));
        }
        Ok(TyArg::Val(CtValue::Deferred(decl.binder())))
    }

    /// The argument of a generator-typed parameter (`apply[Twice]()`,
    /// `apply[S.k]()`, `apply[G]()`): a generator that converts to the
    /// declared generator type ([`mojito_types::types::generator_converts`]),
    /// as upstream converts one, constant or still a parameter. Any other
    /// argument is rejected with the pin's parameter-type mismatch.
    fn resolve_generator_param_arg(
        &self,
        decl: &ParamDecl,
        expected: &Ty,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use mojito_ast::ast::ParamArg;
        let expr = match arg {
            ParamArg::Named { value, .. } => {
                return self.resolve_generator_param_arg(decl, expected, value);
            }
            ParamArg::Value(expr) => expr.clone(),
        };
        let generator = self.compile_dependent_ct_expr(&expr)?;
        if !generator
            .meta()
            .is_generator_of(&mojito_types::param_expr::MetaTy::value(expected.clone()))
        {
            return Err(TypeError::ParamValueType {
                callee: mojito_symbol::symbol::declaration_owner_name(&decl.id().owner)
                    .unwrap_or(&decl.id().owner)
                    .to_string(),
                param: decl.name().to_string(),
                expected: expected.to_string(),
                found: generator.meta().to_string(),
            });
        }
        Ok(TyArg::Val(
            self.converted_generator(generator, expected)?.into_value(),
        ))
    }

    /// A constant generator converted to the generator type `expected`, as
    /// upstream's generator conversion converts the body: a literal body
    /// (`One[a: Int] = 1`) takes the expected result type (`Int`).
    fn converted_generator(
        &self,
        generator: ParamExpr,
        expected: &Ty,
    ) -> Result<ParamExpr, TypeError> {
        let (Some(CtValue::Generator(value)), TyKind::Generator(expected)) =
            (generator.as_constant(), expected.kind())
        else {
            return Ok(generator);
        };
        let Some(body) = value
            .body
            .as_constant()
            .filter(|_| value.body.meta().as_value() != Some(&expected.result))
        else {
            return Ok(generator);
        };
        let context = &self.param_context;
        let body = context
            .constant_as(body.clone(), &expected.result)
            .map_err(param_error)?;
        context
            .constant(CtValue::Generator(Box::new(
                mojito_types::ct::GeneratorValue {
                    params: value.params.clone(),
                    body,
                },
            )))
            .map_err(param_error)
    }

    /// Upstream's generator type `__generator_type[params] result`
    /// ([`TyKind::Generator`]): its parameters classified as a declaration's
    /// are (a value parameter by its type, a type parameter by its bounds),
    /// under the one owner every generator type's binders share, since a
    /// generator converts by signature, names aside. The list scopes the
    /// result, so a result naming its own binders (`T`, `List[T]`) names
    /// them; a result naming a trait is the trait used as a type, the type
    /// of a type a generator yields.
    fn generator_type(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        result: &Expr,
    ) -> Result<Ty, TypeError> {
        let decls =
            self.classify_param_list(mojito_types::types::GENERATOR_BINDER_OWNER, params)?;
        // The list scopes the result, as a declaration's list scopes its
        // signature: `__generator_type[T: Copyable] List[T]`.
        self.push_param_scope(&decls);
        let result = match &result.kind {
            // A trait-valued binder in scope is the trait it binds used as a
            // type (`__generator_type[From: From] To` over `To:
            // type_of(AnyType)`), symbolic until the binder is bound.
            ExprKind::Identifier(name)
                if self.lookup_tparam(name).is_none()
                    && let Some(binder) = self.trait_valued_binder(name) =>
            {
                Ok(mojito_types::types::DependentType::resolve(binder))
            }
            // A trait or an `&` composition of traits is the trait value
            // used as a type, as upstream's `AnyTrait` value of `&` is.
            _ => match self.trait_value(result) {
                Some(value) => value.map(|value| TyKind::Trait(value.traits).into()),
                None => self.emit_expr_type(result),
            },
        };
        self.pop_param_scope();
        let result = result?;
        Ok(Ty::generator(decls, result))
    }

    /// The type `name[args]` denotes, beside what each of its bracket
    /// arguments bound to, as an annotation resolves it.
    pub(super) fn applied_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let depth = self.annotation_depth.get();
        self.annotation_depth.set(depth + 1);
        let resolved = self.applied_type_at_depth(name, args);
        self.annotation_depth.set(depth);
        resolved
    }

    /// The type `name[args]` names: the application [`Self::applied_type`]
    /// resolves, or, when the head names no type, the expression it
    /// spells ([`Self::value_headed_type`]).
    pub(super) fn named_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        match self.applied_type(name, args) {
            Err(TypeError::UnknownType(unknown)) if unknown == name => {
                self.value_headed_type(name, args)
            }
            resolved => resolved.map(|(ty, _)| ty),
        }
    }

    /// The type a `Named` annotation denotes when its head names no type:
    /// the expression it spells (the head, or its subscript by the bracket
    /// arguments), built as upstream's `emitExprType` builds any annotation,
    /// so a compile-time value's subscript is its `__getitem_param__`
    /// element and a value alone is "expected a type". The spelling is asked
    /// first, so a local value never shadows a type of its name here. A head
    /// that names nothing at all stays an unknown type.
    pub(super) fn value_headed_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        use mojito_ast::ast::{ParamArg, SubscriptArg};
        let unknown = || TypeError::UnknownType(name.to_string());
        let operand = |arg: &ParamArg| match arg {
            ParamArg::Value(value) => Some(value.clone()),
            ParamArg::Named { .. } => None,
        };
        let operands = args
            .iter()
            .map(operand)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(unknown)?;
        let span = operands
            .first()
            .map_or_else(mojito_common::token::Span::default, |first| first.span);
        let head = Expr::new(ExprKind::Identifier(name.to_string()), span);
        let expr = match <[Expr; 1]>::try_from(operands) {
            Ok([index]) => Expr::new(
                ExprKind::Index {
                    object: Box::new(head),
                    index: Box::new(index),
                },
                span,
            ),
            Err(operands) if operands.is_empty() => head,
            Err(operands) => Expr::new(
                ExprKind::MultiIndex {
                    object: Box::new(head),
                    args: operands.into_iter().map(SubscriptArg::Index).collect(),
                },
                span,
            ),
        };
        self.denoted_type(&expr, "type annotation")
            .map_err(|error| match error {
                TypeError::NotComptime(unbound) if unbound == name => unknown(),
                error => error,
            })
    }

    /// The arguments the enclosing struct's `Self` gives its argument
    /// binders (`self_decls`), position by position.
    pub(super) fn self_view_arguments(&self) -> Option<std::borrow::Cow<'_, [TyArg]>> {
        let (owner, arguments) = self.self_ty.as_ref()?.kind().as_non_pointer_struct()?;
        Some(
            self.structs
                .get(owner)
                .map_or(std::borrow::Cow::Borrowed(arguments.as_slice()), |info| {
                    info.view_arguments(arguments)
                }),
        )
    }

    /// The type the enclosing struct's type parameter at `index` denotes
    /// as `Self` binds it: the argument a concrete `Self` supplies (`Self.T`
    /// is `Int` while `Self` is `Optional[Int]`), else the parameter itself.
    pub(super) fn self_type_parameter(&self, index: usize) -> Option<Ty> {
        let decl = self.self_decls.get(index)?;
        if matches!(decl, ParamDecl::Type { .. })
            && let Some(arguments) = self.self_view_arguments()
            && arguments.len() >= self.self_decls.len()
            && let Some(TyArg::Ty(bound)) = arguments.get(index)
            && !matches!(bound.kind(), TyKind::Param { .. })
        {
            return Some(bound.clone());
        }
        type_parameter(decl)
    }

    /// `Self.NAME`, one of the enclosing struct's own type parameters as
    /// `Self` binds it ([`Self::self_type_parameter`]), else a type member
    /// of `Self` ([`Self::associated_type_for_self`]), as upstream reads
    /// `Self.Ts` from the struct's parameter list. The struct's own pack
    /// is named by its starred spelling (`*Ts`).
    pub(super) fn self_pack_type(&self, name: &str) -> Result<Ty, TypeError> {
        self.self_decls
            .iter()
            .position(|decl| decl.name() == name)
            .and_then(|index| self.self_type_parameter(index))
            .map_or_else(|| self.associated_type_for_self(name), Ok)
    }

    /// The parametric function type `callable` spells inline, in a type
    /// argument or an annotation (`OptionalReg[def[d: DType](Scalar[d])
    /// thin -> Scalar[d]]`): its own list opened over the enclosing
    /// declaration's, as an alias body or a callable bound opens it. The
    /// binders are owned by the nesting level of inline spellings, so two
    /// identical spellings are one type, as upstream identifies a function
    /// type by its spelling, and a spelling nested in another's signature
    /// never shares the outer list's binders.
    fn inline_callable_type(&self, callable: &FunctionTypeExpr) -> Result<Ty, TypeError> {
        let level = self.inline_callable_depth.get();
        self.inline_callable_depth.set(level + 1);
        let lowered = self.lower_anonymous_callable_type(
            &format!("$callable:inline:{level}"),
            callable,
            &self.enclosing_type_params,
        );
        self.inline_callable_depth.set(level);
        lowered
    }

    /// The function type `callable` spells, upstream's `emitExprType` of a
    /// function-type expression. One with its own `def[...]` list is
    /// resolved in the scope of that list wherever it is spelled, as
    /// upstream emits every type in the scope of its own parameters: a
    /// `GenericFunc` over its binders.
    fn function_type(&self, callable: &FunctionTypeExpr) -> Result<Ty, TypeError> {
        if !callable.type_params.is_empty() {
            return self.inline_callable_type(callable);
        }
        // A clause with no list has no binder to constrain.
        if !callable.where_clauses.is_empty() {
            return Err(TypeError::Unsupported(
                "a 'where' clause on a function type is only supported on a \
                 compile-time callable parameter bound with def[...] parameters"
                    .to_string(),
            ));
        }
        self.function_signature_type(callable, &[])
    }

    /// The parameter list of a parameterized associated type `name` reachable from
    /// a base type — a trait's abstract `Self` or a bounded type parameter — or
    /// `None` when the member is monomorphic, absent, or on a concrete struct
    /// (whose concrete resolution is handled separately).
    fn parameterized_assoc_params(
        &self,
        base_ty: &Ty,
        name: &str,
    ) -> Option<Vec<mojito_ast::ast::TypeParam>> {
        match base_ty.kind() {
            TyKind::SelfType => self
                .trait_self_comptime
                .last()
                .and_then(|reqs| reqs.get(name))
                .and_then(|req| match req {
                    CtMemberReq::Type { params, .. } if !params.is_empty() => Some(params.clone()),
                    _ => None,
                }),
            TyKind::Param { .. } => {
                self.lookup_trait_assoc_params(&self.refined_bound(base_ty)?, name)
            }
            // A concrete struct that instantiates the member: its parameter list
            // comes from the struct's own parameterized definition, so an
            // application such as a conformer's `Self.IteratorType[origin_of(self)]`
            // resolves concretely rather than falling through to dependent indexing.
            _ if base_ty.kind().is_non_pointer_struct() || base_ty.is_pointer() => self
                .nominal_struct(base_ty)
                .and_then(|(sname, _)| self.structs.get(sname))
                .and_then(|info| info.parameterized_associated.get(name))
                .map(|member| member.params.clone()),
            _ => None,
        }
    }

    /// The type an indexed projection `base[index]` denotes: an unbound
    /// pack's dependent element (`Self.Ts[i]`),
    /// a parameterized member application (`Self.IteratorType[o]`,
    /// `C.IteratorType[o]`), or else an index into a dependent type
    /// sequence (`values.Ts[i]`).
    pub(super) fn indexed_projection_type(
        &self,
        base: &Expr,
        index: &Expr,
    ) -> Result<Ty, TypeError> {
        if let Some(pack) = self.unbound_pack_type(base) {
            return self.pack_element_type(&pack, index);
        }
        // A parameterized member application is spelled like a dependent
        // index; it resolves as an application rather than as sequence
        // indexing.
        let application = match &base.kind {
            // `Self.IteratorType[..]`: the concrete `Self` (a registered
            // struct) when there is one, so the member resolves concretely;
            // otherwise a trait's abstract `Self`.
            _ if let Some(name) = base.self_member() => {
                let base = match &self.self_ty {
                    Some(ty)
                        if ty.is_simd() || ty.is_pointer() || ty.kind().is_non_pointer_struct() =>
                    {
                        ty.clone()
                    }
                    _ => Ty::self_type(),
                };
                let args = [mojito_ast::ast::subscript_param_arg(index)];
                let applied = if self.declared_member(&base, name) {
                    Some(self.member_application(&base, name, &args)?)
                } else {
                    self.parameterized_assoc_application(&base, name, &args)?
                };
                // `Self`'s own member is no dependent type sequence.
                return applied.ok_or_else(dependent_indexing_unsupported);
            }
            // `C.IteratorType[..]` over a bounded type parameter or another
            // base; a base that names no type (`values.element_types` over a
            // value binding) falls through to dependent sequence indexing.
            ExprKind::Member { object, field } => {
                self.member_index_application(object, field, index)?
            }
            _ => None,
        };
        if let Some(applied) = application {
            return Ok(applied);
        }
        let elements = self.dependent_type_sequence(base)?;
        let index = self.compile_dependent_ct_expr(index)?;
        self.resolve_dependent_index(elements, &index, &DependentValues::default())
    }

    /// Resolve a parameterized associated-type application `base.name[args]`.
    /// Returns `None` when `name` is not a parameterized associated member of
    /// `base` (so the caller falls back to dependent sequence indexing). The
    /// result is a symbolic associated type; concrete resolution happens once the
    /// base is a conforming struct (a later iteration subtask).
    pub(super) fn parameterized_assoc_application(
        &self,
        base_ty: &Ty,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<Ty>, TypeError> {
        let Some(params) = self.parameterized_assoc_params(base_ty, name) else {
            return Ok(None);
        };
        // Explicit parameters (after the `//` infer-only marker) are supplied at
        // the application; infer-only parameters are derived from them.
        let explicit: Vec<&mojito_ast::ast::TypeParam> =
            params.iter().filter(|p| !p.infer_only).collect();
        if args.len() != explicit.len() {
            return Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: explicit.len(),
                got: args.len(),
            });
        }
        // A struct member's checked declarations tell a value parameter
        // whose type the syntax cannot (`idx: SIMDLength`).
        let declared = self
            .nominal_struct(base_ty)
            .and_then(|(struct_name, _)| self.structs.get(struct_name))
            .and_then(|info| info.parameterized_associated.get(name))
            .map(|member| match &member.template {
                MemberTemplate::Type { decls, .. } | MemberTemplate::Value { decls, .. } => {
                    decls.clone()
                }
            });
        // Carry the application arguments in the checked type. The base here is
        // still abstract (`Self` or a bounded parameter). An `origin_of(self)`
        // argument in a trait method's abstract signature has no bound `self`
        // place; it lowers to the symbolic `Origin::SelfParam`, resolved to the
        // concrete receiver origin once the base is a conforming struct. Concrete
        // substitution and per-kind argument validation happen there.
        let origins: Vec<bool> = params
            .iter()
            .enumerate()
            .filter(|(_, param)| !param.infer_only)
            .map(|(index, _)| self.origin_binder_at(&params, index).is_some())
            .collect();
        let arguments = explicit
            .iter()
            .zip(origins)
            .zip(args)
            .map(|((param, origin), arg)| match (arg, &declared) {
                (mojito_ast::ast::ParamArg::Value(value), Some(decls))
                    if decls.iter().any(|decl| {
                        decl.name() == param.name && matches!(decl, ParamDecl::Value { .. })
                    }) =>
                {
                    self.eval_associated_ct(value).map(TyArg::Val)
                }
                _ => self.lower_assoc_application_arg(param, origin, arg),
            })
            .collect::<Result<Vec<_>, _>>()?;
        // A concrete struct base instantiates the member here and now (a conformer
        // spelling `Self.IteratorType[origin_of(self)]` as its own return type):
        // resolve it through the same path as a non-indexed struct-based
        // application. An abstract base (`Self` or a bounded parameter) stays
        // symbolic until the base becomes a conforming struct.
        if base_ty.kind().is_non_pointer_struct() || base_ty.is_pointer() {
            return Ok(Some(
                self.associated_type_from_base(base_ty, name, &arguments)?,
            ));
        }
        Ok(Some(
            TyKind::Assoc {
                base: base_ty.clone(),
                name: name.to_string(),
                args: arguments,
            }
            .into(),
        ))
    }

    /// The type member `name` of `base`, applied to `args` (none for an
    /// unparameterized member): `T.Element`, `S[Int].Of[Bool]`, `S.Of[Bool]`.
    pub(super) fn member_type(
        &self,
        base: &Expr,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        let (base_ty, generator) = self.member_base(base)?;
        let member = if args.is_empty() {
            self.associated_type_from_base(&base_ty, name, &[])?
        } else {
            self.member_application(&base_ty, name, args)?
        };
        self.generator_member_type(generator, name, member)
    }

    /// `member`, the type member `name` read through a base
    /// [`Self::member_base`] resolved, once a generator base's read is
    /// cleared of its unbound parameters.
    fn generator_member_type(
        &self,
        generator: Option<&str>,
        name: &str,
        member: Ty,
    ) -> Result<Ty, TypeError> {
        if let Some(base) = generator {
            self.reject_unbound_dependence(base, name, &CtValue::Type(member.clone()))?;
        }
        Ok(member)
    }

    /// The type a member read's base names or constructs, beside the
    /// struct's name when the base is a bare parameterized struct, read as
    /// its unbound generator ([`Self::unbound_generator`]).
    pub(super) fn member_base<'a>(
        &self,
        base: &'a Expr,
    ) -> Result<(Ty, Option<&'a str>), TypeError> {
        if let ExprKind::Identifier(name) = &base.kind
            && let Some(generator) = self.unbound_generator(name)
        {
            return Ok((generator, Some(name)));
        }
        // A compile-time construction's member is its type's (`S[2]().H`).
        if matches!(
            base.kind,
            ExprKind::Call { .. } | ExprKind::Invoke { .. } | ExprKind::MethodCall { .. }
        ) && !self.may_denote_type(base)
        {
            return self.compile_time_value_type(base).map(|ty| (ty, None));
        }
        self.emit_expr_type(base)
            .or_else(|error| self.value_base_type(base).ok_or(error))
            .map(|ty| (ty, None))
    }

    /// `base.name[args]` over a whole bracket list (`S[Int].Of[Bool]`,
    /// `M.Pair[1, 2]`). A struct's parameterized member, a type or a value
    /// denoting one (`TypeList.__getitem_param__[SIMDLength(0)]`), binds the
    /// list through the contract an alias application uses (keywords,
    /// defaults, bounds, a value's declared type); an abstract base lowers
    /// each argument at its declared parameter.
    pub(super) fn member_application(
        &self,
        base_ty: &Ty,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        if let Some((struct_name, _)) = self.nominal_struct(base_ty)
            && let Some(member) = self
                .structs
                .get(struct_name)
                .and_then(|info| info.parameterized_associated.get(name))
            && let MemberTemplate::Type { decls, .. } | MemberTemplate::Value { decls, .. } =
                &member.template
        {
            // Every parameter binds, an infer-only one (`Trait` of
            // `TypeList.of`) from its keyword or the arguments it types.
            let slots: Option<Vec<usize>> = member
                .params
                .iter()
                .map(|param| decls.iter().position(|decl| decl.name() == param.name))
                .collect();
            if let Some(slots) = slots {
                let label = format!("{base_ty}.{name}");
                let bound = self.resolve_use_params(&label, decls, args, &[], &[])?;
                let arguments: Vec<TyArg> = slots
                    .into_iter()
                    .map(|index| {
                        ParamDecl::source_argument(decls, &bound.arguments, index)
                            .cloned()
                            .unwrap_or_else(|| bound.arguments[index].clone())
                    })
                    .collect();
                return self.associated_type_from_base(base_ty, name, &arguments);
            }
        }
        self.parameterized_assoc_application(base_ty, name, args)?
            .ok_or_else(|| TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.to_string(),
            })
    }

    /// `inner.name[index]`, a one-argument application of `inner`'s
    /// parameterized member, or `None` when `inner` names no type or `name`
    /// is no parameterized member of it. A member with an infer-only
    /// parameter (`ParameterList.of[1]`) binds its bracket as a whole list
    /// does, which infers it.
    fn member_index_application(
        &self,
        inner: &Expr,
        name: &str,
        index: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        let Ok((base_ty, generator)) = self.member_base(inner) else {
            return Ok(None);
        };
        let infers = self
            .parameterized_assoc_params(&base_ty, name)
            .is_some_and(|params| params.iter().any(|param| param.infer_only));
        let applied = if infers || self.declared_member(&base_ty, name) {
            Some(self.member_application(
                &base_ty,
                name,
                &[mojito_ast::ast::subscript_param_arg(index)],
            )?)
        } else {
            self.parameterized_assoc_application(
                &base_ty,
                name,
                &[mojito_ast::ast::subscript_param_arg(index)],
            )?
        };
        applied
            .map(|applied| self.generator_member_type(generator, name, applied))
            .transpose()
    }

    /// Whether `name` is a concrete struct's parameterized member whose
    /// bracket binds against its declared parameters, as a whole
    /// list does: a member with no origin parameter, which the declared
    /// parameters erase.
    fn declared_member(&self, base_ty: &Ty, name: &str) -> bool {
        let Some((struct_name, _)) = self.nominal_struct(base_ty) else {
            return false;
        };
        self.structs
            .get(struct_name)
            .and_then(|info| info.parameterized_associated.get(name))
            .is_some_and(|member| !self.declares_origin_metadata(&member.params))
    }

    /// Lower one application argument of a parameterized associated type into a
    /// checked `TyArg`, dispatched by the declared parameter's kind: an origin
    /// parameter takes an `origin_of(...)`/builtin origin, a value parameter a
    /// compile-time value, and a type parameter a type. `origin` tells
    /// whether `param` is an origin binder ([`Self::origin_binder_at`]).
    fn lower_assoc_application_arg(
        &self,
        param: &mojito_ast::ast::TypeParam,
        origin: bool,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use mojito_ast::ast::ParamArg;
        // A conversion (`ImmOrigin(o)`) or cast (`o.unsafe_mut_cast[m]()`)
        // binds the member's origin at its mutability, as the member's
        // infer-only `mut` binder takes it.
        if origin
            && let ParamArg::Value(expression) = argument
            && let Some(read) = self
                .origin_method_cast(expression)
                .or_else(|| self.origin_conversion(expression))
        {
            return read.map(|(origin, _)| TyArg::Origin(origin));
        }
        if origin {
            return match self.explicit_origin_argument(argument) {
                Ok(origin) => Ok(TyArg::Origin(origin)),
                // A signature may name a parameter's place only the body can
                // resolve (`C.IteratorType[origin_of(items)]`), as a struct
                // application's origin tail may: it is left to the call.
                Err(_)
                    if self.signature_origin_leniency.get()
                        && syntactic_origin_argument(argument) =>
                {
                    Ok(TyArg::Origin(mojito_types::origin::Origin::Unbound))
                }
                Err(error) => Err(error),
            };
        }
        let arg = match argument {
            ParamArg::Value(arg) => arg,
            ParamArg::Named { name, .. } => {
                return Err(TypeError::Unsupported(format!(
                    "the keyword argument '{name}=' of an associated member application"
                )));
            }
        };
        if is_value_parameter(param) {
            return Ok(TyArg::Val(self.eval_associated_ct(arg)?));
        }
        // A `!kgen.type` binder takes a trait as a type
        // (`_MLIR.KGENParamListType[Tr]`).
        if param.lone_bound() == Some(TYPE_OF_ALL_TYPES)
            && let Some(ty) = self.trait_type_arg(argument)
        {
            return ty.map(TyArg::Ty);
        }
        // A type parameter: a bare identifier or a member chain rooted at a
        // name that is no binding (`M.KGENTypeType`) names a type;
        // otherwise the argument must evaluate to a compile-time type value.
        let ty = match &arg.kind {
            ExprKind::Identifier(_) => self.emit_expr_type(arg)?,
            ExprKind::Member { .. } if self.may_denote_type(arg) => self.emit_expr_type(arg)?,
            _ => match self.eval_associated_ct(arg)? {
                CtValue::Type(ty) => ty,
                _ => {
                    return Err(TypeError::TypeMismatch {
                        expected: "a type".to_string(),
                        found: "a value".to_string(),
                        context: format!("associated type parameter '{}'", param.name),
                    });
                }
            },
        };
        Ok(TyArg::Ty(ty))
    }

    /// The type the application `name[args]` names at the current depth, as
    /// [`Self::applied_type`] answers it.
    fn applied_type_at_depth(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        if let Some(applied) = self.builtin_applied_type(name, args)? {
            return Ok(applied);
        }
        // Literal families are lang items: direct `_` holes are solved
        // from an initializer before ordinary generic-bound checking.
        if name == "List" {
            return self.list_type(args);
        }
        if name == "Set" {
            return self.set_type(args);
        }
        if name == "Dict" {
            return self.dict_type(args);
        }
        if name == "Tuple" {
            return self.tuple_type(args);
        }
        // Generic comptime aliases share the redeclaration-checked
        // type namespace with structs; expand an application into the
        // aliased type before the struct lookup.
        if let Some(alias) = self.comptime_aliases.get(name) {
            let alias = alias.clone();
            return self.resolve_comptime_alias(name, &alias, args);
        }
        // Relative package re-exports may retain only the sibling
        // module tail in a qualified comptime-alias reference. Resolve
        // that spelling when it selects one unique registered alias, and
        // no declared struct is spelled so (the program's own `Scalar`).
        if name.starts_with("__module$") && !self.structs.contains_key(name) {
            let leaf = name.rsplit('$').next().unwrap_or(name);
            let mut matches = self
                .comptime_aliases
                .keys()
                .filter(|candidate| candidate.rsplit('$').next() == Some(leaf));
            if let Some(candidate) = matches.next()
                && matches.next().is_none()
                && let Some(alias) = self.comptime_aliases.get(candidate)
            {
                let candidate = candidate.to_string();
                let alias = alias.clone();
                return self.resolve_comptime_alias(&candidate, &alias, args);
            }
        }
        // The declared `Pointer` struct's applications are the compiler's
        // pointer type (`Ty::new` folds them), so its bracket binds here.
        if name == mojito_types::types::POINTER_TYPE_NAME {
            return self.pointer_type(args);
        }
        if let Some(info) = self.structs.get(name) {
            // A bare parametric struct is not concrete where the position
            // must name a concrete type; only a run-time or compile-time
            // parameter's own annotation, or an initialized binding's,
            // leaves its parameters to inference.
            if args.is_empty()
                && self.application_strictness() == super::StorageStrictness::Full
                && let Some(shown) = unbound_struct_spelling(name, &info.params)
            {
                return Err(TypeError::NotConcrete(shown));
            }
            let decls = info.decls().into_owned();
            let BoundParameters {
                arguments,
                attributes,
                ..
            } = self.resolve_struct_use_args(name, &decls, args, &[], &[])?;
            return Ok((Self::struct_instance_type(name, arguments), attributes));
        }
        // The enclosing struct's own parameter spelled bare: Mojo
        // requires `Self.T` / `Self.n` inside the body.
        if args.is_empty() && self.is_enclosing_struct_param(name) {
            return Err(TypeError::UnqualifiedStructParam(name.to_string()));
        }
        // A spread the pack-qualification pass normalized to the bare
        // spelling names the enclosing struct's pack.
        if args.is_empty() && name.starts_with('*') && self.self_ty.is_some() {
            return self
                .self_pack_type(name)
                .map(|ty| (ty, DeclaredAttributes::default()));
        }
        // A `def`'s own `Ts[index]` while the pack is still a
        // parameter: the dependent element type.
        if let [mojito_ast::ast::ParamArg::Value(index)] = args
            && let Some(pack) = self.lookup_tparam(&format!("*{}", name.trim_start_matches('*')))
        {
            return self
                .pack_element_type(&pack, index)
                .map(|ty| (ty, DeclaredAttributes::default()));
        }
        Err(TypeError::UnknownType(name.to_string()))
    }

    /// The type a bracketed `__mlir_type[...]` spelling denotes, read as
    /// KGEN reads it: the text pieces joined, each operand a `{}` hole.
    /// `!kgen.param_list<T>` is upstream's `_MLIR.KGENParamListType[T]`
    /// body, the raw parameter list of `T`, and `!kgen.simd<length, dtype>`
    /// the vector storage `SIMD._mlir_value` holds ([`TyKind::KgenSimd`]),
    /// `!kgen.simd<length, address>` a vector of `length` addresses
    /// ([`TyKind::KgenAddressSimd`]),
    /// and `!kgen.pointer<T>` the default-address-space address a `Pointer`
    /// wraps ([`TyKind::KgenPointer`]). `!kgen.struct<:param_list<type> L
    /// isParamPack>` is the literal struct of a computed list of types
    /// (`VariadicPack._kgen_pack_with_pointer_type`). `!lit.origin<mut>` is
    /// an origin's raw attribute type ([`TyKind::LitOrigin`]), and
    /// `!lit.ref<T, origin._mlir_origin[, 0]>` and `!lit.ref.pack<…>` the
    /// references a `Pointer` and a `VariadicPack` hold. Every other spelling
    /// is outside the subset.
    fn mlir_applied_type(&self, args: &[mojito_ast::ast::ParamArg]) -> Result<Ty, TypeError> {
        let (text, operands) = mojito_types::types::mlir_type_spelling(args);
        match (text.as_str(), operands.as_slice()) {
            ("!kgen.param_list<{}>", [element]) => self.mlir_param_list_type(element),
            ("!kgen.simd<{},{}>", [length, dtype]) => {
                let width = self.mlir_index_operand(length)?;
                let dtype = mojito_types::types::mlir_value_operand(dtype)
                    .ok_or_else(|| mlir_operand_mismatch("!kgen.dtype", "DType"))?;
                let vector = simd_of(self.dtype_from_arg(&dtype)?, positive_width(width)?)?;
                Ok(mojito_types::types::simd_storage(&vector).unwrap_or(vector))
            }
            ("!kgen.simd<{},address>", [length]) => Ok(TyKind::KgenAddressSimd {
                width: positive_width(self.mlir_index_operand(length)?)?,
            }
            .into()),
            ("!kgen.struct<{}isParamPack>", [elements]) => self.mlir_param_pack_struct(elements),
            ("!kgen.struct<:param_list<type>{}isParamPack>", [list]) => {
                self.mlir_param_list_struct(list)
            }
            ("!lit.ref.pack<:param_list<{}>{},{}>", [element_trait, elements, origin]) => {
                self.mlir_ref_pack_type(element_trait, elements, origin)
            }
            ("!kgen.pointer<{}>", [element]) => Ok(Ty::kgen_pointer(
                self.mlir_type_operand(element)?,
                mojito_types::types::AddressSpaceSlot::GENERIC,
            )),
            ("!kgen.pointer<{},{}>", [element, address_space]) => Ok(Ty::kgen_pointer(
                self.mlir_type_operand(element)?,
                self.mlir_address_space_operand(address_space)?,
            )),
            ("!lit.ref<{},{}>", [element, origin]) => self.mlir_lit_ref_type(
                element,
                origin,
                mojito_types::types::AddressSpaceSlot::GENERIC,
            ),
            ("!lit.ref<{},{},{}>", [element, origin, address_space]) => {
                let address_space = self.mlir_address_space_operand(address_space)?;
                self.mlir_lit_ref_type(element, origin, address_space)
            }
            (text, [element, origin])
                if let Some(space) = literal_address_space(text, "!lit.ref<{},{},") =>
            {
                self.mlir_lit_ref_type(element, origin, space)
            }
            (text, [element])
                if let Some(space) = literal_address_space(text, "!kgen.pointer<{},") =>
            {
                Ok(Ty::kgen_pointer(self.mlir_type_operand(element)?, space))
            }
            ("!lit.origin<{}>", [mutability]) => {
                Ok(TyKind::LitOrigin(self.mlir_bool_operand(mutability)?).into())
            }
            ("!pop.array<{},{}>", [size, element]) => {
                let size = self.mlir_array_size_operand(size)?;
                Ok(TyKind::PopArray {
                    size,
                    element: self.mlir_type_operand(element)?,
                }
                .into())
            }
            (text, [element]) => {
                let size = text
                    .strip_prefix("!pop.array<")
                    .and_then(|rest| rest.strip_suffix(",{}>"))
                    .and_then(|size| size.parse().ok())
                    .ok_or_else(unsupported_mlir_type)?;
                Ok(TyKind::PopArray {
                    size: mojito_types::types::ArraySize::Known(size),
                    element: self.mlir_type_operand(element)?,
                }
                .into())
            }
            _ => Err(unsupported_mlir_type()),
        }
    }

    /// The `index` size operand of a `!pop.array`, an `Int`'s
    /// `__mlir_index__()` (`Self.size.__mlir_index__()`), as upstream's
    /// `StaticTuple` spells its storage.
    fn mlir_array_size_operand(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::types::ArraySize, TypeError> {
        let mojito_ast::ast::ParamArg::Value(size) = arg else {
            return Err(mlir_operand_mismatch("index", "Int.__mlir_index__()"));
        };
        let size = self.compile_dependent_ct_expr(size)?;
        mojito_types::types::ArraySize::of(size).map_err(param_error)
    }

    /// The literal struct `!kgen.struct<~Ts.values isParamPack>` of a type
    /// list's elements, which Mojito holds as the compiler-private tuple
    /// storage (`TyKind::Tuple`) of the same elements, exactly as
    /// `__RuntimeTuple[*Ts]` builds them.
    fn mlir_param_pack_struct(
        &self,
        elements: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        use mojito_ast::ast::{ParamArg, PrefixOp};
        let ParamArg::Value(Expr {
            kind: ExprKind::Prefix(PrefixOp::Invert, values),
            ..
        }) = elements
        else {
            return Err(unsupported_mlir_type());
        };
        Ok(TyKind::Tuple(self.mlir_type_list_elements(values)?).into())
    }

    /// The literal struct `!kgen.struct<:param_list<type> L isParamPack>`
    /// of the list of types a compile-time expression `L` computes
    /// ([`Self::param_list_tuple`]).
    fn mlir_param_list_struct(&self, list: &mojito_ast::ast::ParamArg) -> Result<Ty, TypeError> {
        let mojito_ast::ast::ParamArg::Value(expr) = list else {
            return Err(unsupported_mlir_type());
        };
        let list = self.compile_dependent_ct_expr(expr)?;
        if !matches!(list.meta(), MetaTy::ParamList(_)) {
            return Err(TypeError::TypeMismatch {
                expected: "a parameter list of types".to_string(),
                found: list.meta().to_string(),
                context: "the list operand of a '!kgen.struct<:param_list<type>>'".to_string(),
            });
        }
        self.param_list_tuple(list, expr)
    }

    /// The reference pack `!lit.ref.pack<:param_list<Trait> Ts.values,
    /// origin._mlir_origin>`, upstream's `VariadicPack._mlir_type`: one
    /// `!lit.ref` under `origin` to each element of the type list `Ts`.
    fn mlir_ref_pack_type(
        &self,
        element_trait: &mojito_ast::ast::ParamArg,
        elements: &mojito_ast::ast::ParamArg,
        origin: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        use mojito_ast::ast::ParamArg;
        if let Some(element_trait) = self.trait_type_arg(element_trait) {
            element_trait?;
        }
        let ParamArg::Value(values) = elements else {
            return Err(unsupported_mlir_type());
        };
        let elements = self.mlir_type_list_elements(values)?;
        let (origin, mutability) = self.mlir_origin_operand(origin)?;
        Ok(TyKind::LitRefPack {
            elements,
            origin,
            mutability,
        }
        .into())
    }

    /// The reference `!lit.ref<element, origin._mlir_origin[, as]>`, a
    /// `!lit.ref` to `element` under `origin` in `address_space`, as
    /// upstream's `Pointer._mlir_lit_ref` spells it.
    fn mlir_lit_ref_type(
        &self,
        element: &mojito_ast::ast::ParamArg,
        origin: &mojito_ast::ast::ParamArg,
        address_space: mojito_types::types::AddressSpaceSlot,
    ) -> Result<Ty, TypeError> {
        let (origin, mutability) = self.mlir_origin_operand(origin)?;
        Ok(Ty::lit_ref(mojito_types::origin::RefTy {
            referent: self.mlir_type_operand(element)?,
            origin,
            mutability,
            address_space,
        }))
    }

    /// The `index` address-space operand of a `!kgen.pointer` or a
    /// `!lit.ref`, an `AddressSpace`'s `_value._mlir_value`
    /// (`Self.address_space._value._mlir_value`), as upstream's `Pointer`
    /// spells its raw types; or a closed `SIMDLength`'s `_mlir_value`.
    fn mlir_address_space_operand(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::types::AddressSpaceSlot, TypeError> {
        use mojito_ast::ast::ParamArg;
        let not_index = || mlir_operand_mismatch("index", "AddressSpace._value");
        let operand = mojito_types::types::mlir_value_operand(arg).ok_or_else(not_index)?;
        if let ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) = &operand
            && field == mojito_types::types::ADDRESS_SPACE_VALUE_FIELD
        {
            return self.address_space_arg(&ParamArg::Value((**object).clone()));
        }
        match self.slot_value(&operand)? {
            CtValue::Expr(_) => Err(not_index()),
            closed => mojito_types::types::known_simd_length(&closed)
                .and_then(|space| u32::try_from(space).ok())
                .map(mojito_types::types::AddressSpaceSlot::Known)
                .ok_or_else(not_index),
        }
    }

    /// The raw origin operand `X._mlir_origin` of an MLIR type spelling,
    /// the origin `X` names (`Self.o`, a binder `o`, `origin_of(...)`).
    fn mlir_origin_operand(
        &self,
        origin: &mojito_ast::ast::ParamArg,
    ) -> Result<
        (
            mojito_types::origin::Origin,
            mojito_types::origin::Mutability,
        ),
        TypeError,
    > {
        super::raw_origin_argument(origin)
            .ok_or_else(unsupported_mlir_type)
            .and_then(|base| self.pointer_origin_arg(&base))
    }

    /// The elements of a type list's raw `values` (`Self.Ts.values`,
    /// `Ts.values`, any other list's) as tuple elements: an enclosing
    /// pack spreads as `*Self.Ts` does.
    fn mlir_type_list_elements(&self, values: &Expr) -> Result<Vec<Ty>, TypeError> {
        use mojito_ast::ast::ParamArg;
        let ExprKind::Member { object, field } = &values.kind else {
            return Err(unsupported_mlir_type());
        };
        if field != "values" {
            return Err(unsupported_mlir_type());
        }
        // An enclosing pack (`Self.Ts`, `Ts`) spreads as `*Self.Ts` does,
        // and any other type list by its elements.
        let spread = ParamArg::Value(Expr {
            kind: ExprKind::Spread(object.clone()),
            ..values.clone()
        });
        let (elements, _) = self.tuple_element_types(std::slice::from_ref(&spread))?;
        Ok(elements)
    }

    /// The raw parameter list `!kgen.param_list<element>` holds.
    fn mlir_param_list_type(&self, element: &mojito_ast::ast::ParamArg) -> Result<Ty, TypeError> {
        let element = match self.trait_type_arg(element) {
            Some(element) => element?,
            None => self.mlir_type_operand(element)?,
        };
        Ok(TyKind::ParamList(element).into())
    }

    /// A type operand of an MLIR type spelling: a type (`Int`,
    /// `Self.element_type`) or a type binder in scope.
    fn mlir_type_operand(&self, element: &mojito_ast::ast::ParamArg) -> Result<Ty, TypeError> {
        use mojito_ast::ast::ParamArg;
        match element {
            ParamArg::Value(
                operand @ Expr {
                    kind: ExprKind::Identifier(_),
                    ..
                },
            ) => self.emit_expr_type(operand),
            ParamArg::Value(Expr {
                kind: ExprKind::Member { object, field },
                ..
            }) if matches!(&object.kind, ExprKind::Identifier(base) if base == "__mlir_type") => {
                mojito_types::types::mlir_type_named(field).ok_or_else(unsupported_mlir_type)
            }
            ParamArg::Value(expr) if self.may_denote_type(expr) => self.emit_expr_type(expr),
            _ => Err(unsupported_mlir_type()),
        }
    }

    /// An `index` operand of an MLIR type spelling, a `SIMDLength`'s
    /// `_mlir_value`, as the length the slot holds. An `Int`'s
    /// `_mlir_value` is a scalar `!kgen.simd`, never an index, as at the
    /// pin.
    fn mlir_index_operand(&self, arg: &mojito_ast::ast::ParamArg) -> Result<SimdWidth, TypeError> {
        let not_index = || mlir_operand_mismatch("index", "SIMDLength");
        let operand = mojito_types::types::mlir_value_operand(arg).ok_or_else(not_index)?;
        let length = match self.slot_value(&operand)? {
            CtValue::Expr(length) => length,
            closed => self.param_context.constant(closed).map_err(param_error)?,
        };
        if !length.meta().as_value().is_some_and(is_simd_length) {
            return Err(not_index());
        }
        SimdWidth::of_length(&self.param_context, length).map_err(param_error)
    }

    /// The `i1` mutability operand of a `!lit.origin`, a `Bool`'s
    /// `_mlir_value` (`mut._mlir_value`, `True._mlir_value`), as upstream's
    /// `_lit_origin_type_of_mut` spells it.
    pub(in crate::checker) fn mlir_bool_operand(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::types::LitOriginMut, TypeError> {
        let not_bool = || mlir_operand_mismatch("i1", "Bool");
        let operand = mojito_types::types::mlir_value_operand(arg).ok_or_else(not_bool)?;
        let mutability = match self.slot_value(&operand)? {
            CtValue::Expr(mutability) => mutability,
            closed => self.param_context.constant(closed).map_err(param_error)?,
        };
        mojito_types::types::LitOriginMut::of(mutability).map_err(|_| not_bool())
    }

    /// The type a bracket argument denotes where a trait is a type (a raw
    /// list's element, a `!kgen.type` binder's argument), as KGEN holds a
    /// trait as the type `!lit.trait<…>`: a trait or a `&` composition is
    /// its [`TyKind::Trait`], and a trait-valued binder in scope (`Tr`) the
    /// symbolic type it binds. `None` when the argument names neither.
    pub(super) fn trait_type_arg(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Option<Result<Ty, TypeError>> {
        use mojito_ast::ast::ParamArg;
        let trait_binder = |name: &str| {
            self.value_parameter_in_scope(name)
                .filter(|reference| {
                    reference
                        .meta()
                        .as_value()
                        .is_some_and(|ty| matches!(ty.kind(), TyKind::AnyTrait(_)))
                })
                .map(|reference| Ok(DependentType::resolve(reference)))
        };
        let expr = match arg {
            ParamArg::Named { value, .. } => return self.trait_type_arg(value),
            ParamArg::Value(expr) => expr,
        };
        if let Some(value) = self.trait_value(expr) {
            return Some(value.map(|value| TyKind::Trait(value.traits).into()));
        }
        if let ExprKind::Identifier(name) = &expr.kind {
            return trait_binder(name);
        }
        // The enclosing struct's own trait-valued binder (`Self.Trait`).
        let param = expr.self_member()?;
        self.self_param_value(param).and_then(|value| match value {
            CtValue::Trait(traits) => Some(Ok(TyKind::Trait(traits).into())),
            CtValue::Expr(expr) if value.is_trait_valued() => {
                Some(Ok(DependentType::resolve(expr)))
            }
            _ => None,
        })
    }

    /// The type a compiler-known name applies to `args` (`Never`, `SIMD`,
    /// `Scalar`, the compiler-private storage types, a type parameter in
    /// scope), beside its bracket attributes; `None` for any other name.
    fn builtin_applied_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<(Ty, DeclaredAttributes)>, TypeError> {
        if name == "__mlir_type" {
            return self
                .mlir_applied_type(args)
                .map(|ty| Some((ty, DeclaredAttributes::default())));
        }
        if name == "Some"
            && let [mojito_ast::ast::ParamArg::Value(argument)] = args
            && let Some(existential) = self.existential_type(argument)?
        {
            return Ok(Some((existential, DeclaredAttributes::default())));
        }
        if name == "Never" && args.is_empty() {
            return Ok(Some((Ty::never(), DeclaredAttributes::default())));
        }
        if args.is_empty()
            && let Some(parameter) = self.lookup_tparam(name)
        {
            return Ok(Some((parameter, DeclaredAttributes::default())));
        }
        // `Bool` is still the compiler's own spelling; `Int`, `UInt`, and
        // `Float64` resolve through `std.simd`'s comptime aliases.
        if args.is_empty() && name == "Bool" {
            return Ok(Some((Ty::bool(), DeclaredAttributes::default())));
        }
        if name == "SIMD" && !self.simd_declared_application(args) {
            return self.simd_type(args).map(Some);
        }
        if name == "$pack" {
            let (elements, attributes) = self.tuple_element_types(args)?;
            let attributes = DeclaredAttributes::of_builtin(name, PACK_PARAMS, args, attributes)?;
            return Ok(Some((TyKind::RuntimePack(elements).into(), attributes)));
        }
        if name == "__RuntimeTuple" {
            let (elements, attributes) = self.tuple_element_types(args)?;
            let attributes = DeclaredAttributes::of_builtin(name, PACK_PARAMS, args, attributes)?;
            return Ok(Some((TyKind::Tuple(elements).into(), attributes)));
        }
        // Compiler-private tagged-union storage (the self-hosted
        // `Variant`'s field): the intrinsic union type, reachable only
        // from bundled standard-library sources.
        if name == mojito_types::types::VARIANT_STORAGE_TYPE_NAME {
            if !self.bundled_stdlib_declaration {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is compiler-private storage; use Variant from std.utils"
                )));
            }
            return self.variant_type(args).map(Some);
        }
        // The case-indexed storage of an `enum`: the intrinsic union over
        // the case payload types, which may repeat and may be none at all.
        if name == mojito_types::types::ENUM_STORAGE_TYPE_NAME {
            if !self.enum_storage_allowed() {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is the storage of an 'enum'; declare one instead"
                )));
            }
            return self.enum_storage_type(args).map(Some);
        }
        // Compiler-private inline uninit storage (`MaybeUninit`'s
        // field), reachable only from the bundled crossing module.
        if name == mojito_types::types::UNINIT_STORAGE_TYPE_NAME {
            if !self.bundled_stdlib_declaration {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is compiler-private storage; use MaybeUninit from std.memory"
                )));
            }
            if args.len() != 1 {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let (element, attributes) = self.uninit_storage_element(args)?;
            return Ok(Some((
                TyKind::Struct(name.to_string(), vec![TyArg::Ty(element)]).into(),
                attributes,
            )));
        }
        if name == "_" && args.is_empty() {
            return Ok(Some((Ty::infer(), DeclaredAttributes::default())));
        }
        if name == "DType" && args.is_empty() && !self.structs.contains_key(name) {
            return Ok(Some((Ty::dtype(), DeclaredAttributes::default())));
        }
        Ok(None)
    }

    /// The anonymous type binder upstream's `Some[X]` binds its trait
    /// parameter to: `X` evaluated to its trait value (a trait, a trait
    /// constant, a `&` composition) bounds it by those traits, and a
    /// function type bounds it as a callable (`Some[def(var Int)]`). A
    /// trait-valued binder (`Some[Tr]` over `Tr: type_of(Writable)`,
    /// `Some[Self.Tr]`) bounds it symbolically: by the binder's declared
    /// trait set, until a substitution binds the trait it receives.
    /// `None` when `X` is none of these.
    fn existential_type(&self, argument: &Expr) -> Result<Option<Ty>, TypeError> {
        if let Some((trait_param, declared)) = self.trait_valued_argument(argument) {
            return Ok(Some(
                TyKind::Param {
                    binder: trait_param_existential_binder(&trait_param),
                    bounds: declared,
                    callable_bound: None,
                    trait_param: Some(Box::new(trait_param)),
                }
                .into(),
            ));
        }
        let (spelling, bounds, callable_bound) =
            if let ExprKind::FunctionType(callable) = &argument.kind {
                let bound = self.function_type(callable)?;
                (bound.to_string(), Vec::new(), Some(bound))
            } else if let ExprKind::Identifier(trait_name) = &argument.kind
                && (COMPILER_TRAITS.contains(&trait_name.as_str())
                    || self.traits.contains_key(trait_name)
                    || self.predeclared_traits.contains(trait_name))
            {
                (trait_name.clone(), vec![trait_name.clone()], None)
            } else if let Some(value) = self.trait_value(argument) {
                let traits = value?.traits;
                (traits.join(" & "), traits, None)
            } else {
                return Ok(None);
            };
        Ok(Some(
            TyKind::Param {
                binder: synthetic_binder(&format!("Some[{spelling}]")),
                bounds,
                callable_bound,
                trait_param: None,
            }
            .into(),
        ))
    }

    /// The trait-valued binder `argument` builds to (`Tr`, `Self.Tr`), with
    /// its declared trait set, as the one compile-time builder reads it.
    fn trait_valued_argument(&self, argument: &Expr) -> Option<(ParamRef, Vec<String>)> {
        let built = self.compile_dependent_ct_expr(argument).ok()?;
        let Some(TyKind::AnyTrait(declared)) = built.meta().as_value().map(Ty::kind) else {
            return None;
        };
        Some((built.as_decl_ref()?.clone(), declared.clone()))
    }

    /// Resolve a struct application's explicit compile-time arguments over
    /// the struct's whole declared list, in one solve, as upstream binds a
    /// type's parameter list ([`Self::struct_bracket`]). The resolved
    /// arguments hold each origin argument at its binder's position:
    /// origin arguments are part of the struct's checked identity, and a
    /// slot the application leaves to inference is `Origin::Unbound`.
    /// `decls` binds a name no struct declares.
    pub(super) fn resolve_struct_use_args(
        &self,
        name: &str,
        decls: &[ParamDecl],
        args: &[mojito_ast::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<BoundParameters, TypeError> {
        let Some(info) = self.structs.get(name) else {
            return self.resolve_use_params(name, decls, args, patterns, actuals);
        };
        let bracket = self.struct_bracket(name, args)?;
        let BoundParameters {
            subst,
            arguments,
            attributes,
            ..
        } = self.resolve_explicit_use_params(
            name,
            &info.params,
            (args, &bracket.explicit),
            patterns,
            actuals,
        )?;
        Ok(BoundParameters {
            subst,
            arguments: self.place_struct_bracket(info, arguments, None, &bracket),
            attributes,
            origins: Vec::new(),
        })
    }

    /// The explicit arguments of struct `name`'s application `args`, bound
    /// over its whole declared list as a type's bracket
    /// ([`BracketUse::Type`]): each origin and `mut=` argument in its
    /// binder's slot, and each type argument resolved once, for
    /// [`Self::resolve_explicit_use_params`] to read back. A type
    /// application must bind its origin binders
    /// ([`Self::check_struct_application`]).
    pub(super) fn struct_bracket(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<StructBracket, TypeError> {
        let params = self.struct_params(name);
        // Only the outermost application of a collected storage annotation
        // reports its demands; nested applications resolve uncollected.
        let collecting = self.storage_origin_demands.take();
        let strictness = self.application_strictness();
        let explicit = self.explicit_bracket_args(name, params, args, BracketUse::Type(strictness));
        if let Some(mut demands) = collecting {
            if let Ok(explicit) = &explicit {
                demands.extend(
                    explicit
                        .origins()
                        .iter()
                        .filter(|bound| bound.origin != mojito_types::origin::Origin::Unbound)
                        .map(|bound| (params[bound.slot].name().to_string(), bound.origin.clone())),
                );
            }
            self.storage_origin_demands.replace(Some(demands));
        }
        let explicit = explicit?;
        let bound: Vec<BoundOrigin> = explicit
            .origins()
            .iter()
            .filter(|bound| bound.origin != mojito_types::origin::Origin::Unbound)
            .cloned()
            .collect();
        let origins = super::struct_origin_decls(params)
            .map(|(id, _)| {
                bound
                    .iter()
                    .find(|origin| origin.id == id)
                    .map_or(mojito_types::origin::Origin::Unbound, |origin| {
                        origin.origin.clone()
                    })
            })
            .collect();
        let mutabilities = explicit
            .mutabilities()
            .iter()
            .map(|bound| (bound.id.clone(), bound.value.clone()))
            .collect();
        Ok(StructBracket {
            explicit,
            origins,
            bound,
            mutabilities,
        })
    }

    /// The instance of the struct `info` an application bound as `bracket`
    /// names, its argument binders taking `view` and its origin binders the
    /// origins of `instance`, when there is one, else the application's
    /// ([`Self::place_struct_origins`]).
    pub(super) fn place_struct_bracket(
        &self,
        info: &super::StructInfo,
        view: impl IntoIterator<Item = TyArg>,
        instance: Option<&Ty>,
        bracket: &StructBracket,
    ) -> Vec<TyArg> {
        self.place_struct_origins(
            &info.params,
            view,
            super::StructInfo::instance_origins_or(instance, &bracket.origins),
            &bracket.mutabilities,
        )
    }

    /// Check that a type application of `name` binds its declared list
    /// `params`, as upstream's type expression must ("use '[]' to bind
    /// missing parameters"): unless the position leaves parameters to
    /// inference ([`Self::application_strictness`]), a nonempty bracket
    /// binds each explicit origin binder, and a concrete position spells
    /// none as `_`. A pack the bracket's positional arguments fill leaves
    /// the origin binders after it to keywords, so a bracket naming none of
    /// them binds the pack alone.
    pub(super) fn check_struct_application(
        name: &str,
        params: &[ParamDecl],
        bound: &[Vec<(usize, &mojito_ast::ast::ParamArg)>],
        strictness: super::StorageStrictness,
    ) -> Result<(), TypeError> {
        use mojito_ast::call::CompileTimeParam as _;
        if strictness == super::StorageStrictness::Off || bound.iter().all(Vec::is_empty) {
            return Ok(());
        }
        let explicit_origins = || {
            params
                .iter()
                .zip(bound)
                .filter(|(decl, _)| decl.is_origin() && !decl.infer_only())
        };
        let packed = params
            .iter()
            .any(|decl| !decl.infer_only() && decl.variadic())
            && explicit_origins().all(|(_, arguments)| arguments.is_empty());
        if packed {
            return Ok(());
        }
        for (decl, arguments) in explicit_origins() {
            let Some(&(_, argument)) = arguments.first() else {
                return Err(TypeError::CannotInferParam {
                    name: name.to_string(),
                    param: decl.name().to_string(),
                });
            };
            if strictness == super::StorageStrictness::Full
                && mojito_ast::call::unbound_marker(argument).is_some()
            {
                return Err(TypeError::NotConcrete(format!("{name}[_]")));
            }
        }
        Ok(())
    }

    /// The checked parameter list of struct `name`, empty for a struct not
    /// registered.
    fn struct_params(&self, name: &str) -> &[ParamDecl] {
        self.structs
            .get(name)
            .map_or(&[], |info| info.params.as_slice())
    }

    /// Resolve a return annotation: explicit origin slots must be applied
    /// there (a bare origin-slotted generic or a placeholder is not concrete,
    /// pin-attested), while an applied origin expression naming places only
    /// the body can resolve (`origin_of(self.entries)`) is accepted
    /// syntactically and erased.
    pub(super) fn resolve_return_annotation(&self, annotation: &Expr) -> Result<Ty, TypeError> {
        let saved = self.signature_origin_leniency.replace(true);
        let result = self.resolve_storage_annotation(annotation, super::StorageStrictness::Full);
        self.signature_origin_leniency.set(saved);
        result
    }

    /// Resolve the return annotation of a compiler-generated (`$`-mangled)
    /// specialization: it rebinds an already-checked annotation whose origin
    /// identity is legitimately erased, so signature concreteness does not
    /// apply, but an origin expression naming a parameter place
    /// (`Span[T, origin_of(xs)]` cloned as `view$Int`) is still accepted
    /// syntactically and erased rather than judged against the erased binder.
    pub(super) fn resolve_generated_return_annotation(
        &self,
        annotation: &Expr,
    ) -> Result<Ty, TypeError> {
        let saved = self.signature_origin_leniency.replace(true);
        let generated = self.generated_declaration.replace(true);
        let result = self.resolve_storage_annotation(annotation, super::StorageStrictness::Off);
        self.generated_declaration.set(generated);
        self.signature_origin_leniency.set(saved);
        result
    }

    /// [`Self::resolve_storage_annotation`] that also returns the explicit
    /// origin arguments the annotation's outermost struct application
    /// supplied (`var w: StringSpan[ImmStaticOrigin] = s`, `var v: Span[Int,
    /// origin_of(xs)] = xs`), erased from the resolved type, so the binding
    /// site can judge its initializer against them.
    pub(super) fn resolve_storage_annotation_with_origins(
        &self,
        annotation: &Expr,
        strictness: super::StorageStrictness,
    ) -> Result<(Ty, Vec<(String, mojito_types::origin::Origin)>), TypeError> {
        let saved = self.storage_origin_demands.replace(Some(Vec::new()));
        let result = self.resolve_storage_annotation(annotation, strictness);
        let demands = self
            .storage_origin_demands
            .replace(saved)
            .unwrap_or_default();
        result.map(|ty| (ty, demands))
    }

    /// Resolve a type annotation in a storage position (a struct field or a
    /// local `var` type): explicit origin slots must be bound there, because
    /// storage has no constructor value argument to infer them from.
    pub(super) fn resolve_storage_annotation(
        &self,
        annotation: &Expr,
        strictness: super::StorageStrictness,
    ) -> Result<Ty, TypeError> {
        let saved = self.strict_storage_annotation.replace(strictness);
        let result = self.emit_expr_type(annotation);
        self.strict_storage_annotation.set(saved);
        result
    }

    /// The `comptime` binding annotation `annotation` as shown when it
    /// spells a parametric struct bare (`comptime x: W = W[1]()`), whose
    /// value cannot be constructed, as at the pin.
    pub(super) fn unconstructible_comptime_annotation(&self, annotation: &Expr) -> Option<String> {
        let ExprKind::Identifier(name) = &annotation.kind else {
            return None;
        };
        let info = self.structs.get(name)?;
        unbound_struct_spelling(name, &info.params)
    }

    /// Accept one resolved origin argument against its declared slot: a slot
    /// declared `Origin[mut=True]` rejects a provably immutable argument, and
    /// a value-shaped accepted argument records `origin` as its attribute —
    /// it is compile-time data on the construction, never a runtime value. A
    /// storage annotation is never lowered, so its arguments record nothing.
    /// Answers the bracket attribute the argument bound to.
    /// How strictly the struct application being resolved must bind its
    /// parameters. A struct applied as a type argument of another
    /// application (`List[RefBox]`, `List[W]`) must name a concrete type in
    /// every position, as at the pin; only the outermost application may
    /// leave parameters to inference. A compiler-generated declaration
    /// rebinds already-checked spellings and keeps its context's leniency.
    pub(super) fn application_strictness(&self) -> super::StorageStrictness {
        let generated = self.generated_declaration.get()
            || self.self_ty.as_ref().is_some_and(|ty| {
                ty.kind()
                    .as_non_pointer_struct()
                    .is_some_and(|(name, _)| name.contains('$'))
            });
        if self.annotation_depth.get() > 1 && !generated {
            super::StorageStrictness::Full
        } else {
            self.strict_storage_annotation.get()
        }
    }

    /// Reject `argument`, an origin argument of binder `decl` of `name`,
    /// when it is not the binder's representation: the raw `!lit.origin`
    /// an `X._mlir_origin` reads and the `Origin` value `X` are distinct
    /// types, and each binder takes only its own.
    pub(super) fn check_origin_argument_representation(
        &self,
        name: &str,
        decl: &ParamDecl,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<(), TypeError> {
        use mojito_types::origin::Mutability;
        let mut value = argument;
        while let mojito_ast::ast::ParamArg::Named { value: inner, .. } = value {
            value = inner;
        }
        let ParamDecl::Origin {
            mutability: declared,
            lit,
            ..
        } = decl
        else {
            return Ok(());
        };
        if origin_placeholder(value) || self.origin_argument_is_raw(value) == *lit {
            return Ok(());
        }
        let fixed = match self
            .resolve_origin_param_arg(value)
            .ok()
            .and_then(|(_, own)| own)
        {
            Some(Mutability::Mutable) => Some(true),
            Some(Mutability::Immutable) => Some(false),
            _ => None,
        };
        let expected = match declared {
            OriginMutability::Fixed(fixed) => super::origin_value_type(Some(*fixed), *lit),
            other => other.binder_type(*lit),
        };
        Err(TypeError::TypeMismatch {
            expected,
            found: super::origin_value_type(fixed, !*lit),
            context: format!("value passed to '{}' of '{name}'", decl.name()),
        })
    }

    /// Whether origin argument `value` is a raw `!lit.origin` rather than
    /// an `Origin` value: a raw read ([`super::reads_raw_origin`]), or a
    /// use of an in-scope origin binder (`r`, `Self.r`) declared raw.
    fn origin_argument_is_raw(&self, value: &mojito_ast::ast::ParamArg) -> bool {
        use mojito_ast::ast::ParamArg;
        let binder = match value {
            ParamArg::Value(expr) => super::origin_binder_name(expr),
            ParamArg::Named { .. } => None,
        };
        super::reads_raw_origin(value)
            || binder
                .and_then(|name| self.enclosing_origin_type_param(name))
                .is_some_and(|(_, decl)| matches!(decl, ParamDecl::Origin { lit: true, .. }))
    }

    /// Resolve the type-valued sequence at the base of an indexed type
    /// projection. A source value may expose such a sequence through an
    /// associated compile-time member; its runtime value is never inspected.
    pub(super) fn dependent_type_sequence(&self, projection: &Expr) -> Result<Vec<Ty>, TypeError> {
        let ExprKind::Member {
            object,
            field: name,
        } = &projection.kind
        else {
            return Err(dependent_indexing_unsupported());
        };
        let base_ty = match &object.kind {
            ExprKind::Identifier(binding) => self
                .lookup(binding)
                .cloned()
                .map_or_else(|| self.emit_expr_type(object), Ok)?,
            _ => self.emit_expr_type(object)?,
        };

        // Public Tuple exposes its concrete element pack as `element_types`.
        // Use the checked nominal arguments, never its generated symbol text.
        if (name == "element_types" || name == "Ts")
            && let Some(elements) = tuple_elements(&base_ty)
        {
            return Ok(elements.into_iter().cloned().collect());
        }

        let Some((struct_name, arguments)) = base_ty.kind().as_non_pointer_struct() else {
            return Err(TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            });
        };
        let info = self
            .structs
            .get(struct_name)
            .ok_or_else(|| TypeError::UnknownType(struct_name.clone()))?;
        let value = info
            .associated
            .get(name)
            .ok_or_else(|| TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            })?;
        let (CtValue::Tuple(values) | CtValue::List(values)) = value else {
            return Err(TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            });
        };
        let substitution = struct_subst(&info.params, arguments);
        values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Ok(self.resolve_assoc_ty(&substitute(ty, &substitution))),
                _ => Err(TypeError::NotComptime(format!(
                    "{base_ty}.{name} contains a non-type value"
                ))),
            })
            .collect()
    }

    /// Collapse an indexed dependent type when its compile-time environment is
    /// concrete; otherwise retain the structural expression in generic
    /// metadata for later specialization.
    pub(super) fn resolve_dependent_index(
        &self,
        elements: Vec<Ty>,
        index: &ParamExpr,
        values: &DependentValues,
    ) -> Result<Ty, TypeError> {
        let context = &self.param_context;
        let bindings = &values.bindings;
        // Selecting a type needs the index's value, so it is evaluated.
        context
            .replace(index, bindings)
            .and_then(|index| context.fold(&index))
            .and_then(|index| context.select(elements, &index))
            .map(DependentType::resolve)
            .map_err(param_error)
    }

    /// Resolve dependent leaves after a generic use has supplied its value
    /// parameters. This is a typed walk: the candidate type sequence and the
    /// retained [`ParamExpr`] remain structural until the environment is concrete.
    pub(super) fn resolve_dependent_ty(
        &self,
        ty: &Ty,
        values: &DependentValues,
    ) -> Result<Ty, TypeError> {
        Ok(match ty.kind() {
            TyKind::Dependent(dependent) => match dependent.selection() {
                Some((elements, index)) => {
                    let elements = elements
                        .iter()
                        .map(|element| self.resolve_dependent_ty(element, values))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.resolve_dependent_index(elements, index, values)?
                }
                // Any other type-valued expression closes as the use binds
                // its parameters: an element of a pack that is still a
                // parameter as its pack and index bind, a generator's
                // application as its generator binds, and a conditional
                // type (`Int if n > 0 else Float64`) as its condition does.
                None if !values.is_empty() => {
                    let context = &self.param_context;
                    let bindings = &values.bindings;
                    // A pack bound to a list of types selects the indexed
                    // one, whether or not each is closed (`Tuple[T, Int]`
                    // over a binder `T`).
                    if let Some((list, index)) = dependent.pack_element()
                        && let Some(elements) =
                            list.as_decl_ref().and_then(|pack| values.list(pack))
                        && let Some(position) = context
                            .replace(index, bindings)
                            .ok()
                            .as_ref()
                            .and_then(ParamExpr::as_constant)
                            .and_then(mojito_types::param_expr::fold::integer_value)
                            .and_then(|position| position.to_i64())
                            .and_then(|position| usize::try_from(position).ok())
                        && let Some(CtValue::Type(element)) = elements.get(position)
                    {
                        return Ok((*element).clone());
                    }
                    context
                        .replace(dependent.expr(), bindings)
                        .map(DependentType::resolve)
                        .map_err(param_error)?
                }
                None => ty.clone(),
            },
            // An existential over a trait-valued binder (`Some[Tr]`) is
            // bounded by the trait the use binds it to.
            TyKind::Param {
                trait_param: Some(_),
                ..
            } if !values.is_empty() => mojito_types::types::replace_parameters(
                &self.param_context,
                ty,
                &values.bindings,
                0,
            )
            .map_err(param_error)?,
            // A symbolic lane dtype or width closes under the use's bindings
            // and re-canonicalizes: `Scalar[dt]` at `dt = DType.float64` is
            // `Float64`.
            kind if (kind.is_simd()
                || kind.string_literal_value().is_some()
                || kind.is_numeric_literal()
                || matches!(kind, TyKind::KgenSimd { .. } | TyKind::PopArray { .. })
                || kind
                    .pointer_address_space()
                    .is_some_and(|space| space.is_symbolic()))
                && !values.is_empty()
                && mojito_types::types::is_symbolic(ty) =>
            {
                let context = &self.param_context;
                let bindings = &values.bindings;
                mojito_types::types::replace_parameters(context, ty, bindings, 0)
                    .map_err(param_error)?
            }
            // A spread of a list computed from packs closes as they bind.
            struct_ty
                if let Some((name, arguments)) = struct_ty.as_non_pointer_struct()
                    && !values.is_empty()
                    && let Some(list) = mojito_types::types::list_spread_argument(arguments) =>
            {
                let context = &self.param_context;
                let bindings = &values.bindings;
                let spread = context
                    .replace(list, bindings)
                    .and_then(|list| mojito_types::types::spread_arguments(name, &list))
                    .map_err(param_error)?;
                ty.reusing(TyKind::Struct(name.clone(), spread))
            }
            struct_ty if let Some((name, arguments)) = struct_ty.as_non_pointer_struct() => ty
                .reusing(TyKind::Struct(
                    name.clone(),
                    arguments
                        .iter()
                        .map(|argument| match argument {
                            TyArg::Ty(ty) => self.resolve_dependent_ty(ty, values).map(TyArg::Ty),
                            // A residual value argument closes under the use's
                            // bindings and re-folds: `Buf[n + 1]` at `n = 3` is
                            // `Buf[4]`.
                            TyArg::Val(value @ CtValue::Expr(_)) if !values.is_empty() => {
                                let context = &self.param_context;
                                let bindings = &values.bindings;
                                mojito_types::types::replace_value_parameters(
                                    context, value, bindings,
                                )
                                .map(TyArg::Val)
                                .map_err(param_error)
                            }
                            TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                            // A cast's residual mutability closes alike:
                            // `#lit.origin.mutcast<o> : !lit.origin<dest_mut>`
                            // at `dest_mut = False`.
                            TyArg::Origin(origin) if !values.is_empty() => origin
                                .try_map_cast_mutability(&mut |mutability| {
                                    self.param_context.replace(mutability, &values.bindings)
                                })
                                .map(TyArg::Origin)
                                .map_err(param_error),
                            TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                        })
                        .collect::<Result<Vec<_>, TypeError>>()?,
                )),
            TyKind::ComptimeList(element) => {
                TyKind::ComptimeList(self.resolve_dependent_ty(element, values)?).into()
            }
            TyKind::ParamList(element) => {
                TyKind::ParamList(self.resolve_dependent_ty(element, values)?).into()
            }
            TyKind::Tuple(elements) => TyKind::Tuple(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, values))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            TyKind::RuntimePack(elements) => TyKind::RuntimePack(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, values))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            TyKind::VariadicPack(element) => {
                TyKind::VariadicPack(self.resolve_dependent_ty(element, values)?).into()
            }
            TyKind::Variant(alternatives) => TyKind::Variant(
                alternatives
                    .iter()
                    .map(|alternative| self.resolve_dependent_ty(alternative, values))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            kind if let Some(reference) = kind.pointer_lit_ref() => {
                Ty::pointer_from_lit_ref(&mojito_types::origin::RefTy {
                    referent: self.resolve_dependent_ty(&reference.referent, values)?,
                    ..reference
                })
            }
            // A reference's space closes at the bound `AddressSpace` binder.
            TyKind::Ref(reference) => TyKind::Ref(mojito_types::origin::RefTy {
                referent: self.resolve_dependent_ty(&reference.referent, values)?,
                origin: reference.origin.clone(),
                mutability: reference.mutability,
                address_space: reference
                    .address_space
                    .map_or_keep(|space| self.param_context.replace(space, &values.bindings)),
            })
            .into(),
            // A nested generic callable owns its own value-binder scope. Leave
            // that scope structural here; its own invocation resolves it.
            TyKind::GenericFunc(_) => ty.clone(),
            TyKind::Func(func) => {
                let FuncTy {
                    environment,
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                    transfers,
                } = &**func;
                TyKind::Func(Box::new(FuncTy {
                    environment: environment.clone(),
                    params: params
                        .iter()
                        .map(|parameter| self.resolve_dependent_ty(parameter, values))
                        .collect::<Result<Vec<_>, _>>()?,
                    names: names.clone(),
                    ret: self.resolve_dependent_ty(ret, values)?,
                    required: required.clone(),
                    variadic: variadic
                        .as_ref()
                        .map(|parameter| self.resolve_dependent_ty(parameter, values))
                        .transpose()?,
                    kw_variadic: kw_variadic
                        .as_ref()
                        .map(|parameter| self.resolve_dependent_ty(parameter, values))
                        .transpose()?,
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    error: error
                        .as_ref()
                        .map(|error| self.resolve_dependent_ty(error, values))
                        .transpose()?,
                    conventions: conventions.clone(),
                    ref_params: ref_params.clone(),
                    ref_return: ref_return.clone(),
                    transfers: transfers.clone(),
                }))
                .into()
            }
            TyKind::Overload(candidates) => TyKind::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_dependent_ty(candidate, values))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            _ => ty.clone(),
        })
    }

    pub(super) fn associated_type_for_self(&self, name: &str) -> Result<Ty, TypeError> {
        if let Some(reqs) = self.trait_self_comptime.last()
            && let Some(req) = reqs.get(name)
        {
            return match req {
                CtMemberReq::Type { .. } => Ok(TyKind::Assoc {
                    base: Ty::self_type(),
                    name: name.to_string(),
                    args: Vec::new(),
                }
                .into()),
                CtMemberReq::Value(_) => Err(TypeError::NoSuchAssociatedType {
                    object_type: "Self".to_string(),
                    member: name.to_string(),
                }),
            };
        }
        let Some(self_ty) = &self.self_ty else {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        };
        if let Some((sname, _)) = self_ty.kind().as_non_pointer_struct()
            && !self.structs.contains_key(sname)
        {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        }
        self.associated_type_from_base(self_ty, name, &[])
    }

    pub(super) fn associated_type_from_base(
        &self,
        base: &Ty,
        name: &str,
        args: &[TyArg],
    ) -> Result<Ty, TypeError> {
        match base.kind() {
            struct_ty if let Some((sname, targs)) = struct_ty.as_non_pointer_struct() => {
                self.declared_associated_type(base, sname, targs, name, args)
            }
            TyKind::Param { .. } => {
                let bounds = self.refined_bound(base).unwrap_or_default();
                if self.lookup_trait_assoc_type(&bounds, name).is_some() {
                    Ok(TyKind::Assoc {
                        base: base.clone(),
                        name: name.to_string(),
                        args: args.to_vec(),
                    }
                    .into())
                } else {
                    Err(TypeError::NoSuchAssociatedType {
                        object_type: base.to_string(),
                        member: name.to_string(),
                    })
                }
            }
            TyKind::Assoc { .. } => Ok(TyKind::Assoc {
                base: base.clone(),
                name: name.to_string(),
                args: args.to_vec(),
            }
            .into()),
            // A vector's `!kgen.simd` storage type, its `_mlir_value`'s.
            kind if kind.is_simd() && name == "_mlir_type" && args.is_empty() => {
                Ok(mojito_types::types::simd_storage(base).unwrap_or_else(|| base.clone()))
            }
            // A member alias `SIMD` or `Pointer` declares, over the type's
            // parameters, or one a literal's struct declares
            // (`StringLiteral._from`); a pointer's type parameter too
            // (`type_of(p).T`).
            kind if (kind.is_simd()
                || kind.is_pointer()
                || kind.string_literal_value().is_some()
                || kind.is_numeric_literal())
                && let Some((sname, targs)) = self.nominal_struct(base)
                && self.structs.get(sname).is_some_and(|info| {
                    info.associated.contains_key(name)
                        || info.parameterized_associated.contains_key(name)
                        || (kind.is_pointer()
                            && info.decls().iter().any(|decl| decl.name() == name))
                }) =>
            {
                self.declared_associated_type(base, sname, &targs, name, args)
            }
            _ => Err(TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            }),
        }
    }

    /// The associated member `name` of the declared struct `sname` applied to
    /// `targs`, as `base` spells it.
    fn declared_associated_type(
        &self,
        base: &Ty,
        sname: &str,
        targs: &[TyArg],
        name: &str,
        args: &[TyArg],
    ) -> Result<Ty, TypeError> {
        let info = self
            .structs
            .get(sname)
            .ok_or_else(|| TypeError::UnknownType(sname.to_string()))?;
        // A parameterized associated member instantiated by a conforming
        // struct: substitute the application's arguments into its template.
        if let Some(member) = info.parameterized_associated.get(name) {
            let member = member.clone();
            let decls = info.decls().into_owned();
            let targs = info.view_arguments(targs).into_owned();
            return self.resolve_parameterized_member(base, name, &member, &decls, &targs, args);
        }
        // A type parameter of the struct is a member of its application
        // (`List[Int].T`).
        if args.is_empty()
            && let Some(TyArg::Ty(argument)) = info
                .decls()
                .iter()
                .position(|decl| decl.name() == name)
                .and_then(|index| {
                    ParamDecl::source_argument(&info.decls(), &info.view_arguments(targs), index)
                        .cloned()
                })
        {
            return Ok(argument);
        }
        let value = info
            .associated
            .get(name)
            .ok_or_else(|| TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            })?;
        if let Some(constraints) = info.associated_constraints.get(name) {
            let view = info.view_arguments(targs);
            let environment = ConstraintEnvironment::declared(&info.decls(), &view);
            if !environment.arguments().any(tyarg_is_symbolic) {
                for constraint in constraints {
                    self.validate_constraint_in_environment(
                        &format!("{base}.{name}"),
                        constraint,
                        &environment,
                    )?;
                }
            }
        }
        let CtValue::Type(ty) = value else {
            return Err(TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            });
        };
        // The member's type names the struct's own parameters of
        // every kind; the instance binds them all at once.
        Ok(self.resolve_assoc_ty(&self.substitute_at_instance(ty, info, targs)))
    }

    /// Expand a generic comptime alias application (`Alias[args]`) into the
    /// aliased type. `resolve_use_params` validates arity, bounds, defaults,
    /// and the alias's declaration constraints — including repeated `where`
    /// clauses, each retaining its own message — through the same contract a
    /// struct application uses; the resulting arguments then substitute into
    /// the symbolic template.
    fn resolve_comptime_alias(
        &self,
        name: &str,
        alias: &ComptimeAlias,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        if let Some(generator) = alias.generator.as_deref()
            && (!args.is_empty() || matches!(alias.body, AliasBody::Generator))
        {
            return self.applied_type_at_depth(generator, args);
        }
        if let Some((head, applied)) = self.alias_infer_only_application(alias, args) {
            return self.applied_type_at_depth(head, &applied);
        }
        let template = match &alias.body {
            AliasBody::Type(template) => template,
            AliasBody::Predicate(_) => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is a Bool-valued comptime alias, not a type"
                )));
            }
            AliasBody::Value(_) | AliasBody::OriginValue => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is a value comptime alias, not a type"
                )));
            }
            AliasBody::Generator => {
                return Err(TypeError::InvariantViolation(format!(
                    "generator alias '{name}' names no struct"
                )));
            }
        };
        let BoundParameters {
            arguments: tyargs,
            attributes,
            origins,
            ..
        } = self.resolve_use_params(name, &alias.decls, args, &[], &[])?;
        let mut types = HashMap::new();
        let mut values = HashMap::new();
        let mut ids = mojito_types::param_expr::ParamBindings::new();
        for (decl, argument) in ParamDecl::aligned_pairs(&alias.decls, &tyargs) {
            if let Some(list) = self.forwarded_type_pack(decl, argument) {
                ids.bind(decl.id().clone(), list);
            }
            match (decl, argument) {
                (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => {
                    types.insert(id.clone(), ty.clone());
                }
                // A type pack binds its element list: `Ts.values` reads it.
                (ParamDecl::Type { id, .. } | ParamDecl::Value { id, .. }, TyArg::Val(value)) => {
                    if let Some(bound) = self.bound_parameter_value(value) {
                        ids.bind(id.clone(), bound);
                    }
                    if let ParamDecl::Value { name, .. } = decl {
                        values.insert(name.clone(), value.clone());
                    }
                }
                _ => {}
            }
        }
        // A spread of a type pack (`Tuple[*Ts]`) expands to its elements.
        let template = super::generics::expand_solved_packs(template, &alias.decls, &tyargs);
        let bindings = AssocBindings {
            types,
            values,
            ids,
            origins: HashMap::new(),
        };
        // The origin arguments bind the alias's origin binders wherever the
        // template names them, a pointer's provenance included, at the
        // mutability each bound origin gives a parametric binder.
        let tails = origins
            .into_iter()
            .map(|bound| (bound.id, bound.origin))
            .collect();
        let substituted = super::generics::bind_struct_origins(
            &substitute_assoc(&template, &bindings),
            &tails,
            &|_, origin| self.bound_origin_mutability(origin),
        );
        let substituted = bind_alias_origin_placeholders(&substituted, &alias.decls, args);
        // The symbolic template keeps canonical `Tuple[T, ...]`; a substituted
        // application must re-select the executable nominal implementation so
        // discovery can materialize it.
        Ok((self.resolve_assoc_ty(&substituted), attributes))
    }

    /// The struct application an alias declaring no parameters takes
    /// `args` into, when each is a keyword naming an infer-only parameter
    /// the alias's struct application leaves unbound: upstream's alias
    /// keeps such a parameter applicable (`ImmOrigin[_mlir_origin=r]`
    /// over `comptime ImmOrigin = Origin[mut=False]`). `None` for any other
    /// application.
    fn alias_infer_only_application<'a>(
        &self,
        alias: &'a ComptimeAlias,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Option<(&'a str, Vec<mojito_ast::ast::ParamArg>)> {
        use mojito_ast::ast::ParamArg;
        use mojito_ast::call::CompileTimeParam as _;
        if !alias.decls.is_empty() || args.is_empty() {
            return None;
        }
        let (head, written) = alias.value.bracket_application()?;
        let params = &self.structs.get(head)?.params;
        let bound = |keyword: &str| {
            written
                .iter()
                .any(|argument| matches!(argument, ParamArg::Named { name, .. } if name == keyword))
        };
        args.iter()
            .all(|argument| {
                matches!(argument, ParamArg::Named { name: keyword, .. }
                    if !bound(keyword)
                        && params
                            .iter()
                            .any(|decl| decl.infer_only() && decl.name() == keyword))
            })
            .then(|| (head, written.iter().chain(args).cloned().collect()))
    }

    /// Concretely resolve a parameterized associated-type application on a
    /// conforming struct. Binds the struct's own parameters (from its type
    /// arguments) and the member's parameters (from the application arguments)
    /// into the member's symbolic template. When the arguments were not carried —
    /// an `origin_of(self)` dropped at the abstract trait signature — the result
    /// stays symbolic rather than resolving with missing bindings.
    fn resolve_parameterized_member(
        &self,
        base: &Ty,
        name: &str,
        member: &ParameterizedMember,
        struct_decls: &[ParamDecl],
        struct_targs: &[TyArg],
        args: &[TyArg],
    ) -> Result<Ty, TypeError> {
        // The arguments are the explicit parameters' (an abstract trait
        // signature's application), or every parameter's when a bracket list
        // bound them all ([`Self::member_application`]).
        let every = args.len() == member.params.len();
        let supplies = |param: &mojito_ast::ast::TypeParam| every || !param.infer_only;
        let explicit = member.params.iter().filter(|p| !p.infer_only).count();
        let (decls, declared, template) = match &member.template {
            MemberTemplate::Type {
                decls,
                declared,
                template,
            } => (decls, declared, template),
            MemberTemplate::Value { .. } => {
                return Err(TypeError::TypeMismatch {
                    expected: "a type".to_string(),
                    found: "a value".to_string(),
                    context: format!("'{base}.{name}' is a compile-time value member"),
                });
            }
        };
        if !every && args.len() != explicit {
            return Ok(TyKind::Assoc {
                base: base.clone(),
                name: name.to_string(),
                args: args.to_vec(),
            }
            .into());
        }
        // The struct's own type and value parameters concretize `Self.T` / `Self.n`
        // references in the template, and its type pack binds by identity to
        // the element list the instance closed it at (`Self.Ts[id]`).
        let mut types = struct_subst(struct_decls, struct_targs);
        let mut values = HashMap::new();
        for (index, decl) in struct_decls.iter().enumerate() {
            if let (ParamDecl::Value { name, .. }, Some(TyArg::Val(value))) = (
                decl,
                ParamDecl::source_argument(struct_decls, struct_targs, index),
            ) {
                values.insert(name.clone(), value.clone());
            }
        }
        let mut ids = mojito_types::param_expr::ParamBindings::new();
        if let Some((pack, elements)) =
            super::generics::bound_pack_elements(struct_decls, struct_targs)
            && let Some(decl) = struct_decls
                .iter()
                .find(|decl| !decl.is_type_pack() && decl.spelling() == pack)
            && let Ok(list) = self.param_context.type_list(elements)
        {
            ids.bind(decl.id().clone(), list);
        }
        // The member's own explicit parameters concretize from the application:
        // a value binder by its identity, converted at its declared type as a
        // parameter binding converts (`0` at `idx: Int` is `Int(0)`, and an
        // integer at `idx: SIMDLength` is upstream's implicit
        // `SIMDLength.__init__(Int)` applied to it). Origin
        // parameters are keyed by the id assigned while the template was
        // lowered (their position in `enclosing_type_params`).
        let mut origins = HashMap::new();
        let mut supplied = args.iter();
        for (index, param) in member.params.iter().enumerate() {
            if !supplies(param) {
                continue;
            }
            let Some(arg) = supplied.next() else { break };
            match arg {
                TyArg::Ty(ty) => {
                    types.insert(
                        member_binder(&member.binder_owner, index, &param.name).id,
                        ty.clone(),
                    );
                }
                TyArg::Val(value) => {
                    let decl = declared.iter().find(|decl| decl.name() == param.name);
                    let value = match decl {
                        Some(ParamDecl::Value { ty, .. }) if is_simd_length(ty) => {
                            self.simd_length_argument(value, ty)?
                        }
                        Some(ParamDecl::Value { ty, .. }) => value
                            .clone()
                            .materialize_as(ty)
                            .unwrap_or_else(|| value.clone()),
                        _ => value.clone(),
                    };
                    if let Some(decl) = decl
                        && let Some(bound) = self.bound_parameter_value(&value)
                    {
                        // A pack's arguments are its list binder's.
                        let id = ParamDecl::pack_list(declared, decl).unwrap_or(decl).id();
                        ids.bind(id.clone(), bound);
                    }
                    values.insert(param.name.clone(), value);
                }
                TyArg::Origin(origin) => {
                    origins.insert(
                        mojito_types::origin::OriginParamId((member.param_base + index) as u32),
                        origin.clone(),
                    );
                }
            }
        }
        self.validate_member_availability(
            &format!("{base}.{name}"),
            &member.availability,
            struct_decls,
            struct_targs,
            decls,
            member
                .params
                .iter()
                .filter(|parameter| supplies(parameter))
                .map(|parameter| parameter.name.as_str())
                .zip(args),
        )?;
        // A spread of the member's own type pack (`Tuple[*Ts]`) expands to
        // its elements.
        let (packs, elements): (Vec<ParamDecl>, Vec<TyArg>) = member
            .params
            .iter()
            .filter(|parameter| supplies(parameter))
            .zip(args)
            .filter_map(|(parameter, argument)| {
                decls
                    .iter()
                    .find(|decl| decl.name() == parameter.name)
                    .map(|decl| ParamDecl::pack_list(decls, decl).unwrap_or(decl))
                    .map(|decl| (decl.clone(), argument.clone()))
            })
            .unzip();
        let template = super::generics::expand_solved_packs(template, &packs, &elements);
        // Each of the member's origin `mut=` binders takes the mutability
        // its bound origin decides, as upstream infers it with the origin;
        // one nothing decides yet is deferred as the origin is.
        let mut open = Vec::new();
        for decl in ParamDecl::mutability_binders(declared) {
            let governed = declared.iter().find_map(|candidate| match candidate {
                ParamDecl::Origin {
                    origin,
                    mutability: mojito_types::types::OriginMutability::Binder(binder),
                    ..
                } if binder.id == *decl.id() => origins.get(origin),
                _ => None,
            });
            let read = governed
                .map(|origin| self.origin_mutability_slot(decl, origin, None))
                .and_then(|value| match value {
                    CtValue::Deferred(_) => None,
                    CtValue::Expr(read) => Some(read),
                    value => self.param_context.constant(value).ok(),
                });
            match read {
                Some(read) => ids.bind(decl.id().clone(), read),
                None if governed.is_some() => open.push(decl.id().clone()),
                None => {}
            }
        }
        let template = self.defer_mutability_reads(&template, &open);
        let bindings = AssocBindings {
            types,
            values,
            ids,
            origins: HashMap::new(),
        };
        // The origin arguments bind the member's origin binders as an
        // alias's bind its own: a pointer's provenance included, at the
        // mutability each bound origin gives a parametric binder. A
        // mutability no `Mutability` spells (a cast to a value binder,
        // `o.unsafe_mut_cast[m]()`) keeps the binder's own.
        let substituted = super::generics::bind_struct_origins(
            &substitute_assoc(&template, &bindings),
            &origins,
            &|id, origin| {
                self.bound_origin_mutability(origin)
                    .or(Some(mojito_types::origin::Mutability::Param(id)))
            },
        );
        Ok(self.resolve_assoc_ty(&substituted))
    }

    /// `value` bound at a `SIMDLength` binder: an integer converted as
    /// upstream's implicit `SIMDLength.__init__(Int)` converts it, so `1` is
    /// `SIMDLength(1)`, and any other value converted at the binder's type.
    fn simd_length_argument(&self, value: &CtValue, ty: &Ty) -> Result<CtValue, TypeError> {
        let Some(lanes) = self
            .param_context
            .constant(value.clone())
            .ok()
            .filter(|lanes| lanes.meta().is_integer())
        else {
            return Ok(value
                .clone()
                .materialize_as(ty)
                .unwrap_or_else(|| value.clone()));
        };
        let length = self.simd_length_of_lanes(&lanes).ok_or_else(|| {
            TypeError::InvariantViolation(format!("'{lanes}' does not convert to a SIMDLength"))
        })?;
        Ok(length
            .as_constant()
            .cloned()
            .unwrap_or(CtValue::Expr(length)))
    }

    /// `ty` with its member reads on concrete subjects closed, as a binder
    /// substitution leaves them: a type member (`S.Elem`) becomes the type
    /// the subject binds, and a value member (`S[Int, Bool]._types[0]`,
    /// `Box[S.K]`) the subject's value.
    pub(super) fn resolve_assoc_ty(&self, ty: &Ty) -> Ty {
        match ty.kind() {
            TyKind::Param {
                binder,
                bounds,
                callable_bound,
                trait_param,
            } => TyKind::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound
                    .as_ref()
                    .map(|bound| self.resolve_assoc_ty(bound)),
                trait_param: trait_param.clone(),
            }
            .into(),
            TyKind::Assoc { base, name, args } => {
                let base = self.resolve_assoc_ty(base);
                let args = map_tyargs(args, |t| self.resolve_assoc_ty(t));
                self.associated_type_from_base(&base, name, &args)
                    .unwrap_or_else(|_| {
                        TyKind::Assoc {
                            base,
                            name: name.clone(),
                            args,
                        }
                        .into()
                    })
            }
            struct_ty if let Some((name, args)) = struct_ty.as_non_pointer_struct() => {
                let rebuilt = ty.reusing(TyKind::Struct(
                    name.clone(),
                    map_tyargs(args, |t| self.resolve_assoc_ty(t)),
                ));
                if args
                    .iter()
                    .any(|argument| matches!(argument, TyArg::Val(CtValue::Expr(_))))
                {
                    self.answer_concrete_members(&rebuilt)
                } else {
                    rebuilt
                }
            }
            kind if kind.is_simd() || matches!(kind, TyKind::Dependent(_)) => {
                self.answer_concrete_members(ty)
            }
            TyKind::ComptimeList(elem) => TyKind::ComptimeList(self.resolve_assoc_ty(elem)).into(),
            TyKind::ParamList(elem) => TyKind::ParamList(self.resolve_assoc_ty(elem)).into(),
            TyKind::Tuple(elems) => {
                TyKind::Tuple(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect()).into()
            }
            TyKind::RuntimePack(elems) => {
                TyKind::RuntimePack(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect()).into()
            }
            TyKind::Variant(alternatives) => TyKind::Variant(
                alternatives
                    .iter()
                    .map(|ty| self.resolve_assoc_ty(ty))
                    .collect(),
            )
            .into(),
            kind if let Some(reference) = kind.pointer_lit_ref() => {
                Ty::pointer_from_lit_ref(&mojito_types::origin::RefTy {
                    referent: self.resolve_assoc_ty(&reference.referent),
                    ..reference
                })
            }
            TyKind::Func(func) => {
                let FuncTy {
                    environment,
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                    transfers,
                } = &**func;
                TyKind::Func(Box::new(FuncTy {
                    environment: environment.clone(),
                    params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                    names: names.clone(),
                    ret: self.resolve_assoc_ty(ret),
                    required: required.clone(),
                    variadic: variadic.as_ref().map(|v| self.resolve_assoc_ty(v)),
                    kw_variadic: kw_variadic.as_ref().map(|v| self.resolve_assoc_ty(v)),
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    error: error.as_ref().map(|error| self.resolve_assoc_ty(error)),
                    conventions: conventions.clone(),
                    ref_params: ref_params.clone(),
                    ref_return: ref_return.clone(),
                    transfers: transfers.clone(),
                }))
                .into()
            }
            TyKind::GenericFunc(generic) => {
                let GenericFuncTy {
                    decls,
                    func:
                        FuncTy {
                            environment,
                            params,
                            names,
                            ret,
                            required,
                            variadic,
                            kw_variadic,
                            positional_only,
                            keyword_only,
                            raises,
                            error,
                            conventions,
                            ref_params,
                            ref_return,
                            transfers,
                        },
                } = &**generic;
                TyKind::GenericFunc(Box::new(GenericFuncTy {
                    decls: decls.clone(),
                    func: FuncTy {
                        environment: environment.clone(),
                        params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                        names: names.clone(),
                        ret: self.resolve_assoc_ty(ret),
                        required: required.clone(),
                        variadic: variadic.as_ref().map(|v| self.resolve_assoc_ty(v)),
                        kw_variadic: kw_variadic.as_ref().map(|v| self.resolve_assoc_ty(v)),
                        positional_only: *positional_only,
                        keyword_only: *keyword_only,
                        raises: *raises,
                        error: error.as_ref().map(|error| self.resolve_assoc_ty(error)),
                        conventions: conventions.clone(),
                        ref_params: ref_params.clone(),
                        ref_return: ref_return.clone(),
                        transfers: transfers.clone(),
                    },
                }))
                .into()
            }
            TyKind::Overload(candidates) => TyKind::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_assoc_ty(candidate))
                    .collect(),
            )
            .into(),
            _ => ty.clone(),
        }
    }

    /// Resolve one supplied parameter argument against its declared parameter: a
    /// type parameter takes a type (bound-checked); a value parameter takes a
    /// comptime `Int` or the enclosing struct's own value parameter spelled
    /// `Self.n` (upstream's required spelling inside a struct body). A value
    /// argument of a type parameter is the type its expression denotes
    /// ([`Self::denoted_type`]).
    pub(super) fn resolve_param_arg(
        &self,
        decl: &ParamDecl,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<BoundArgument, TypeError> {
        use mojito_ast::ast::ParamArg;
        match decl {
            // An origin is inferred from a place, never bound by a bracket
            // argument until instances and calls carry their origins in
            // declared order.
            ParamDecl::Origin { name, .. } => Err(TypeError::Unsupported(format!(
                "an explicit argument for the origin parameter '{name}'"
            ))),
            // A trait is a type in KGEN, so `!kgen.type` binds one
            // (`_MLIR.KGENParamListType[Tr]`, `k[Copyable]()`).
            ParamDecl::Type { bounds, .. }
                if matches!(bounds.as_slice(), [only] if only == TYPE_OF_ALL_TYPES)
                    && let Some(ty) = self.trait_type_arg(arg) =>
            {
                Ok(TyArg::Ty(ty?).into())
            }
            ParamDecl::Type { name, bounds, .. } => {
                // A bare or parameterized type argument the parser encoded as a
                // value expression binds the type, which is its attribute.
                let ty = match arg {
                    // A pack's spread (`*Ts`) binds the pack.
                    ParamArg::Value(_) if let Some(operand) = arg.spread_operand() => {
                        match (&operand.kind, operand.self_member()) {
                            (_, Some(field)) => self.self_pack_type(&format!("*{field}"))?,
                            (ExprKind::Identifier(pack), None) => {
                                self.applied_type(&format!("*{pack}"), &[])?.0
                            }
                            _ => return Err(TypeError::UnknownType(name.clone())),
                        }
                    }
                    // A reflected field type (`reflect[Q].field_at[0].T`,
                    // or a name bound to one) is an `AnyType` value, which
                    // binds only a binder that asks nothing more of it.
                    ParamArg::Value(expression)
                        if let Some(reflected) =
                            self.reflected_type_operand(expression).transpose() =>
                    {
                        if let Some(bound) = bounds.iter().find(|bound| *bound != "AnyType") {
                            let owner = decl.binder().id.owner;
                            return Err(TypeError::BadCall {
                                func: owner.to_string(),
                                reason: format!(
                                    "'{owner}' parameter '{name}' has '{bound}' type, but value \
                                     has type 'AnyType'"
                                ),
                            });
                        }
                        reflected?
                    }
                    // Any other expression is the type it denotes, built by
                    // the one builder; once it resolves it is the call's
                    // parameter attribute, never a runtime value.
                    ParamArg::Value(expression) => {
                        self.denoted_type(expression, &format!("type parameter '{name}'"))?
                    }
                    ParamArg::Named { value, .. } => {
                        return self.resolve_param_arg(decl, value);
                    }
                };
                for bound in bounds {
                    if !self.conforms_to(&ty, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: name.clone(),
                            ty: ty.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(&ty, bound),
                        });
                    }
                }
                Ok(TyArg::Ty(ty).into())
            }
            ParamDecl::Value { ty, .. } if let TyKind::AnyTrait(bound) = ty.kind() => self
                .resolve_trait_param_arg(decl, bound, arg)
                .map(BoundArgument::attribute),
            ParamDecl::Value { ty, .. } if matches!(ty.kind(), TyKind::Generator(_)) => self
                .resolve_generator_param_arg(decl, ty, arg)
                .map(BoundArgument::attribute),
            // A leading-dot argument (`f[.bool]()`) names a member of the
            // parameter's declared type, as upstream resolves it against
            // the parameter's type.
            ParamDecl::Value { ty, .. }
                if let ParamArg::Value(expr) = arg
                    && super::inference::contextual_root_span(expr).is_some() =>
            {
                let base = super::inference::contextual_parameter_base(ty, expr)?;
                let rewritten =
                    super::inference::with_contextual_root(expr, &ExprKind::Identifier(base));
                let resolved = self.resolve_param_arg(decl, &ParamArg::Value(rewritten))?;
                // The argument's own nodes are typed against the parameter,
                // so a later read of the bracket takes the resolved base.
                let _position = self.comptime_position();
                self.infer_with_expected(expr, ty, true)?;
                Ok(resolved)
            }
            // A list display at a type that takes it, or a brace at a type
            // with no set or dictionary display, is the construction
            // upstream's parser emits for it (`T(a, b,
            // __list_literal__=None)`, `T(a, b)`): the argument is that
            // construction's value.
            ParamDecl::Value { ty, .. }
                if let ParamArg::Value(expr) = arg
                    && let Some(construction) = self.display_construction(expr, ty) =>
            {
                let resolved = self.resolve_param_arg(decl, &ParamArg::Value(construction?))?;
                let _position = self.comptime_position();
                self.infer_with_expected(expr, ty, true)?;
                Ok(resolved)
            }
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    if matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) {
                        return self
                            .resolve_callable_param_arg(decl, name, ty, expr)
                            .map(Into::into);
                    }
                    // Source validation types before the elaborator folds a
                    // value argument it alone can evaluate (a vector
                    // constructor keying `AHasher[SIMD[DType.uint64, 4](0)]`);
                    // the application stays symbolic in that argument, and
                    // the executable check sees the folded value.
                    // An argument computed from parameters still open
                    // (`self[Self.Ts.length - 1 - i]`) is that expression,
                    // and a binder in scope shadows a module constant of its
                    // name (a `comptime for i` under a module `comptime i`).
                    // A scalar or a `String` computed some other way from
                    // them (`h(L[0])`, `n > 2`, `pick(n)`) is the application
                    // of the function lifted for it.
                    let _lifting = self.lifting_position();
                    // A tuple or list display at a struct slot is the
                    // application of the literal constructor its slot
                    // selects, and so is a module display constant's raw
                    // element list, at its natural type.
                    let applied = |applied: Result<ParamExpr, TypeError>| {
                        applied.map_err(|error| match error {
                            TypeError::TypeMismatch {
                                expected, found, ..
                            } => TypeError::TypeMismatch {
                                expected,
                                found,
                                context: format!("value parameter '{name}'"),
                            },
                            error => error,
                        })
                    };
                    let display = ty
                        .kind()
                        .is_non_pointer_struct()
                        .then(|| self.display_application(expr, Some(ty)))
                        .flatten();
                    let value = match display {
                        Some(display) => {
                            return Ok(applied_display_argument(applied(display)?));
                        }
                        None => match self.eval_associated_ct(expr) {
                            Ok(value) if ty.kind().is_non_pointer_struct() => {
                                match self.aggregate_application(&value, ty) {
                                    Some(aggregate) => {
                                        return Ok(applied_display_argument(applied(aggregate)?));
                                    }
                                    None => value,
                                }
                            }
                            Ok(value) => value,
                            Err(error) => match self
                                .lifted_application(expr)
                                .filter(|expression| expression.as_constant().is_none())
                            {
                                Some(expression) => CtValue::Expr(expression),
                                None => return Err(self.unevaluated_argument(error, expr)),
                            },
                        },
                    };
                    let value = self.string_parameter_value(value, ty);
                    if let CtValue::Expr(expression) = &value {
                        self.record_compiled_expression(expr, expression);
                    }
                    // A struct over an applied module constant is its own
                    // type at the pin (`Buf[f(7)]` is not `Buf[8]`), which no
                    // instance below the waist can be named by yet.
                    if self.structs.contains_key(decl.id().owner.as_ref())
                        && let CtValue::Expr(applied) = &value
                        && applied.as_constant().is_none()
                        && applied.require_constant().is_ok()
                    {
                        return Err(TypeError::Unsupported(format!(
                            "a module constant that applies a function cannot be a struct \
                             argument yet: '{applied}'"
                        )));
                    }
                    // A type bound to a value slot is its metatype there, which
                    // converts to no value type, as at the pin: even the slot's
                    // own type (`Box[Int]` over `n: Int`) is `AnyStruct[Int]`.
                    if let CtValue::Type(bound) = &value
                        && let Some(error) = self.type_value_mismatch(bound.clone(), ty)
                    {
                        let (expected, found) = match error {
                            TypeError::MetatypeConversion { found, expected } => (expected, found),
                            TypeError::TypeAsInstance(ty) => {
                                (ty.clone(), format!("AnyStruct[{ty}]"))
                            }
                            error => return Err(error),
                        };
                        return Err(TypeError::ParamValueType {
                            callee: mojito_symbol::symbol::declaration_owner_name(&decl.id().owner)
                                .unwrap_or(&decl.id().owner)
                                .to_string(),
                            param: name.clone(),
                            expected,
                            found,
                        });
                    }
                    let actual = self
                        .member_value_type(expr)
                        .or_else(|| self.ct_value_ty(&value, ty))
                        .ok_or_else(|| TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: "a non-materializable compile-time value".to_string(),
                            context: format!("value parameter '{name}'"),
                        })?;
                    if matches!(actual.kind(), TyKind::ParamList(_))
                        && matches!(ty.kind(), TyKind::ParamList(element)
                            if mojito_types::types::trait_type_bound(element).is_some())
                    {
                        if !self.param_list_converts(&actual, ty) {
                            return Err(TypeError::ParamListConversion {
                                param: name.trim_start_matches('*').to_string(),
                                from: actual.to_string(),
                                to: ty.to_string(),
                            });
                        }
                    } else if let Some(lanes) = self.simd_length_lanes(
                        &actual,
                        ty,
                        &self
                            .param_context
                            .constant(value.clone())
                            .map_err(param_error)?,
                    ) {
                        let lanes = lanes?;
                        let lanes =
                            TyArg::Val(lanes.as_i64().map_or(CtValue::Expr(lanes), CtValue::Int));
                        return Ok(BoundArgument::attribute(lanes));
                    } else if !self.coerces_by_origins(&actual, ty) {
                        // A struct parameter converts its argument through
                        // the `@implicit` constructor a run-time argument's
                        // own type selects (a literal its `IntLiteral` one).
                        // An annotated constant (`comptime T: StaticString =
                        // "x"`) whose value folds to a literal converts from
                        // that literal, as its binding did.
                        let source = match self.argument_source_type(expr, &value, &actual) {
                            declared if self.coerces_by_origins(&declared, ty) => actual.clone(),
                            declared => declared,
                        };
                        let mut slot = None;
                        let converted =
                            self.implicit_parameter_application(&source, ty, |slot_ty| {
                                slot = Some(slot_ty.clone());
                                self.applied_argument(expr, slot_ty)
                            })?;
                        let (Some(application), Some(slot)) = (converted, slot) else {
                            return Err(TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: actual.to_string(),
                                context: format!("value parameter '{name}'"),
                            });
                        };
                        self.record_literal_materializations(expr, &source, &slot)?;
                        // The converted value is the constructor's call node,
                        // never the argument expression's run-time value.
                        return Ok(BoundArgument::attribute(TyArg::Val(
                            application.into_value(),
                        )));
                    }
                    self.record_literal_materializations(expr, &actual, ty)?;
                    // A type list (`Ts.values`), a closed raw list of
                    // values, or a KGEN string has no run-time form.
                    let closed_value_list = matches!(&value, CtValue::Tuple(elements)
                        if !elements.iter().any(|element| matches!(element, CtValue::Type(_))));
                    let attribute = value.is_compile_time_only()
                        || (closed_value_list && matches!(ty.kind(), TyKind::ParamList(_)));
                    let bound = |value| BoundArgument {
                        value: TyArg::Val(value),
                        attribute,
                    };
                    // A symbolic enclosing-scope parameter stays symbolic here;
                    // specialization bakes it out like the callable case above.
                    if matches!(value, CtValue::Expr(_)) {
                        return Ok(bound(value));
                    }
                    let rendered = value.to_string();
                    if let Some(value) = value.materialize_as(ty) {
                        return Ok(bound(value));
                    }
                    // A literal at a vector whose dtype is still symbolic
                    // (`Scalar[dtype]`) is its declared literal initializer's
                    // application.
                    self.implicit_parameter_application(&actual, ty, |slot| {
                        self.applied_argument(expr, slot)
                    })?
                    .map(|application| bound(application.into_value()))
                    .ok_or_else(|| TypeError::TypeMismatch {
                        expected: ty.to_string(),
                        found: rendered,
                        context: format!("value parameter '{name}'"),
                    })
                }
                ParamArg::Named { value, .. } => self.resolve_param_arg(decl, value),
            },
        }
    }

    /// Record the binding a declared-parameter binder bound the
    /// application at `span`'s bracket arguments to, keyed by declaration,
    /// as the checked call's parameters read it. One of run-time values
    /// alone keeps a binding another binder already recorded there.
    pub(super) fn record_declared_attributes(
        &self,
        span: SourceSpan,
        attributes: &DeclaredAttributes,
    ) {
        let binding = attributes.binding();
        if binding.slots.iter().all(Vec::is_empty)
            || (binding.is_runtime()
                && self.has_fact(&span, |facts| facts.parameter_attributes.is_some()))
        {
            return;
        }
        self.record_at(span).parameter_attributes = Some(binding);
    }

    /// Record, in the binding the application at `span` already recorded,
    /// the closed values `folded` carries for its value-shaped bracket
    /// arguments: the instantiation solved them, so the call carries them
    /// as parameter attributes, as upstream's op does.
    pub(super) fn record_folded_bracket_attributes(
        &self,
        span: &SourceSpan,
        folded: &DeclaredAttributes,
    ) {
        let folded = folded.binding();
        if folded.is_runtime() {
            return;
        }
        let mut binding = self
            .fact(span, |facts| facts.parameter_attributes.as_ref())
            .unwrap_or_default();
        for (index, slot) in folded.slots.into_iter().enumerate() {
            for bound in slot {
                if bound.attribute.is_some() {
                    binding.set(index, bound.position, bound.attribute);
                }
            }
        }
        for (index, declaration) in folded.declarations.iter().enumerate() {
            binding.declare(index, declaration.as_deref());
        }
        self.record_at(span.clone()).parameter_attributes = Some(binding);
    }

    /// Resolve the type in value position `expr`, recording the type
    /// at it ([`mojito_checked::checked::NodeFacts::type_operand`]).
    pub(super) fn type_value(&self, expr: &Expr) -> Result<Ty, TypeError> {
        let resolved = self.emit_expr_type(expr)?;
        self.record_at(expr.source_span()).type_operand = Some(resolved.clone());
        Ok(resolved)
    }

    /// Resolve `List[T]` from its single type argument.
    pub(super) fn list_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        if args.is_empty() {
            return Ok((list_type(Ty::infer()), DeclaredAttributes::default()));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "List".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let (element, attributes) = self.collection_type_arguments("List", args)?;
        // `List[T: AnyType]`: only a `!kgen.type` binder falls short. An
        // inference hole (`List[_]`) is unknown until its binding solves it.
        if self.conformance(&element[0], "AnyType")
            == mojito_types::conformance::Conformance::Refuted
        {
            return Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: element[0].to_string(),
                trait_name: "AnyType".to_string(),
                reason: None,
            });
        }
        let attributes = DeclaredAttributes::of_builtin("List", LIST_PARAMS, args, attributes)?;
        Ok((list_type(element[0].clone()), attributes))
    }

    /// Each of `args` resolved as a type argument of `collection`, beside
    /// the attribute each bound to.
    fn collection_type_arguments(
        &self,
        collection: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Vec<Ty>, BracketAttributes), TypeError> {
        let types = args
            .iter()
            .map(|argument| self.collection_type_argument(collection, argument))
            .collect::<Result<Vec<_>, _>>()?;
        let attributes = types_attributes(args, &types);
        Ok((types, attributes))
    }

    pub(super) fn collection_type_argument(
        &self,
        collection: &str,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        let resolved = match argument {
            mojito_ast::ast::ParamArg::Value(expression) => {
                self.denoted_type(expression, &format!("{collection} type argument"))
            }
            mojito_ast::ast::ParamArg::Named { .. } => Err(TypeError::TypeMismatch {
                expected: "a positional type argument".to_string(),
                found: "a named argument".to_string(),
                context: format!("{collection} type argument"),
            }),
        }?;
        reject_stored_callable_type(&resolved, &format!("the '{collection}' element type"))?;
        Ok(resolved)
    }

    pub(super) fn set_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let types = match args {
            [] => Vec::new(),
            [element] => vec![self.collection_type_argument("Set", element)?],
            [element, hasher] => vec![
                self.collection_type_argument("Set", element)?,
                self.hasher_type_argument("Set", hasher)?,
            ],
            _ => {
                return Err(TypeError::WrongTypeArgCount {
                    name: "Set".to_string(),
                    expected: 2,
                    got: args.len(),
                });
            }
        };
        let ty = match types.as_slice() {
            [] => self.nominal_set(Ty::infer()),
            [element] => self.nominal_set(element.clone()),
            [element, hasher, ..] => {
                mojito_types::types::set_type_with(element.clone(), hasher.clone())
            }
        };
        let attributes = types_attributes(args, &types);
        Ok((
            ty,
            DeclaredAttributes::of_builtin("Set", SET_PARAMS, args, attributes)?,
        ))
    }

    pub(super) fn dict_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let types = match args {
            [] => Vec::new(),
            [key, value] => vec![
                self.collection_type_argument("Dict", key)?,
                self.collection_type_argument("Dict", value)?,
            ],
            [key, value, hasher] => vec![
                self.collection_type_argument("Dict", key)?,
                self.collection_type_argument("Dict", value)?,
                self.hasher_type_argument("Dict", hasher)?,
            ],
            _ => {
                return Err(TypeError::WrongTypeArgCount {
                    name: "Dict".to_string(),
                    expected: 3,
                    got: args.len(),
                });
            }
        };
        let ty = match types.as_slice() {
            [key, value] => self.nominal_dict(key.clone(), value.clone()),
            [key, value, hasher, ..] => {
                mojito_types::types::dict_type_with(key.clone(), value.clone(), hasher.clone())
            }
            _ => self.nominal_dict(Ty::infer(), Ty::infer()),
        };
        let attributes = types_attributes(args, &types);
        Ok((
            ty,
            DeclaredAttributes::of_builtin("Dict", DICT_PARAMS, args, attributes)?,
        ))
    }

    /// `Set[T]` with the hasher argument filled from the linked declaration's
    /// default (`H: Hasher = default_hasher`); a seam program without the
    /// stdlib keeps the element-only spelling.
    pub(super) fn nominal_set(&self, element: Ty) -> Ty {
        match self.default_hasher_argument(mojito_types::types::SET_TYPE_NAME) {
            Some(hasher) => mojito_types::types::set_type_with(element, hasher),
            None => set_type(element),
        }
    }

    /// `Dict[K, V]` with the hasher argument filled from the linked
    /// declaration's default; see [`Self::nominal_set`].
    pub(super) fn nominal_dict(&self, key: Ty, value: Ty) -> Ty {
        match self.default_hasher_argument(mojito_types::types::DICT_TYPE_NAME) {
            Some(hasher) => mojito_types::types::dict_type_with(key, value, hasher),
            None => dict_type(key, value),
        }
    }

    /// The declared default of a hash collection's trailing `H: Hasher`
    /// parameter, when the collection is linked with one.
    fn default_hasher_argument(&self, collection: &str) -> Option<Ty> {
        let info = self.structs.get(collection)?;
        match info.decls().last()? {
            ParamDecl::Type {
                bounds,
                default: Some(default),
                ..
            } if bounds.iter().any(|bound| *bound == super::hasher_trait()) => {
                Some((*default).clone())
            }
            _ => None,
        }
    }

    /// Resolve an explicit hasher type argument of a hash collection; it
    /// must name a `Hasher` conformer.
    fn hasher_type_argument(
        &self,
        collection: &str,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        let hasher = self.collection_type_argument(collection, argument)?;
        let hasher_trait = super::hasher_trait();
        if !self.conforms_to(&hasher, &hasher_trait) {
            return Err(TypeError::TraitNotSatisfied {
                param: "H".to_string(),
                ty: hasher.to_string(),
                reason: self.trait_failure_reason(&hasher, &hasher_trait),
                trait_name: hasher_trait,
            });
        }
        Ok(hasher)
    }

    /// Resolve a `Pointer[...]` bracket by [`Self::pointer_bracket`]. The pin's
    /// `origin` has no default, so a type position must bind it (or unbind
    /// it explicitly with `_` or `...`); a bracket leaving it open fails to
    /// infer it, as the pin reports.
    pub(super) fn pointer_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let bracket = self.pointer_bracket(args)?;
        let Some(element) = bracket.element else {
            return Err(TypeError::BadCall {
                func: mojito_types::types::POINTER_TYPE_NAME.to_string(),
                reason: "failed to infer parameter 'T'".to_string(),
            });
        };
        let Some((origin, mutability)) = bracket.origin else {
            return Err(TypeError::CannotInferParam {
                name: mojito_types::types::POINTER_TYPE_NAME.to_string(),
                param: "origin".to_string(),
            });
        };
        let address_space = bracket
            .address_space
            .unwrap_or(mojito_types::types::AddressSpaceSlot::GENERIC);
        // Only an initialized local's annotation leaves its space to the
        // initializer; a storage position must name a concrete one.
        if address_space.is_inferred()
            && self.application_strictness() == super::StorageStrictness::Full
        {
            return Err(TypeError::NotConcrete(pointer_spelling(
                &element, &origin, args,
            )));
        }
        Ok((
            Ty::pointer_from_lit_ref(&mojito_types::origin::RefTy {
                referent: element,
                origin,
                mutability,
                address_space,
            }),
            bracket.attributes,
        ))
    }

    /// Bind a `Pointer[...]` bracket as upstream's binder binds
    /// `Pointer[mut: Bool, //, T: AnyType, origin: Origin[mut=mut], *,
    /// address_space: AddressSpace = .GENERIC]`: positionals fill `T` and
    /// `origin`, a keyword binds any slot by name (`mut` only ahead of every
    /// positional, `address_space` only by keyword), and a `mut=` literal
    /// passes the origin at its type: `True` rejects an immutable origin,
    /// `False` converts a mutable one to immutable.
    pub(super) fn pointer_bracket(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<PointerBracket, TypeError> {
        use mojito_ast::ast::ParamArg;
        let name = mojito_types::types::POINTER_TYPE_NAME;
        let keywords = mojito_ast::call::bracket_keywords(args);
        let slots = mojito_ast::call::match_parameter_slots(POINTER_PARAMS, &keywords)
            .map_err(|error| error.into_type_error(name))?;
        let mut bracket = PointerBracket::default();
        let mut origin_argument = None;
        for (argument, &slot) in args.iter().zip(&slots) {
            let value = match argument {
                ParamArg::Named { value, .. } => value.as_ref(),
                other @ ParamArg::Value(_) => other,
            };
            match POINTER_PARAMS[slot].name {
                "mut" => {
                    bracket.mutability =
                        super::generics::origin_mutability_literal(name, "mut", value)?;
                }
                "T" => bracket.element = Some(self.pointer_element_arg(name, value)?),
                "origin" => {
                    origin_argument = Some(value);
                }
                _ if origin_placeholder(value) => {
                    bracket.address_space = Some(mojito_types::types::AddressSpaceSlot::inferred(
                        &self.param_context,
                    ));
                }
                _ => bracket.address_space = Some(self.address_space_arg(value)?),
            }
        }
        if let Some(argument) = origin_argument {
            bracket.origin = Some(self.pointer_bracket_origin(name, argument, &bracket)?);
        }
        if let Some((origin, mutability)) = bracket.origin.take() {
            bracket.origin = Some(match (bracket.mutability, mutability.known()) {
                (Some(true), Some(false)) => {
                    return Err(TypeError::TypeMismatch {
                        expected: super::origin_value_type(Some(true), false),
                        found: super::origin_value_type(Some(false), false),
                        context: format!("value passed to 'origin' of '{name}'"),
                    });
                }
                (Some(false), _) => mojito_types::origin::Origin::immutable(origin.clone())
                    .canonical_pointer_parts(mojito_types::origin::Mutability::Immutable)
                    .ok_or_else(|| {
                        TypeError::Unsupported(format!(
                            "the origin {origin} as an immutable pointer's origin"
                        ))
                    })?,
                _ => (origin, mutability),
            });
        }
        let attributes =
            args.iter()
                .zip(&slots)
                .map(|(argument, &slot)| match POINTER_PARAMS[slot].name {
                    "T" => bracket.element.as_ref().and_then(|element| {
                        bracket_attribute(argument, &TyArg::Ty(element.clone()))
                    }),
                    "origin" => bracket.origin.as_ref().and_then(|(origin, _)| {
                        bracket_attribute(argument, &TyArg::Origin(origin.clone()))
                    }),
                    _ => None,
                })
                .collect();
        bracket.attributes =
            DeclaredAttributes::of_builtin(name, POINTER_PARAMS, args, attributes)?;
        Ok(bracket)
    }

    /// A `Pointer` bracket's element argument: a type, a bare identifier,
    /// or an expression denoting a type.
    fn pointer_element_arg(
        &self,
        name: &str,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        use mojito_ast::ast::ParamArg;
        match argument {
            ParamArg::Value(
                element @ Expr {
                    kind: ExprKind::Identifier(_),
                    ..
                },
            ) => self.emit_expr_type(element),
            ParamArg::Value(element) => self.denoted_type(element, &format!("{name} element type")),
            ParamArg::Named { value, .. } => self.pointer_element_arg(name, value),
        }
    }

    /// A `Pointer` bracket's origin argument. A placeholder (`_`) is a
    /// parameter whose origin, and so whose `mut`, is inferred per call: it
    /// accepts any provenance, the callee holds no loan, and the body cannot
    /// prove the capability, so upstream rejects writes and
    /// mutable-demanding forwards through it. Outside parameter position
    /// the bare placeholder is not concrete (upstream's verdict).
    fn pointer_bracket_origin(
        &self,
        name: &str,
        argument: &mojito_ast::ast::ParamArg,
        bracket: &PointerBracket,
    ) -> Result<
        (
            mojito_types::origin::Origin,
            mojito_types::origin::Mutability,
        ),
        TypeError,
    > {
        if !origin_placeholder(argument) {
            return self.pointer_origin_arg(argument);
        }
        if !self.resolving_parameter_annotation.get() {
            let element = bracket
                .element
                .as_ref()
                .map_or_else(|| "T".to_string(), ToString::to_string);
            return Err(TypeError::Unsupported(format!(
                "'{name}[{element}, _]' is not concrete outside parameter position: a \
                 bare placeholder origin is inferred only for a parameter; spell \
                 ImmPointer[T, _] or MutPointer[T, _] here"
            )));
        }
        Ok((
            mojito_types::origin::Origin::UnsafeAny { mutable: false },
            mojito_types::origin::Mutability::Immutable,
        ))
    }

    /// The address space a `Pointer`'s `address_space=` argument, or an
    /// `unsafe_address_space_cast` target, names: an `AddressSpace` value,
    /// closed (`AddressSpace.SHARED`, a leading-dot `.GENERIC`) or a binder
    /// in scope (`Self.address_space`, `a`).
    pub(super) fn address_space_arg(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::types::AddressSpaceSlot, TypeError> {
        use mojito_ast::ast::ParamArg;
        let mut value = argument;
        while let ParamArg::Named { value: inner, .. } = value {
            value = inner;
        }
        let rewritten;
        let value = match value {
            ParamArg::Value(expr) if super::inference::contextual_root_span(expr).is_some() => {
                rewritten = ParamArg::Value(super::inference::with_contextual_root(
                    expr,
                    &ExprKind::Identifier(mojito_types::types::ADDRESS_SPACE_TYPE_NAME.to_string()),
                ));
                &rewritten
            }
            other => other,
        };
        let found = self.slot_value(value)?;
        mojito_types::types::AddressSpaceSlot::of_value(&found).ok_or_else(|| {
            TypeError::TypeMismatch {
                expected: mojito_types::types::ADDRESS_SPACE_TYPE_NAME.to_string(),
                found: found.to_string(),
                context: "a pointer's address space".to_string(),
            }
        })
    }

    /// A `Pointer`'s origin argument, resolved as any struct's origin
    /// argument is ([`Self::resolve_origin_param_arg`]), with its `mut`:
    /// upstream's `Pointer` is a struct whose `origin` parameter is an
    /// ordinary `Origin[mut=mut]` value.
    pub(super) fn pointer_origin_arg(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<
        (
            mojito_types::origin::Origin,
            mojito_types::origin::Mutability,
        ),
        TypeError,
    > {
        use mojito_types::origin::{Mutability, Origin};
        let (origin, mutability) = self.resolve_origin_param_arg(argument)?;
        // The receiver's origin has the receiver's symbolic capability, in
        // the body as in the signature, unless a cast pins it.
        let mutability = match mutability {
            _ if matches!(origin.projection_base(), Origin::SelfParam) => Mutability::RECEIVER,
            Some(mutability) => mutability,
            None if matches!(origin.uncast().projection_base(), Origin::SelfParam) => {
                Mutability::RECEIVER
            }
            None => {
                return Err(TypeError::Unsupported(format!(
                    "the origin {origin} of unknown mutability as a pointer's origin"
                )));
            }
        };
        origin.canonical_pointer_parts(mutability).ok_or_else(|| {
            TypeError::Unsupported(if matches!(origin.uncast(), Origin::Union(_)) {
                format!(
                    "the origin {origin}, which names an origin binder or the receiver, as a pointer's union origin"
                )
            } else {
                format!("the origin {origin} as a pointer's origin")
            })
        })
    }

    /// Look up an enclosing `Origin`-bounded type parameter by name: the
    /// binder and its declared `mut`.
    pub(super) fn enclosing_origin_param(
        &self,
        name: &str,
    ) -> Result<
        (
            mojito_types::origin::OriginParamId,
            mojito_types::origin::Mutability,
        ),
        TypeError,
    > {
        let (index, decl) = self
            .enclosing_origin_type_param(name)
            .ok_or_else(|| TypeError::UnknownSelfParam(name.to_string()))?;
        let id = mojito_types::origin::OriginParamId(index);
        Ok((id, super::generics::origin_binder_mutability(&decl, id)))
    }

    /// Resolve `Tuple[T1, …, Tn]` from its type arguments (each a type).
    /// The tuple storage of the literal struct of the type list `list`
    /// (`!kgen.struct<:param_list<type> L isParamPack>`): a known list's
    /// elements, the spread (`*Ts`) of a pack in scope as `Self.Ts` or `Ts`,
    /// and the spread of a list still open otherwise.
    pub(super) fn param_list_tuple(&self, list: ParamExpr, site: &Expr) -> Result<Ty, TypeError> {
        use mojito_ast::ast::ParamArg;
        if let Some(elements) = self
            .param_context
            .list_elements(&list)
            .map_err(param_error)?
        {
            return Ok(TyKind::Tuple(
                elements
                    .into_iter()
                    .map(mojito_types::types::DependentType::resolve)
                    .collect(),
            )
            .into());
        }
        let pack = list.as_decl_ref().and_then(|reference| {
            let name = reference.name.trim_start_matches('*');
            let at = |kind| Expr::new(kind, site.span);
            let own = at(ExprKind::Member {
                object: Box::new(at(ExprKind::Identifier("Self".to_string()))),
                field: name.to_string(),
            });
            let bare = at(ExprKind::Identifier(name.to_string()));
            [
                ParamArg::Value(at(ExprKind::Spread(Box::new(own)))),
                ParamArg::Value(at(ExprKind::Spread(Box::new(bare)))),
            ]
            .iter()
            .find_map(|pack| self.pack_spread_argument(pack).and_then(Result::ok))
        });
        let element = pack.unwrap_or_else(|| {
            TyKind::Dependent(mojito_types::types::DependentType::Parameter(list)).into()
        });
        Ok(TyKind::Tuple(vec![element]).into())
    }

    pub(super) fn tuple_element_types(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Vec<Ty>, BracketAttributes), TypeError> {
        let mut elems = Vec::with_capacity(args.len());
        let mut attributes = Vec::with_capacity(args.len());
        for arg in args {
            if let Some(pack) = self.pack_spread_argument(arg) {
                let pack = pack?;
                attributes.push(bracket_attribute(arg, &TyArg::Ty(pack.clone())));
                elems.push(pack);
                continue;
            }
            // A spread of a computed list is its elements once every length
            // in it is known, and the whole argument list until then.
            if let mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Spread(operand),
                ..
            }) = arg
            {
                let list = self.type_list_operand(operand)?.ok_or_else(|| {
                    TypeError::Unsupported(
                        "a spread type argument takes a pack or a type list".to_string(),
                    )
                })?;
                let spread = mojito_types::types::spread_arguments(
                    mojito_types::types::TUPLE_TYPE_NAME,
                    &list,
                )
                .map_err(param_error)?;
                elems.extend(spread.into_iter().filter_map(|argument| match argument {
                    TyArg::Ty(ty) => Some(ty),
                    TyArg::Val(_) | TyArg::Origin(_) => None,
                }));
                attributes.push(None);
                continue;
            }
            let element = match arg {
                // A bare-identifier arg is reinterpreted as a type (as elsewhere).
                mojito_ast::ast::ParamArg::Value(
                    element @ Expr {
                        kind: ExprKind::Identifier(_),
                        ..
                    },
                ) => self.emit_expr_type(element)?,
                mojito_ast::ast::ParamArg::Value(element) => {
                    self.denoted_type(element, "Tuple element type")?
                }
                mojito_ast::ast::ParamArg::Named { .. } => {
                    return Err(TypeError::Unsupported(
                        "named Tuple element arguments".to_string(),
                    ));
                }
            };
            attributes.push(bracket_attribute(arg, &TyArg::Ty(element.clone())));
            elems.push(element);
        }
        Self::reject_mixed_spread("Tuple", &elems)?;
        Ok((elems, attributes))
    }

    /// The pack a type names by itself while it is still a parameter: the
    /// enclosing struct's `Self.Ts`, or a bare `Ts` of an enclosing `def` or
    /// method.
    fn unbound_pack_type(&self, base: &Expr) -> Option<Ty> {
        if let Some(name) = base.self_member() {
            let starred = format!("*{}", name.trim_start_matches('*'));
            return ParamDecl::type_pack_list(&self.self_decls, name)
                .and_then(|_| self.self_pack_type(&starred).ok())
                .filter(|ty| matches!(ty.kind(), TyKind::Param { .. }));
        }
        let ExprKind::Identifier(name) = &base.kind else {
            return None;
        };
        self.lookup_tparam(&format!("*{}", name.trim_start_matches('*')))
    }

    /// The type of the compile-time value a type expression subscripts or a
    /// member read reads through: a struct construction
    /// (`TypeList[Ts.values]()`), or one through a type's parameterized
    /// member (`M.List[Bool, Int]()`) or a generic alias (`L[Bool, Int]()`),
    /// whose struct has an `__init__` taking no argument. The construction
    /// is typed only to evaluate it, as the type a read reads through.
    pub(super) fn compile_time_value_type(&self, value: &Expr) -> Result<Ty, TypeError> {
        self.evaluating(|| {
            let not_construction = || {
                TypeError::Unsupported(
                    "a subscript in a type takes a struct construction".to_string(),
                )
            };
            let (ty, name, args, kwargs) = match &value.kind {
                ExprKind::Call {
                    name,
                    param_args,
                    args,
                    kwargs,
                } if self.structs.contains_key(name) => (
                    self.applied_type(name, param_args)?.0,
                    name.clone(),
                    args,
                    kwargs,
                ),
                // A construction through a generic alias (`L[Bool, Int]()`)
                // constructs the struct its application denotes.
                ExprKind::Call {
                    name,
                    param_args,
                    args,
                    kwargs,
                } if self.comptime_aliases.contains_key(name) => {
                    let ty = self.applied_type(name, param_args)?.0;
                    let Some((name, _)) = ty.kind().as_non_pointer_struct() else {
                        return Err(not_construction());
                    };
                    let name = name.clone();
                    (ty, name, args, kwargs)
                }
                ExprKind::Invoke {
                    callee,
                    param_args,
                    args,
                    kwargs,
                } => {
                    let ExprKind::Member { object, field } = &callee.kind else {
                        return Err(not_construction());
                    };
                    if !self.may_denote_type(object) {
                        return Err(not_construction());
                    }
                    let ty = self.member_type(object, field, param_args)?;
                    let Some((name, _)) = ty.kind().as_non_pointer_struct() else {
                        return Err(not_construction());
                    };
                    let name = name.clone();
                    (ty, name, args, kwargs)
                }
                // A call of a type member (`CompilationTarget.current()`)
                // constructs the type the member names.
                ExprKind::MethodCall { .. } => {
                    let construction = self
                        .type_member_construction(value)
                        .ok_or_else(not_construction)?;
                    return self.compile_time_value_type(&construction);
                }
                _ => return Err(not_construction()),
            };
            let info = self.structs.get(&name).ok_or_else(not_construction)?;
            if !args.is_empty() || !kwargs.is_empty() {
                return Err(TypeError::Unsupported(
                    "a compile-time construction takes no arguments".to_string(),
                ));
            }
            let nullary = info.methods.get("__init__").is_some_and(|overloads| {
                overloads
                    .iter()
                    .any(|overload| overload.required.iter().all(|required| !required))
            }) || (info.fieldwise_init && info.declared_field_names.is_empty());
            if !nullary {
                return Err(TypeError::BadCall {
                    func: name,
                    reason: "no '__init__' overload takes no arguments".to_string(),
                });
            }
            Ok(ty)
        })
    }

    /// `value`, an argument of a value parameter or of an applied
    /// callable's runtime parameter of type `ty`, in the parameter's
    /// domain: an application computing a `String` (`label[pick(n)]()`,
    /// `ln(String("ab"))`) binds a `String` parameter as the compile-time
    /// string the elaborator freezes its result to, which a value
    /// parameter's call carries as the argument's attribute
    /// ([`bracket_attribute`]).
    pub(super) fn string_parameter_value(&self, value: CtValue, ty: &Ty) -> CtValue {
        let CtValue::Expr(application) = &value else {
            return value;
        };
        let mojito_types::param_expr::ParamKind::Apply { function, args } = application.kind()
        else {
            return value;
        };
        let computes_string = application.meta().as_value().is_some_and(|computed| {
            computed
                .kind()
                .as_non_pointer_struct()
                .is_some_and(|(name, computed_args)| {
                    computed_args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name)
                })
        });
        if !ty.is_string_literal() || !computes_string {
            return value;
        }
        CtValue::Expr(self.param_context.apply(
            function,
            args,
            mojito_types::param_expr::MetaTy::value(ty.clone()),
        ))
    }

    /// Resolve the argument of a trait-valued parameter whose type is
    /// `AnyTrait[bound]`: a trait or a composition refining every trait of
    /// `bound`, or a trait-valued binder in scope (`f[Tr]()`, `W[Self.Tr]`)
    /// whose own bound does. A type or another value is a type mismatch, as
    /// upstream's metatype conversion reports it.
    fn resolve_trait_param_arg(
        &self,
        decl: &ParamDecl,
        bound: &[String],
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use mojito_ast::ast::ParamArg;
        let mismatch = |found: String| TypeError::ParamValueType {
            callee: mojito_symbol::symbol::declaration_owner_name(&decl.id().owner)
                .unwrap_or(&decl.id().owner)
                .to_string(),
            param: decl.name().to_string(),
            expected: TyKind::AnyTrait(bound.to_vec()).to_string(),
            found,
        };
        let type_argument =
            |source: Result<Ty, TypeError>| source.map(|ty| mismatch(format!("AnyStruct[{ty}]")));
        let value = match arg {
            ParamArg::Named { value, .. } => {
                return self.resolve_trait_param_arg(decl, bound, value);
            }
            ParamArg::Value(expr) => match self.trait_value(expr) {
                Some(value) => {
                    let value = value?;
                    return if self.traits_satisfy(&value.ty, bound) {
                        Ok(TyArg::Val(CtValue::Trait(value.traits)))
                    } else {
                        Err(mismatch(TyKind::AnyTrait(value.ty).to_string()))
                    };
                }
                None => match self.compile_dependent_ct_expr(expr) {
                    Ok(value) => value.into_value(),
                    Err(error) => {
                        let source = match &expr.kind {
                            ExprKind::Identifier(_) => self.emit_expr_type(expr),
                            _ if let Some((name, args)) = self.parameter_application(expr) => {
                                self.applied_type(name, &args).map(|(ty, _)| ty)
                            }
                            _ => return Err(error),
                        };
                        return Err(type_argument(source).unwrap_or(error));
                    }
                },
            },
        };
        match self.ct_value_ty(&value, &Ty::error()) {
            Some(ref ty) if let TyKind::AnyTrait(traits) = ty.kind() => {
                if self.traits_satisfy(traits, bound) {
                    Ok(TyArg::Val(value))
                } else {
                    Err(mismatch(ty.to_string()))
                }
            }
            found => Err(mismatch(match (&value, found) {
                (CtValue::IntLiteral(literal), _) => format!("IntLiteral[{literal}]"),
                // A type's value is its metatype.
                (CtValue::Type(ty), _) => match ty.kind() {
                    TyKind::Param { bounds, .. } => self.binder_metatype(bounds),
                    _ => format!("AnyStruct[{ty}]"),
                },
                (_, Some(ty)) => ty.to_string(),
                (value, None) => value.to_string(),
            })),
        }
    }

    fn pack_is_shadowed(&self, starred: &str) -> bool {
        // `local_type_aliases` is searched ahead of `tparams`, so a binding in
        // any alias scope is nearer than one in any parameter scope.
        let nearest = |name: &str| {
            self.local_type_aliases
                .iter()
                .rposition(|scope| scope.contains_key(name))
                .map(|scope| (1u8, scope))
                .or_else(|| {
                    self.param_scopes
                        .borrow()
                        .types
                        .iter()
                        .rposition(|scope| scope.contains_key(name))
                        .map(|scope| (0u8, scope))
                })
        };
        let bare = nearest(starred.trim_start_matches('*'));
        match (bare, nearest(starred)) {
            // A pack declared bare alongside its starred name is not a shadow.
            (Some(bare), Some(pack)) => bare > pack,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// The pack a spread type argument (`*Self.Ts`, or a `def`'s own `*Ts`)
    /// names while that pack is still a parameter, as its `TyKind::Param`.
    pub(super) fn pack_spread_argument(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Option<Result<Ty, TypeError>> {
        // The elaborator's pack-qualification pass owns the rule that a member
        // spells its struct's pack `Self.Ts`; a spread it has normalized to
        // the bare spelling still names that pack.
        let operand = arg.spread_operand()?;
        if let Some(field) = operand.self_member() {
            return Some(self.self_pack_type(&format!("*{field}")));
        }
        let ExprKind::Identifier(name) = &operand.kind else {
            return None;
        };
        let pack = format!("*{name}");
        if self.pack_is_shadowed(&pack) {
            return Some(Err(TypeError::UnknownType(pack)));
        }
        Some(
            self.lookup_tparam(&pack)
                .map_or_else(|| self.self_pack_type(&pack), Ok),
        )
    }

    /// The element `__UninitStorage[T]`'s one bracket argument stores,
    /// beside its attribute.
    pub(super) fn uninit_storage_element(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let (mut elements, attributes) = self.tuple_element_types(args)?;
        let attributes = DeclaredAttributes::of_builtin(
            mojito_types::types::UNINIT_STORAGE_TYPE_NAME,
            T_PARAMS,
            args,
            attributes,
        )?;
        Ok((elements.remove(0), attributes))
    }

    pub(super) fn tuple_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let (elements, attributes) = self.tuple_element_types(args)?;
        for element in &elements {
            reject_stored_callable_type(element, "the 'Tuple' element type")?;
        }
        let attributes = DeclaredAttributes::of_builtin("Tuple", PACK_PARAMS, args, attributes)?;
        Ok((mojito_types::types::tuple_type(elements), attributes))
    }

    /// Upstream's spelling of an origin in a diagnostic: a place as
    /// `origin_of(<place>)` through the names in scope, the enclosing
    /// struct's own binder by its name.
    pub(super) fn display_origin_with_names(
        &self,
        origin: &mojito_types::origin::Origin,
    ) -> String {
        use mojito_types::origin::{Origin, OriginSeg};
        match origin.uncast() {
            Origin::Place(place) => {
                let mut text = self
                    .owner_name(place.root)
                    .map_or_else(|| format!("#{}", place.root.0), str::to_string);
                for segment in &place.path {
                    match segment {
                        OriginSeg::Field(name) => {
                            text.push('.');
                            text.push_str(name);
                        }
                        OriginSeg::AnyIndex => text.push_str("[_]"),
                        OriginSeg::Interior(tag) => {
                            text.push_str("._get_owned_interior[\"");
                            text.push_str(tag);
                            text.push_str("\"]");
                        }
                        OriginSeg::Subtree => {}
                    }
                }
                format!("origin_of({text})")
            }
            // The enclosing struct's own binder spells as its name
            // (`RefBox[origin]`), as upstream prints it.
            Origin::Param(id) => self
                .self_ty
                .as_ref()
                .and_then(|ty| match ty.kind() {
                    struct_ty if let Some((name, _)) = struct_ty.as_non_pointer_struct() => {
                        self.structs.get(name)
                    }
                    _ => None,
                })
                .and_then(|info| info.origin_decl(*id))
                .map_or_else(|| origin.to_string(), |decl| decl.name().to_string()),
            other => other.to_string(),
        }
    }

    /// Upstream's spelling of a checked type in a diagnostic, with each struct
    /// origin argument rendered as `origin_of(<place>)` through the names in
    /// scope (`P[origin_of(xs)]`), `Self.o`-style binders as `origin#N`, and an
    /// unbound slot as `_`. An `ImmOrigin(o)` cast spells as `o`, as
    /// upstream prints the cast's provenance.
    pub(super) fn display_ty_with_origin_names(&self, ty: &Ty) -> String {
        let origin_text =
            |origin: &mojito_types::origin::Origin| self.display_origin_with_names(origin);
        match ty.kind() {
            struct_ty
                if let Some((name, args)) = struct_ty.as_non_pointer_struct()
                    && !args.is_empty() =>
            {
                let base = mojito_types::types::unqualified_type_name(&Ty::new(TyKind::Struct(
                    name.clone(),
                    Vec::new(),
                )));
                let arguments = args
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => self.display_ty_with_origin_names(ty),
                        TyArg::Val(value) => value.to_string(),
                        TyArg::Origin(origin) => origin_text(origin),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{base}[{arguments}]")
            }
            _ => mojito_types::types::unqualified_type_name(ty),
        }
    }

    /// The annotated type of an initialized local with the origin slots its
    /// annotation left to inference bound from the initializer's type
    /// (`var p: P = P(Pointer(to=xs))` declares a `P[origin_of(xs)]`, as
    /// upstream infers it), recursing through type arguments. A `SIMD[dt, _]`
    /// width hole takes the initializer's lane count the same way.
    pub(super) fn bind_unbound_tails(declared: &Ty, found: &Ty) -> Ty {
        match (declared.kind(), found.kind()) {
            (kind, _)
                if let Some((dtype, width)) = kind.as_simd()
                    && width.is_inferred() =>
            {
                mojito_types::types::simd_slots(found)
                    .filter(|(_, found_width)| !found_width.is_expr())
                    .and_then(|(_, found_width)| {
                        mojito_types::types::simd_ty_from_slots(dtype.clone(), found_width).ok()
                    })
                    .unwrap_or_else(|| declared.clone())
            }
            (declared_kind, found_kind)
                if let (Some(reference), Some(found)) = (
                    declared_kind.pointer_lit_ref(),
                    found_kind.pointer_lit_ref(),
                ) && reference.address_space.is_inferred() =>
            {
                Ty::pointer_from_lit_ref(&mojito_types::origin::RefTy {
                    address_space: found.address_space,
                    ..reference
                })
            }
            (TyKind::Struct(name, declared_args), TyKind::Struct(found_name, found_args))
                if !declared.is_simd()
                    && !declared.is_pointer()
                    && !found.is_pointer()
                    && name == found_name
                    && declared_args.len() == found_args.len() =>
            {
                declared.reusing(TyKind::Struct(
                    name.clone(),
                    declared_args
                        .iter()
                        .zip(found_args)
                        .map(|(declared, found)| match (declared, found) {
                            (TyArg::Ty(declared), TyArg::Ty(found)) => {
                                TyArg::Ty(Self::bind_unbound_tails(declared, found))
                            }
                            (
                                TyArg::Origin(mojito_types::origin::Origin::Unbound),
                                TyArg::Origin(_),
                            )
                            | (TyArg::Val(CtValue::Deferred(_)), TyArg::Val(_)) => found.clone(),
                            _ => declared.clone(),
                        })
                        .collect(),
                ))
            }
            _ => declared.clone(),
        }
    }

    /// The upstream-worded rejection of a struct value flowing into a
    /// destination of the same struct whose origin arguments differ (`p =
    /// P(Pointer(to=ys))` over `P[origin_of(xs)]`), or of a string literal
    /// into a `StringLiteral` of another text, or a numeric literal into a
    /// literal type of another value; `None` when the two differ in
    /// anything but their origin tails or their values.
    pub(super) fn struct_origin_mismatch(&self, found: &Ty, expected: &Ty) -> Option<TypeError> {
        if found.kind().string_literal_value().is_some()
            && expected.kind().string_literal_value().is_some()
            || found.is_numeric_literal() && expected.is_numeric_literal()
        {
            return (found != expected).then(|| TypeError::OriginIdentityMismatch {
                found: found.to_string(),
                expected: expected.to_string(),
            });
        }
        let (Some((found_name, found_args)), Some((expected_name, expected_args))) = (
            found.kind().as_non_pointer_struct(),
            expected.kind().as_non_pointer_struct(),
        ) else {
            return None;
        };
        if found_name != expected_name || found_args.len() != expected_args.len() {
            return None;
        }
        // An origin's `mut=` value differs with its origin.
        let params = self.struct_params(found_name);
        let mutability_slot = |index: usize| {
            params.len() == found_args.len() && params[index].is_origin_mutability_binder(params)
        };
        let only_tails_differ = found_args.iter().zip(expected_args).enumerate().all(
            |(index, (found, expected))| match (found, expected) {
                (TyArg::Origin(_), TyArg::Origin(_)) => true,
                (TyArg::Val(_), TyArg::Val(_)) if mutability_slot(index) => true,
                _ => found == expected,
            },
        );
        only_tails_differ.then(|| TypeError::OriginIdentityMismatch {
            found: self.display_ty_with_origin_names(found),
            expected: self.display_ty_with_origin_names(expected),
        })
    }

    /// Construct the checked identity of a struct application.
    pub(super) fn struct_instance_type(name: &str, arguments: Vec<TyArg>) -> Ty {
        TyKind::Struct(
            name.to_string(),
            mojito_types::types::canonical_pack_arguments(name, arguments),
        )
        .into()
    }

    /// The struct's own instance type as `Self` resolves to inside its methods:
    /// [`Self::struct_instance_type`] of `info.self_arguments()` — the same value the checker
    /// installs as `self_ty` during registration, so a `self`-typed parameter's
    /// resolved type is equal to it. Used to canonicalize overload keys back to
    /// `Self` (see [`method_lowered_name`]). `None` when `name` is not a
    /// registered struct (e.g. abstract trait dispatch).
    pub(super) fn self_instance_ty(&self, name: &str) -> Option<Ty> {
        // The nominal String struct keeps its stable `String` overload spelling
        // on both sides; canonicalizing it to `Self` would break the
        // literal→String constructor bridge (see `self_struct_spelling`).
        if mojito_symbol::symbol::is_stdlib_string_struct(name) {
            return None;
        }
        let info = self.structs.get(name)?;
        Some(Self::struct_instance_type(name, info.self_arguments()))
    }

    /// The generator a bare parameterized struct name denotes as the base
    /// of a member read (`S.n`, `S.Of[Bool]`): the struct at its own
    /// binders, its defaults unfilled, as the pin reads it. `None` when
    /// `name` is a binding, a parameter, or an alias, or names no
    /// parameterized struct.
    pub(super) fn unbound_generator(&self, name: &str) -> Option<Ty> {
        if self.lookup(name).is_some()
            || self.lookup_tparam(name).is_some()
            || self.value_parameter_in_scope(name).is_some()
            || self.comptime_aliases.contains_key(name)
        {
            return None;
        }
        let info = self.structs.get(name)?;
        (!info.decls().is_empty())
            .then(|| TyKind::Struct(name.to_string(), info.self_arguments()).into())
    }

    /// Reject `value`, the member `member` read through the generator of
    /// struct `name`, when it depends on one of the generator's unbound
    /// parameters, as the pin does.
    pub(super) fn reject_unbound_dependence(
        &self,
        name: &str,
        member: &str,
        value: &CtValue,
    ) -> Result<(), TypeError> {
        let Some(info) = self.structs.get(name) else {
            return Ok(());
        };
        let mut search = BinderSearch {
            decls: &info.decls(),
            found: None,
        };
        // The search never fails; the rebuilt value is discarded.
        let _ = mojito_types::types::rewrite_value(value, &mut search);
        match search.found {
            Some(parameter) => Err(TypeError::UnboundParameterMember {
                member: member.to_string(),
                parameter: format!("{name}.{}", parameter.trim_start_matches('*')),
            }),
            None => Ok(()),
        }
    }

    /// Resolve the alternatives of `Variant[T1, ..., Tn]`.  Alternative order
    /// is significant because it becomes the runtime tag; duplicate types would
    /// make `isa[T]` and `value[T]` ambiguous and are rejected here.
    pub(super) fn variant_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        if args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: "Variant".to_string(),
                expected: 1,
                got: 0,
            });
        }
        let mut alternatives = Vec::with_capacity(args.len());
        for arg in args {
            let alternative = match self.pack_spread_argument(arg) {
                Some(pack) => pack?,
                None => self.type_param_argument(arg, "Variant alternative")?,
            };
            reject_stored_callable_type(&alternative, "a 'Variant' alternative type")?;
            alternatives.push(alternative);
        }
        Self::reject_mixed_spread("Variant", &alternatives)?;
        let attributes = types_attributes(args, &alternatives);
        let attributes = DeclaredAttributes::of_builtin("Variant", PACK_PARAMS, args, attributes)?;
        Ok((TyKind::Variant(alternatives).into(), attributes))
    }

    /// Resolve the case payload types of `__EnumStorage[T1, ..., Tn]`. Unlike
    /// a `Variant`'s alternatives they may repeat, since every operation
    /// selects a case by index, and an enum without cases has none.
    pub(super) fn enum_storage_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let alternatives = args
            .iter()
            .map(|arg| self.type_param_argument(arg, "enum case payload"))
            .collect::<Result<Vec<_>, _>>()?;
        let attributes = DeclaredAttributes::of_builtin(
            mojito_types::types::ENUM_STORAGE_TYPE_NAME,
            PACK_PARAMS,
            args,
            types_attributes(args, &alternatives),
        )?;
        Ok((TyKind::Variant(alternatives).into(), attributes))
    }

    /// Whether the declaration being checked may name `__EnumStorage`: an
    /// `enum`, whose generated members do, or the bundled library.
    pub(super) const fn enum_storage_allowed(&self) -> bool {
        self.enum_declaration || self.bundled_stdlib_declaration
    }

    pub(super) fn type_param_argument(
        &self,
        arg: &mojito_ast::ast::ParamArg,
        context: &str,
    ) -> Result<Ty, TypeError> {
        match arg {
            mojito_ast::ast::ParamArg::Value(expression)
                if let Some(reflected) = self.reflected_type_operand(expression).transpose() =>
            {
                reflected
            }
            mojito_ast::ast::ParamArg::Value(
                expression @ Expr {
                    kind: ExprKind::Identifier(_),
                    ..
                },
            ) => self.emit_expr_type(expression),
            mojito_ast::ast::ParamArg::Value(expression) => self.denoted_type(expression, context),
            mojito_ast::ast::ParamArg::Named { .. } => Err(TypeError::Unsupported(format!(
                "named arguments are not supported in {context}"
            ))),
        }
    }

    /// Whether `SIMD[args]` resolves through the declared struct. It does
    /// not before the bundled `SIMD` registers (a module alias registered
    /// ahead of the structs, R1846), nor over a `_` width a binding's
    /// initializer solves (R1790), which only the positional reader holds.
    fn simd_declared_application(&self, args: &[mojito_ast::ast::ParamArg]) -> bool {
        self.structs
            .contains_key(mojito_types::types::SIMD_TYPE_NAME)
            && !args.iter().any(is_hole_argument)
    }

    /// Resolve `SIMD[DType.<dt>, width]` from its two positional arguments
    /// to its `(dtype, width)` slots.
    fn positional_simd_dims(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(SimdDtype, SimdWidth, BracketAttributes), TypeError> {
        if args.len() != 2 {
            return Err(TypeError::WrongTypeArgCount {
                name: "SIMD".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let dtype = self.dtype_from_arg(&args[0])?;
        let (width, width_attribute) = if matches!(
            &args[1],
            mojito_ast::ast::ParamArg::Value(Expr { kind: ExprKind::Identifier(name), .. }) if name == "_"
        ) {
            (SimdWidth::inferred(&self.param_context), None)
        } else {
            self.simd_width(&args[1])?
        };
        let attributes = vec![dtype_attribute(&args[0], &dtype), width_attribute];
        Ok((dtype, width, attributes))
    }

    /// The type `SIMD[args]` names and its bracket attributes: the declared
    /// struct's application, which binds positional, keyword, and default
    /// brackets as any struct's do, or the positional reader
    /// ([`Self::simd_type`]) where the declaration cannot answer yet.
    pub(super) fn simd_application(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        if !self.simd_declared_application(args) {
            return self.simd_type(args);
        }
        let name = mojito_types::types::SIMD_TYPE_NAME;
        let info = &self.structs[name];
        let decls = info.decls().into_owned();
        let BoundParameters {
            arguments,
            attributes,
            ..
        } = self.resolve_struct_use_args(name, &decls, args, &[], &[])?;
        Ok((Self::struct_instance_type(name, arguments), attributes))
    }

    /// The (canonicalized) `Ty` for `SIMD[DType.<dt>, width]` by the
    /// positional reader — a width-1 `float64` resolves to
    /// `Ty::float64()` (the unification).
    fn simd_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, DeclaredAttributes), TypeError> {
        let (dtype, width, attributes) = self.positional_simd_dims(args)?;
        let attributes = DeclaredAttributes::of_builtin(
            mojito_types::types::SIMD_TYPE_NAME,
            SIMD_PARAMS,
            args,
            attributes,
        )?;
        Ok((simd_of(dtype, width)?, attributes))
    }

    /// Evaluate a SIMD width argument to the `SIMDLength` its slot holds:
    /// a length as it is, and an `Int` converted by upstream's implicit
    /// `SIMDLength.__init__(Int)`, so `SIMD[dt, n]` holds `SIMDLength(n)`
    /// as at the pin. A known lane count must be positive; a symbolic one
    /// is its instantiation's to check.
    pub(super) fn simd_width(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<(SimdWidth, Option<TyArg>), TypeError> {
        let value = self.slot_value(arg)?;
        let attribute = bracket_attribute(arg, &TyArg::Val(value.clone()));
        let length = match value {
            CtValue::Expr(expr) if expr.meta().as_value().is_some_and(is_simd_length) => expr,
            CtValue::Expr(lanes) if lanes.meta().is_integer() => self
                .simd_length_of_lanes(&lanes)
                .ok_or_else(|| TypeError::BadSimdWidth(lanes.to_string()))?,
            CtValue::Expr(expr) => return Err(TypeError::BadSimdWidth(expr.to_string())),
            CtValue::Struct { .. } => self.param_context.constant(value).map_err(param_error)?,
            count => {
                let width = mojito_types::param_expr::fold::integer_value(&count)
                    .and_then(|width| width.to_i64())
                    .ok_or_else(|| TypeError::BadSimdWidth(count.to_string()))?;
                return Ok((positive_width(SimdWidth::Known(width))?, attribute));
            }
        };
        let width = SimdWidth::of_length(&self.param_context, length).map_err(param_error)?;
        Ok((positive_width(width)?, attribute))
    }

    /// The `SIMDLength` parameter a width slot holds: its length, or the
    /// closed `SIMDLength(k)` of a known count.
    pub(super) fn simd_length_parameter(&self, width: &SimdWidth) -> Option<ParamExpr> {
        match width {
            SimdWidth::Expr(length) => Some(length.clone()),
            SimdWidth::Known(count) => self
                .param_context
                .constant(mojito_types::types::simd_length_value(*count))
                .ok(),
        }
    }

    /// The `SIMDLength` the lane count `lanes` converts to: upstream's
    /// implicit `SIMDLength.__init__(Int)` applied to it, so `n` is
    /// `SIMDLength(n)` and four lanes the closed `SIMDLength(4)`. A program
    /// checked without the bundled library declares no `SIMDLength`, and
    /// its width is that constructor's folded body, the same value.
    pub(super) fn simd_length_of_lanes(&self, lanes: &ParamExpr) -> Option<ParamExpr> {
        let length = mojito_types::types::simd_length_ty();
        let by_symbol = || {
            mojito_types::types::simd_length_converted(
                &self.param_context,
                mojito_symbol::symbol::SIMD_LENGTH_FROM_INT,
                lanes,
            )
            .ok()
        };
        if !self
            .structs
            .contains_key(mojito_types::types::SIMD_LENGTH_TYPE_NAME)
        {
            return by_symbol();
        }
        // A field type resolved before the bundled `SIMDLength`'s
        // initializers are checked applies the same constructor by symbol.
        self.implicit_parameter_application(&Ty::int(), &length, |_| Some(lanes.clone()))
            .ok()
            .flatten()
            .or_else(by_symbol)
    }

    /// The error a value argument `expr` that did not evaluate reports: a
    /// run-time value is no parameter value, as at the pin
    /// ([`Self::dynamic_parameter_error`]), and a name nothing declares is
    /// unknown.
    fn unevaluated_argument(&self, error: TypeError, expr: &Expr) -> TypeError {
        match (&error, &expr.kind) {
            (TypeError::NotComptime(_), _)
                if !self.outside_bodies() && !self.reads_compile_time_alone(expr) =>
            {
                self.dynamic_parameter_error(expr)
            }
            (TypeError::NotComptime(_), ExprKind::Identifier(variable))
                if self.lookup(variable).is_some() =>
            {
                TypeError::DynamicParameterValue
            }
            (TypeError::NotComptime(_), ExprKind::Identifier(variable))
                if !self.comptime_aliases.contains_key(variable)
                    && !self.structs.contains_key(variable) =>
            {
                TypeError::UndefinedVariable(variable.clone())
            }
            _ => error,
        }
    }

    /// The value an index-valued bracket argument evaluates to: a
    /// compile-time value, or — while its declaration is a template — an
    /// expression over value parameters in scope.
    fn slot_value(&self, arg: &mojito_ast::ast::ParamArg) -> Result<CtValue, TypeError> {
        Ok(match arg {
            mojito_ast::ast::ParamArg::Value(expr) => {
                let _lifting = self.lifting_position();
                match self.eval_associated_ct(expr) {
                    Ok(value) => value,
                    Err(error) => CtValue::Expr(self.lifted_application(expr).ok_or(error)?),
                }
            }
            mojito_ast::ast::ParamArg::Named { .. } => {
                return Err(TypeError::BadSimdWidth("a named argument".to_string()));
            }
        })
    }

    /// If `name` is a generic type parameter currently in scope, return its
    /// complete checked type-parameter fact.
    pub(super) fn lookup_tparam(&self, name: &str) -> Option<Ty> {
        self.local_type_aliases
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .cloned()
            .or_else(|| {
                self.param_scopes
                    .borrow()
                    .types
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(name))
                    .cloned()
            })
    }

    /// The parameter value `value` binds a declaration to: a constant, or a
    /// type pack's list of element types still symbolic (`Of[T, U]` in a
    /// template's body).
    fn bound_parameter_value(&self, value: &CtValue) -> Option<ParamExpr> {
        self.param_context.constant(value.clone()).ok().or_else(|| {
            let CtValue::Tuple(elements) = value else {
                return None;
            };
            let types = elements
                .iter()
                .map(|element| match element {
                    CtValue::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            self.param_context.type_list(types).ok()
        })
    }

    /// The type an argument `expr`, valued `value` of type `actual`,
    /// converts from: a struct parameter converts through the `@implicit`
    /// constructor its run-time type selects (a literal its `IntLiteral`
    /// one), and a binder a signature reads is no variable there, of the
    /// type it binds whatever a module declaration shares its name.
    fn argument_source_type(&self, expr: &Expr, value: &CtValue, actual: &Ty) -> Ty {
        if matches!(value, CtValue::Expr(read) if read.as_decl_ref().is_some()) {
            return actual.clone();
        }
        // The argument is typed only to read its type off: the call's
        // binding converts the value, not the syntax.
        let _position = self.comptime_position();
        self.evaluating(|| self.infer(expr))
            .unwrap_or_else(|_| actual.clone())
    }

    /// The type of the compile-time value a member read's base spells when
    /// it names no type (`w.E` over `comptime w = W[3]()`): upstream types
    /// the base as an expression and reads the member on its type. A local
    /// `comptime` binding the builder does not inline types as its
    /// construction (`comptime L = TypeList.of[Trait=AnyType, Int]()`); a
    /// run-time local or argument types as its declared type (`var z: p.T`).
    fn value_base_type(&self, expr: &Expr) -> Option<Ty> {
        if let Some(ty) = self
            .compile_dependent_ct_expr(expr)
            .ok()
            .and_then(|built| built.meta().value_ty())
        {
            return Some(ty);
        }
        let ExprKind::Identifier(name) = &expr.kind else {
            return None;
        };
        if let Some(bound) = self.local_comptime_value(name) {
            return self.compile_time_value_type(bound).ok();
        }
        self.member_subject(expr)
    }
}

/// Whether `value` is an application computing a compile-time string
/// ([`Checker::string_parameter_value`]).
/// `List[T: AnyType, /]`.
const LIST_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("T")];
/// `Set[T: KeyElement, H: Hasher & … = default_hasher]`.
const SET_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("T"), BuiltinParam::new("H")];
/// `Dict[K: KeyElement, V: Movable, H: Hasher & … = default_hasher]`.
const DICT_PARAMS: &[BuiltinParam] = &[
    BuiltinParam::new("K"),
    BuiltinParam::new("V"),
    BuiltinParam::new("H"),
];
/// A pack-only list: `Tuple[*Ts: AnyType]`, `Variant[*Ts: AnyType]`, and
/// the compiler-private storages over one.
const PACK_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("*Ts")];
/// `SIMD[dtype: DType, length: SIMDLength]`.
const SIMD_PARAMS: &[BuiltinParam] = &[BuiltinParam::new("dtype"), BuiltinParam::new("length")];
/// `Pointer[mut: Bool, //, T: AnyType, origin: Origin[mut=mut], *,
/// address_space: AddressSpace = .GENERIC]`.
const POINTER_PARAMS: &[BuiltinParam] = &[
    BuiltinParam::infer_only("mut"),
    BuiltinParam::new("T"),
    BuiltinParam::new("origin"),
    BuiltinParam::keyword_only("address_space"),
];
fn computed_string_literal(value: &CtValue) -> bool {
    matches!(value, CtValue::Expr(expression)
        if matches!(expression.kind(), mojito_types::param_expr::ParamKind::Apply { .. })
            && expression.meta().as_value().is_some_and(Ty::is_string_literal))
}

fn type_is_symbolic(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Param { .. }
        | TyKind::Assoc { .. }
        | TyKind::Dependent(_)
        | TyKind::SelfType
        | TyKind::Infer => true,
        kind if let Some((dtype, width)) = kind.as_simd() => {
            dtype.is_symbolic() || width.is_symbolic()
        }
        kind if kind.is_string_literal() || kind.is_numeric_literal() => false,
        kind if let Some(reference) = kind.pointer_lit_ref() => {
            reference.address_space.is_symbolic() || type_is_symbolic(&reference.referent)
        }
        TyKind::Struct(_, arguments) => arguments.iter().any(tyarg_is_symbolic),
        TyKind::KgenSimd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        TyKind::KgenAddressSimd { width } => width.is_symbolic(),
        TyKind::AnyStruct(ty) => type_is_symbolic(ty),
        TyKind::None
        | TyKind::Never
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::PopIntLiteral
        | TyKind::PopFloatLiteral
        | TyKind::KgenDeferred
        | TyKind::LitOriginSet
        | TyKind::KgenTarget
        | TyKind::Index
        | TyKind::I1
        | TyKind::Ui8
        | TyKind::KgenDtype => false,
        TyKind::Func(_) | TyKind::GenericFunc(_) => ty
            .callable()
            .is_some_and(|func| func.types().any(type_is_symbolic)),
        TyKind::Generator(_) => mojito_types::types::is_symbolic(ty),
        TyKind::Overload(types)
        | TyKind::Tuple(types)
        | TyKind::RuntimePack(types)
        | TyKind::Variant(types) => types.iter().any(type_is_symbolic),
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element) => type_is_symbolic(element),
        TyKind::PopArray { size, element } => size.is_symbolic() || type_is_symbolic(element),
        TyKind::LitOrigin(mutability) => mutability.is_symbolic(),
        TyKind::KgenPointer {
            element,
            address_space,
        } => address_space.is_symbolic() || type_is_symbolic(element),
        TyKind::Ref(reference) => {
            reference.address_space.is_symbolic() || type_is_symbolic(&reference.referent)
        }
        TyKind::LitRefPack { elements, .. } => elements.iter().any(type_is_symbolic),
    }
}

/// Whether a parameterized member's own parameter is a value (`n: Int`).
pub(super) fn is_value_parameter(param: &mojito_ast::ast::TypeParam) -> bool {
    param.value_type.is_some()
        || param
            .lone_bound()
            .is_some_and(|only| scalar_type_name(only).is_some())
}

/// The literal address space `N` an MLIR type spelling's text ends with
/// after `prefix` (`!lit.ref<{},{},1>`).
fn literal_address_space(
    text: &str,
    prefix: &str,
) -> Option<mojito_types::types::AddressSpaceSlot> {
    text.strip_prefix(prefix)?
        .strip_suffix('>')?
        .parse()
        .ok()
        .map(mojito_types::types::AddressSpaceSlot::Known)
}

pub(super) fn unsupported_mlir_type() -> TypeError {
    TypeError::Unsupported(
        "this MLIR type spelling is outside the supported subset; only \
         '__mlir_type.index', '__mlir_type.i1', '__mlir_type.`!kgen.scalar<dtype>`', \
         '__mlir_type.`!kgen.type`', '__mlir_type.`!kgen.string`', \
         '__mlir_type[`!kgen.param_list<`, T, `>`]', \
         '__mlir_type[`!kgen.simd<`, length._mlir_value, `, `, dtype._mlir_value, `>`]', \
         '__mlir_type[`!kgen.struct<`, ~Ts.values, ` isParamPack>`]', \
         '__mlir_type[`!kgen.struct<:param_list<type> `, L, ` isParamPack>`]', \
         '__mlir_type[`!kgen.pointer<`, T, `>`]' (with an optional address space \
         `as._value._mlir_value` or literal `N`), \
         '__mlir_type[`!lit.origin<`, mut._mlir_value, `>`]', \
         '__mlir_type[`!lit.ref<`, T, `, `, origin._mlir_origin, `>`]' (with an optional \
         address space, as for '!kgen.pointer'), \
         '__mlir_type[`!pop.array<`, n.__mlir_index__(), `, `, T, `>`]', and \
         '__mlir_type.`!pop.array<n, T>`' are accepted"
            .to_string(),
    )
}

/// An MLIR type operand of the wrong attribute type: `expected` holds only
/// the `_mlir_value` of a `holder`.
fn mlir_operand_mismatch(expected: &str, holder: &str) -> TypeError {
    TypeError::TypeMismatch {
        expected: format!("'{expected}' (a {holder}'s '_mlir_value')"),
        found: "another operand".to_string(),
        context: "MLIR type operand".to_string(),
    }
}

/// `width` as a SIMD length, which must be positive; a symbolic one is its
/// instantiation's to check. A length need not be a power of two: the pin
/// admits `SIMD[DType.float32, 3]` and rejects only a value of it its code
/// generation meets, which the MIR verifier stands in for.
fn positive_width(width: SimdWidth) -> Result<SimdWidth, TypeError> {
    match width {
        SimdWidth::Known(w) if w < 1 => Err(TypeError::BadSimdWidth(w.to_string())),
        width => Ok(width),
    }
}

/// The struct `name` spelled bare as shown when it declares parameters:
/// a hole per parameter the application spells (`P[_, _]`), or `W` alone
/// when every parameter is infer-only. `None` for a struct without
/// parameters or a compiler-minted specialization.
fn unbound_struct_spelling(name: &str, params: &[ParamDecl]) -> Option<String> {
    use mojito_ast::call::CompileTimeParam as _;
    if params.is_empty() || mojito_symbol::symbol::specialization_template(name).is_some() {
        return None;
    }
    let holes = params.iter().filter(|decl| !decl.infer_only()).count();
    Some(if holes == 0 {
        name.to_string()
    } else {
        format!("{name}[{}]", vec!["_"; holes].join(", "))
    })
}

/// `ty`, an alias's expansion, with each origin binder the bracket
/// `args` leaves to inference (`MutPointer[T, _]`, `AS[_]`) bound as
/// a parameter's `_` binds it: a struct's origin slot unbound, and then a
/// pointer's origin any origin at the binder's declared mutability
/// (immutable for a parametric binder, whose capability the body
/// cannot prove). An unbound slot is no pointer's origin, so the first
/// substitution leaves pointers to the second.
fn bind_alias_origin_placeholders(
    ty: &Ty,
    decls: &[ParamDecl],
    args: &[mojito_ast::ast::ParamArg],
) -> Ty {
    use super::origins::{OriginBindings, substitute_struct_origins};
    use mojito_types::origin::{Mutability, Origin};
    let Ok(slots) = mojito_ast::call::bracket_unbound_slots(decls, args) else {
        return ty.clone();
    };
    let mut tails = OriginBindings::default();
    let mut pointers = OriginBindings::default();
    for (decl, open) in decls.iter().zip(&slots.open) {
        if let (
            ParamDecl::Origin {
                origin, mutability, ..
            },
            Some(_),
        ) = (decl, open)
        {
            let mutable = matches!(mutability, OriginMutability::Fixed(true));
            tails.origins.insert(*origin, Origin::Unbound);
            pointers
                .origins
                .insert(*origin, Origin::UnsafeAny { mutable });
            pointers
                .mutabilities
                .insert(*origin, Mutability::of(mutable).mut_argument());
        }
    }
    substitute_struct_origins(&substitute_struct_origins(ty, &tails), &pointers)
}

fn origin_placeholder(argument: &mojito_ast::ast::ParamArg) -> bool {
    let mut value = argument;
    while let mojito_ast::ast::ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    matches!(
        value,
        mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) if name == "_" || name == "..."
    )
}

/// The origin a struct's origin slot `param` binds for the resolved
/// argument `(origin, mutability)`: an `ImmOrigin` (`Origin[mut=False]`)
/// slot given an origin not already immutable binds its `ImmOrigin(o)`
/// cast, as upstream's implicit conversion of the argument to the slot's
/// type makes it, and as an explicit `ImmOrigin(o)` resolves.
pub(super) fn immutable_slot_origin(
    decl: &ParamDecl,
    (origin, mutability): (
        mojito_types::origin::Origin,
        Option<mojito_types::origin::Mutability>,
    ),
) -> (
    mojito_types::origin::Origin,
    Option<mojito_types::origin::Mutability>,
) {
    use mojito_types::origin::{Mutability, Origin};
    let immutable_slot = declared_mutability(decl) == Some(false);
    if !immutable_slot
        || mutability == Some(Mutability::Immutable)
        || matches!(origin, Origin::Unbound)
        || origin.is_immutable_cast()
    {
        return (origin, mutability);
    }
    (Origin::immutable(origin), Some(Mutability::Immutable))
}

/// Whether an application argument is syntactically origin-shaped — an
/// `origin_of(...)` call, a binder name (`o`, `Self.o`), or a projected origin
/// expression — regardless of whether its places resolve in this context.
pub(super) fn syntactic_origin_argument(argument: &mojito_ast::ast::ParamArg) -> bool {
    let mut value = argument;
    while let mojito_ast::ast::ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    match value {
        mojito_ast::ast::ParamArg::Value(expression) => match &expression.kind {
            // An `origin_of` observation, or a conversion of an origin
            // (`ImmOrigin(o)`); the check rejects a call building no origin.
            ExprKind::Call { name, .. } if name == "origin_of" => true,
            ExprKind::Call { .. } => {
                super::origins::conversion_operand(expression).is_some_and(|operand| {
                    syntactic_origin_argument(&mojito_ast::ast::ParamArg::Value(operand.clone()))
                })
            }
            ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. } => true,
            // A method building an origin from an origin receiver
            // (`o.unsafe_mut_cast[False]()`).
            ExprKind::Invoke {
                callee,
                args,
                kwargs,
                ..
            } if args.is_empty() && kwargs.is_empty() => match &callee.kind {
                ExprKind::Member { object, .. } => {
                    syntactic_origin_argument(&mojito_ast::ast::ParamArg::Value((**object).clone()))
                }
                _ => false,
            },
            _ => false,
        },
        mojito_ast::ast::ParamArg::Named { .. } => false,
    }
}

/// The mutability an origin binder fixes (`MutOrigin`, `ImmOrigin`);
/// `None` for a parametric or inferred one, or any other binder.
const fn declared_mutability(decl: &ParamDecl) -> Option<bool> {
    match decl {
        ParamDecl::Origin {
            mutability: mojito_types::types::OriginMutability::Fixed(fixed),
            ..
        } => Some(*fixed),
        _ => None,
    }
}

/// The first of a generator's own binders a read value names
/// ([`Checker::reject_unbound_dependence`]), by its declared name.
struct BinderSearch<'a> {
    decls: &'a [ParamDecl],
    found: Option<String>,
}

impl BinderSearch<'_> {
    fn note(&mut self, id: &mojito_types::param_expr::ParamId) {
        if self.found.is_none()
            && let Some(decl) = self.decls.iter().find(|decl| decl.id() == id)
        {
            self.found = Some(decl.name().to_string());
        }
    }
}

impl mojito_types::types::TyRewrite for BinderSearch<'_> {
    fn param(&mut self, binder: &mojito_types::param_expr::ParamRef) -> Option<Ty> {
        self.note(&binder.id);
        None
    }

    fn expr(
        &mut self,
        expr: &ParamExpr,
    ) -> Result<ParamExpr, mojito_types::param_expr::ParamError> {
        for reference in expr.free_parameters() {
            self.note(&reference.id);
        }
        let mut nested = Vec::new();
        expr.visit(&mut |node| nested.extend(node.embedded_types().into_iter().cloned()));
        for ty in &nested {
            mojito_types::types::rewrite_ty(ty, self)?;
        }
        Ok(expr.clone())
    }
}

/// A `Pointer` application as the pin shows one that is not concrete:
/// its pointee, its origin (`_` where the bracket leaves it open), and an
/// `address_space=_` hole.
fn pointer_spelling(
    element: &Ty,
    origin: &mojito_types::origin::Origin,
    args: &[mojito_ast::ast::ParamArg],
) -> String {
    let origin_open = args.iter().any(|argument| {
        origin_placeholder(argument)
            && !matches!(argument, mojito_ast::ast::ParamArg::Named { name, .. } if name == "address_space")
    });
    let origin = if origin_open {
        "_".to_string()
    } else {
        origin.to_string()
    };
    format!(
        "{}[{element}, {origin}, address_space=_]",
        mojito_types::types::POINTER_TYPE_NAME
    )
}

/// Whether a bracket argument is the `_` hole, positional or keyword.
fn is_hole_argument(arg: &mojito_ast::ast::ParamArg) -> bool {
    match arg {
        mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => name == "_",
        mojito_ast::ast::ParamArg::Named { value, .. } => is_hole_argument(value),
        mojito_ast::ast::ParamArg::Value(_) => false,
    }
}

/// The bracket argument a display or a module display constant spells, as
/// its literal constructor's application: the parameter value MIR reads,
/// never the display's run-time value.
const fn applied_display_argument(application: ParamExpr) -> BoundArgument {
    BoundArgument::attribute(TyArg::Val(CtValue::Expr(application)))
}

/// The rejection of an index into a base that is no type-valued member.
fn dependent_indexing_unsupported() -> TypeError {
    TypeError::Unsupported(
        "dependent type indexing requires a type-valued associated member".to_string(),
    )
}
