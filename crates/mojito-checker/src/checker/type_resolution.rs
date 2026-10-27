//! Resolution of source type annotations into checked `Ty` values:
//! type-argument-bearing builtins (List/Set/Dict/Pointer/Tuple/SIMD/Variant),
//! dependent/associated type projection, and type-parameter lookup.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::origin::CallableEnvironment;
use mojito_types::types::{FuncTy, GenericFuncTy, TransferSet};
use mojito_types::types::{TYPE_OF_ALL_TYPES, TyKind};

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

/// What each bracket argument of one application bound to, in source
/// order: `Some` for an argument with no run-time form (a type, an origin,
/// or a compile-time value), as upstream's op carries each argument as the
/// parameter attribute its binding produced.
pub(super) type BracketAttributes = Vec<Option<TyArg>>;

/// What [`Checker::resolve_use_params`] bound a use site's parameters to:
/// the type substitution, the full argument list in declaration order (the
/// identity), and each source bracket argument's attribute.
pub(super) struct BoundParameters {
    pub(super) subst: TySubst,
    pub(super) arguments: Vec<TyArg>,
    pub(super) attributes: BracketAttributes,
}

/// Whether a bound argument is still symbolic: an expression over open
/// parameters, or an aggregate holding one.
pub(super) fn tyarg_is_symbolic(argument: &TyArg) -> bool {
    match argument {
        TyArg::Val(CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)) => true,
        TyArg::Val(CtValue::Tuple(values) | CtValue::List(values)) => values.iter().any(|value| {
            matches!(
                value,
                CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)
            )
        }),
        TyArg::Val(_) => false,
        TyArg::Origin(
            mojito_types::origin::Origin::Param(_) | mojito_types::origin::Origin::SelfParam,
        ) => true,
        TyArg::Origin(_) => false,
        TyArg::Ty(ty) => type_is_symbolic(ty),
    }
}

/// The attribute bracket argument `argument` carries once binding resolved
/// it to `bound`: a type-shaped argument always has one, and a
/// value-shaped one only when it bound to a type or an origin, a run-time
/// value having none.
pub(super) fn bracket_attribute(
    argument: &mojito_ast::ast::ParamArg,
    bound: &TyArg,
) -> Option<TyArg> {
    use mojito_ast::ast::ParamArg;
    match argument {
        ParamArg::Named { value, .. } => bracket_attribute(value, bound),
        ParamArg::Type(_) => Some(bound.clone()),
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
            SimdDtype::Known(dtype) => CtValue::Dtype(*dtype),
            SimdDtype::Expr(expr) => CtValue::Expr(expr.clone()),
        }),
    )
}

/// Where the arguments a partition forwards to a binder sit in the source
/// bracket list, beside the attribute each argument it kept back (an
/// origin) bound to: the binder's attributes, over the forwarded list,
/// scatter back into source order.
#[derive(Clone)]
pub(super) struct ForwardedBrackets {
    positions: Vec<usize>,
    kept: BracketAttributes,
}

impl ForwardedBrackets {
    /// Every argument of a `len`-long bracket list forwarded as it stands.
    pub(super) fn all(len: usize) -> Self {
        Self {
            positions: (0..len).collect(),
            kept: vec![None; len],
        }
    }

    /// A partition of a `len`-long bracket list that has forwarded or kept
    /// nothing yet.
    pub(super) fn empty(len: usize) -> Self {
        Self {
            positions: Vec::new(),
            kept: vec![None; len],
        }
    }

    /// Forward the argument at source `position`.
    pub(super) fn forward(&mut self, position: usize) {
        self.positions.push(position);
    }

    /// Keep back the argument at source `position`, bound to `attribute`.
    pub(super) fn keep(&mut self, position: usize, attribute: Option<TyArg>) {
        self.kept[position] = attribute;
    }

    /// The source list's attributes, given the binder's `forwarded` ones.
    pub(super) fn scatter(&self, forwarded: BracketAttributes) -> BracketAttributes {
        let mut attributes = self.kept.clone();
        for (position, attribute) in self.positions.iter().zip(forwarded) {
            attributes[*position] = attribute;
        }
        attributes
    }
}

/// One explicit origin argument a struct application supplied, resolved
/// against its slot: the slot's index in the declaration's full parameter
/// list (the `OriginParamId` domain of the struct's own binders) and the
/// origin the argument names (its mutability was already validated against
/// the slot by `accept_origin_argument`).
pub(in crate::checker) struct ExplicitStructOrigin {
    pub(in crate::checker) id: mojito_types::origin::OriginParamId,
    pub(in crate::checker) origin: mojito_types::origin::Origin,
    /// The argument's capability when known (`None` for a symbolic one): an
    /// `ImmOrigin(o)` cast binds the slot immutably whatever `o` allows.
    pub(in crate::checker) mutability: Option<mojito_types::origin::Mutability>,
}

/// A struct application's compile-time arguments with the origin slots
/// partitioned out: `forwarded` is exactly what the origin-erased binder
/// sees, `explicit_origins` the validated origin arguments, and `tail` the
/// application's origin tail — one origin per slot, `Origin::Unbound` where
/// the application left the slot to inference.
pub(super) struct PartitionedStructArgs {
    pub(super) forwarded: Vec<mojito_ast::ast::ParamArg>,
    /// Where `forwarded` sits in the source list, beside each origin
    /// argument's attribute.
    pub(super) brackets: ForwardedBrackets,
    pub(super) explicit_origins: Vec<ExplicitStructOrigin>,
    pub(super) tail: Vec<mojito_types::origin::Origin>,
}

impl PartitionedStructArgs {
    /// The tail as struct type arguments, to append after the binder prefix.
    pub(super) fn tail_arguments(&self) -> impl Iterator<Item = TyArg> + '_ {
        self.tail.iter().cloned().map(TyArg::Origin)
    }
}

impl Checker {
    /// The type denoted by a source annotation; resolves type parameters and
    /// validates struct names and type-argument counts.
    pub(super) fn ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        self.resolve_ty_from_anno(ty)
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
            ParamArg::Type(ty) => ty
                .expression_spelling(mojito_common::token::Span::default())
                .ok_or_else(|| TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: "a type".to_string(),
                    context: format!("value parameter '{}'", decl.name()),
                })?,
        };
        let generator = self.compile_dependent_ct_expr(&expr)?;
        if !generator
            .meta()
            .is_generator_of(&mojito_types::param_expr::MetaTy::value(expected.clone()))
        {
            return Err(TypeError::ParamValueType {
                callee: decl.id().owner.to_string(),
                param: decl.name().to_string(),
                expected: expected.to_string(),
                found: generator.meta().to_string(),
            });
        }
        let argument = TyArg::Val(self.converted_generator(generator, expected)?.into_value());
        // A generator is an attribute, never a runtime value.
        if let ParamArg::Value(expr) = arg {
            self.record_at(expr.source_span()).argument_attribute = Some(argument.clone());
        }
        Ok(argument)
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
    /// generator converts by signature, names aside. A result naming one of
    /// its own type parameters is that binder; a result naming a trait is
    /// the trait used as a type, the type of a type a generator yields.
    fn generator_type(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        result: &SourceType,
    ) -> Result<Ty, TypeError> {
        let mut decls = Vec::new();
        for (slot, param) in params.iter().enumerate() {
            let id = mojito_types::param_expr::ParamId::new(GENERATOR_TYPE_OWNER, slot);
            let value_ty = match (&param.value_type, param.bounds.as_slice()) {
                (Some(value_type), _) => Some(self.resolve_ty_from_anno(value_type)?),
                (None, [only]) => scalar_type_name(only).or_else(|| {
                    self.structs
                        .contains_key(only)
                        .then(|| TyKind::Struct(only.clone(), Vec::new()).into())
                }),
                _ => None,
            };
            let (name, infer_only, variadic) = (
                param.name.clone(),
                param.infer_only,
                param.name.starts_with('*'),
            );
            if let Some(ty) = value_ty {
                decls.push(ParamDecl::Value {
                    id,
                    name,
                    ty: ParamDecl::value_list_ty(ty, variadic),
                    default: None,
                    callable_default: None,
                    infer_only,
                    variadic,
                    constraints: Vec::new(),
                });
                continue;
            }
            for bound in &param.bounds {
                self.check_trait_name(bound)?;
            }
            decls.push(ParamDecl::Type {
                id,
                name,
                bounds: param.bounds.clone(),
                trait_param: None,
                callable_bound: None,
                default: None,
                infer_only,
                variadic,
                constraints: Vec::new(),
            });
        }
        let result = match result {
            SourceType::Named(name, args) if args.is_empty() => {
                match decls.iter().find(|decl| decl.name() == name) {
                    Some(ParamDecl::Type { id, bounds, .. }) => TyKind::Param {
                        binder: mojito_types::param_expr::ParamRef {
                            id: id.clone(),
                            name: name.as_str().into(),
                        },
                        bounds: bounds.clone(),
                        callable_bound: None,
                    }
                    .into(),
                    _ if !self.structs.contains_key(name)
                        && !self.comptime_aliases.contains_key(name)
                        && self.check_trait_name(name).is_ok() =>
                    {
                        TyKind::Trait(vec![name.clone()]).into()
                    }
                    _ => self.resolve_ty_from_anno(result)?,
                }
            }
            _ => self.resolve_ty_from_anno(result)?,
        };
        Ok(Ty::generator(decls, result))
    }

    /// The source spelling `StringLiteral`: upstream parameterizes the type
    /// by the literal's value, so the bare spelling is concrete only as a
    /// whole parameter annotation, where the value infers per call.
    pub(super) fn bare_string_literal(&self) -> Result<Ty, TypeError> {
        if self.bare_string_literal_parameter.get() {
            Ok(Ty::string_literal())
        } else {
            Err(TypeError::Unsupported(
                "'StringLiteral[_]' is not concrete, use '[]' to bind missing parameters"
                    .to_string(),
            ))
        }
    }

    pub(super) fn resolve_ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        // Nesting depth: the outermost application of an annotation may leave
        // its origin slots to inference where its position allows; an
        // application nested as a type argument (`List[RefBox]`) is not
        // concrete without them, as at the pin.
        let depth = self.annotation_depth.get();
        self.annotation_depth.set(depth + 1);
        let resolved = self.resolve_ty_from_anno_at_depth(ty);
        self.annotation_depth.set(depth);
        resolved
    }

    /// The type `name[args]` denotes, beside what each of its bracket
    /// arguments bound to, as an annotation resolves it.
    pub(super) fn applied_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        let depth = self.annotation_depth.get();
        self.annotation_depth.set(depth + 1);
        let resolved = self
            .reflected_type_annotation(&SourceType::Named(name.to_string(), args.to_vec()))
            .and_then(|reflected| match reflected {
                Some(ty) => Ok((ty, vec![None; args.len()])),
                None => self.applied_type_at_depth(name, args),
            });
        self.annotation_depth.set(depth);
        resolved
    }

    fn resolve_ty_from_anno_at_depth(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        if let Some(reflected) = self.reflected_type_annotation(ty)? {
            return Ok(reflected);
        }
        Ok(match ty {
            SourceType::Int => Ty::int(),
            SourceType::UInt => Ty::uint(),
            SourceType::Bool => Ty::bool(),
            SourceType::StringLiteral => self.bare_string_literal()?,
            SourceType::ClosedStringLiteral => Ty::string_literal(),
            SourceType::TypeOf(operand) => self.type_of_operand(operand)?,
            SourceType::Float64 => Ty::float64(),
            SourceType::None => Ty::none(),
            SourceType::Generator { params, result } => self.generator_type(params, result)?,
            SourceType::Func {
                type_params,
                params,
                ret,
                thin,
                capturing,
                raises,
                raises_type,
                where_clauses,
            } => {
                // Clauses are compiled onto the anonymous contract's decls by
                // `lower_anonymous_callable_type`, which strips them before
                // resolving here. Any clause still present is on a plain
                // function-type annotation, where it has no binder to
                // constrain.
                if !where_clauses.is_empty() {
                    return Err(TypeError::Unsupported(
                        "a 'where' clause on a function type is only supported on a \
                         compile-time callable parameter bound with def[...] parameters"
                            .to_string(),
                    ));
                }
                let environment =
                    self.lower_callable_environment(type_params, *thin, capturing.as_ref())?;
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
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let variadic = function_params
                    .iter()
                    .find(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Variadic)
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .transpose()?;
                let kw_variadic = function_params
                    .iter()
                    .find(|parameter| parameter.kind == mojito_ast::ast::ParamKind::KwVariadic)
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .transpose()?;
                let (return_type, ref_return) = match &**ret {
                    SourceType::Ref { referent, origin } => (
                        self.resolve_ty_from_anno(referent)?,
                        Some(Box::new(self.lower_callable_ref_sig(
                            origin.as_ref().ok_or_else(|| {
                                TypeError::Unsupported(
                                    "reference return requires an origin".to_string(),
                                )
                            })?,
                            type_params,
                            &regular,
                        )?)),
                    ),
                    return_type => (self.resolve_ty_from_anno(return_type)?, None),
                };
                TyKind::Func(Box::new(FuncTy {
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
                            Some(error) => self.resolve_ty_from_anno(error)?,
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
                    ref_params: self.lower_callable_ref_param_sigs(type_params, &regular)?,
                    ref_return,
                    transfers: TransferSet::default(),
                }))
                .into()
            }
            SourceType::Ref { referent, origin } => {
                let spec = origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported(
                        "reference-valued fields require an explicit origin".to_string(),
                    )
                })?;
                let [origin_expr] = spec.as_slice() else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields currently require one origin parameter"
                            .to_string(),
                    ));
                };
                let Some(origin_name) = super::origins::origin_binder_name(origin_expr) else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields require a named origin parameter".to_string(),
                    ));
                };
                let origin_name = origin_name.to_string();
                if origin_name.ends_with("UnsafeAnyOrigin") {
                    return Err(TypeError::Unsupported(
                        "an UnsafeAnyOrigin reference cannot be hidden in a stored reference                          field"
                            .to_string(),
                    ));
                }
                if origin_name == "ImmUntrackedOrigin" {
                    return Ok(TyKind::Ref(mojito_types::origin::RefTy {
                        referent: self.resolve_ty_from_anno(referent)?,
                        origin: mojito_types::origin::Origin::Untracked { mutable: false },
                        mutability: mojito_types::origin::Mutability::Immutable,
                    })
                    .into());
                }
                let (index, parameter) = self
                    .enclosing_type_params
                    .iter()
                    .enumerate()
                    .find(|(_, parameter)| {
                        parameter.name == *origin_name && parameter.bounds.as_slice() == ["Origin"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(origin_name.clone()))?;
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => mojito_types::origin::Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => mojito_types::origin::Mutability::Immutable,
                    _ => mojito_types::origin::Mutability::Param(
                        mojito_types::origin::OriginParamId(index as u32),
                    ),
                };
                TyKind::Ref(mojito_types::origin::RefTy {
                    referent: self.resolve_ty_from_anno(referent)?,
                    origin: mojito_types::origin::Origin::Param(
                        mojito_types::origin::OriginParamId(index as u32),
                    ),
                    mutability,
                })
                .into()
            }
            // A bare name may be an in-scope type parameter (a generic `def`'s
            // `T`) or a struct type, optionally applied to parameter arguments.
            // A head that names no type may name a compile-time value, whose
            // subscript is a type (`tl[1]` over a `TypeList` value).
            SourceType::Named(name, args) => {
                return match self.applied_type_at_depth(name, args) {
                    Err(TypeError::UnknownType(unknown)) if unknown == *name => {
                        self.value_headed_type(name, args)
                    }
                    resolved => resolved.map(|(ty, _)| ty),
                };
            }
            // `Self.T` — one of the enclosing struct's *type* parameters (a value
            // parameter is not a type, so `Self.n` in type position is an error).
            SourceType::SelfParam(name) => {
                // A concrete `Self` binds the struct's parameters: `Self.T`
                // is `Int` while `Self` is `Optional[Int]`.
                if let Some(TyKind::Struct(_, arguments)) = self.self_ty.as_ref().map(Ty::kind)
                    && arguments.len() >= self.self_decls.len()
                    && let Some(index) = self
                        .self_decls
                        .iter()
                        .position(|d| d.name() == name && matches!(d, ParamDecl::Type { .. }))
                    && let Some(TyArg::Ty(bound)) = arguments.get(index)
                    && !matches!(bound.kind(), TyKind::Param { .. })
                {
                    return Ok(bound.clone());
                }
                match self
                    .self_decls
                    .iter()
                    .find(|d| d.name() == name)
                    .and_then(type_parameter)
                {
                    Some(parameter) => parameter,
                    None => return self.associated_type_for_self(name),
                }
            }
            // Bare `Self` — the enclosing struct type or a trait's abstract Self.
            // Value-parameterized structs carry their symbolic `CtValue::Expr`
            // arguments here; specialization bakes them out.
            SourceType::SelfType => match &self.self_ty {
                Some(ty) => ty.clone(),
                None => return Err(TypeError::UnknownSelfParam("Self".to_string())),
            },
            // Upstream's `Indexer` requirement returns `__mlir_type.index`;
            // the VM represents that index as `Int`. `!kgen.type` is the
            // metatype of every type, and `!kgen.string` a `StringLiteral`'s
            // parameter type. Every other MLIR type spelling is outside the
            // subset.
            SourceType::Assoc { base, name, args }
                if matches!(base.as_ref(), SourceType::Named(base, base_args)
                    if base == "__mlir_type" && base_args.is_empty()) =>
            {
                match (name.as_str(), args.is_empty()) {
                    ("index", true) => Ty::int(),
                    (name, true) if let Some(ty) = mojito_types::types::kgen_type_named(name) => ty,
                    _ => return Err(unsupported_mlir_type()),
                }
            }
            SourceType::Assoc { base, name, args } => self.member_type(base, name, args)?,
            // A compile-time value is a type only through a subscript, as at
            // the pin ("expected a type, not a value").
            SourceType::Value(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "a type".to_string(),
                    found: "a value".to_string(),
                    context: "type annotation".to_string(),
                });
            }
            // `TypeList[Trait=AnyType, Ts.values]()[i]`: the value's
            // `__getitem_param__[i]`, the path its explicit spelling takes.
            SourceType::IndexedProjection { base, index }
                if let SourceType::Value(value) = base.as_ref() =>
            {
                let ty = self.compile_time_value_type(value)?;
                self.parameterized_assoc_application(
                    &ty,
                    "__getitem_param__",
                    &[mojito_ast::ast::ParamArg::Value(index.as_ref().clone())],
                )?
                .ok_or_else(|| TypeError::NotIndexable(ty.to_string()))?
            }
            // `Self.Ts[index]` (or a `def`'s own `Ts[index]`) while the pack
            // is still a parameter: the dependent element type.
            SourceType::IndexedProjection { base, index }
                if let Some(pack) = self.unbound_pack_type(base) =>
            {
                self.pack_element_type(&pack, index)?
            }
            // `Self.values[idx]` over the struct's own raw type list, in the
            // bundled `variadics.mojo` alone: the stand-in for upstream's
            // `#kgen.param_list.get` that `TypeList.__getitem_param__` spells.
            SourceType::IndexedProjection { base, index }
                if let SourceType::SelfParam(name) = base.as_ref()
                    && super::overload_support::is_bundled_variadics_source(
                        index.source_span().source.as_deref(),
                    )
                    && let Some(raw) = self.self_raw_list(name)
                    && raw.meta() == &mojito_types::param_expr::MetaTy::type_list() =>
            {
                let index = self.compile_dependent_ct_expr(index)?;
                self.param_context
                    .list_get(&raw, &index)
                    .map(mojito_types::types::DependentType::resolve)
                    .map_err(super::param_error)?
            }
            SourceType::IndexedProjection { base, index } => {
                // A parameterized associated-type application such as
                // `Self.IteratorType[origin_of(self)]` is spelled like a dependent
                // index but names a parameterized associated member; resolve it as
                // an application rather than compile-time sequence indexing.
                let application = match base.as_ref() {
                    // `Self.IteratorType[..]` — the abstract trait `Self` in a trait
                    // method, or the concrete struct when a conformer spells the
                    // application as its own return type. Use the concrete `Self`
                    // (a registered struct) when available so the member resolves
                    // concretely; otherwise the trait's abstract `Self`.
                    SourceType::SelfParam(name) => {
                        let base = match &self.self_ty {
                            Some(ty) if matches!(ty.kind(), TyKind::Struct(..)) => ty.clone(),
                            _ => Ty::self_type(),
                        };
                        self.parameterized_assoc_application(
                            &base,
                            name,
                            &[mojito_ast::ast::ParamArg::Value(index.as_ref().clone())],
                        )?
                    }
                    // `C.IteratorType[..]` — a bounded type parameter or other base.
                    // If the base does not name a type (e.g. `values.element_types`
                    // where `values` is a value binding), this is not an
                    // associated-type application; fall through to dependent
                    // sequence indexing instead of surfacing a "not a type" error.
                    SourceType::Assoc {
                        base: inner, name, ..
                    } => self.member_index_application(inner, name, index)?,
                    _ => None,
                };
                if let Some(applied) = application {
                    applied
                } else {
                    let elements = self.dependent_type_sequence(base)?;
                    let index = self.compile_dependent_ct_expr(index)?;
                    self.resolve_dependent_index(elements, &index, &HashMap::new())?
                }
            }
        })
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
            TyKind::Struct(sname, _) => self
                .structs
                .get(sname)
                .and_then(|info| info.parameterized_associated.get(name))
                .map(|member| member.params.clone()),
            _ => None,
        }
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
        // Carry the application arguments in the checked type. The base here is
        // still abstract (`Self` or a bounded parameter). An `origin_of(self)`
        // argument in a trait method's abstract signature has no bound `self`
        // place; it lowers to the symbolic `Origin::SelfParam`, resolved to the
        // concrete receiver origin once the base is a conforming struct. Concrete
        // substitution and per-kind argument validation happen there.
        let arguments = explicit
            .iter()
            .zip(args)
            .map(|(param, arg)| self.lower_assoc_application_arg(param, arg))
            .collect::<Result<Vec<_>, _>>()?;
        // A concrete struct base instantiates the member here and now (a conformer
        // spelling `Self.IteratorType[origin_of(self)]` as its own return type):
        // resolve it through the same path as a non-indexed struct-based
        // application. An abstract base (`Self` or a bounded parameter) stays
        // symbolic until the base becomes a conforming struct.
        if matches!(base_ty.kind(), TyKind::Struct(..)) {
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
    fn member_type(
        &self,
        base: &SourceType,
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

    /// The type a member read's base names, beside the struct's name when
    /// the base is a bare parameterized struct, read as its unbound
    /// generator ([`Self::unbound_generator`]).
    fn member_base<'a>(&self, base: &'a SourceType) -> Result<(Ty, Option<&'a str>), TypeError> {
        if let SourceType::Named(name, args) = base
            && args.is_empty()
            && let Some(generator) = self.unbound_generator(name)
        {
            return Ok((generator, Some(name)));
        }
        self.ty_from_anno(base).map(|ty| (ty, None))
    }

    /// `base.name[args]` over a whole bracket list (`S[Int].Of[Bool]`,
    /// `M.Pair[1, 2]`). A struct's parameterized type member binds the list
    /// through the contract an alias application uses (keywords, defaults,
    /// bounds); an abstract base lowers each argument at its declared
    /// parameter.
    fn member_application(
        &self,
        base_ty: &Ty,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        if let TyKind::Struct(struct_name, _) = base_ty.kind()
            && let Some(member) = self
                .structs
                .get(struct_name)
                .and_then(|info| info.parameterized_associated.get(name))
            && let MemberTemplate::Type { decls, .. } = &member.template
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
                    .map(|index| bound.arguments[index].clone())
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
        inner: &SourceType,
        name: &str,
        index: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        let Ok((base_ty, generator)) = self.member_base(inner) else {
            return Ok(None);
        };
        let infers = self
            .parameterized_assoc_params(&base_ty, name)
            .is_some_and(|params| params.iter().any(|param| param.infer_only));
        let applied = if infers {
            let argument = index.type_spelling().map_or_else(
                || mojito_ast::ast::ParamArg::Value(index.clone()),
                mojito_ast::ast::ParamArg::Type,
            );
            Some(self.member_application(&base_ty, name, &[argument])?)
        } else {
            self.parameterized_assoc_application(
                &base_ty,
                name,
                &[mojito_ast::ast::ParamArg::Value(index.clone())],
            )?
        };
        applied
            .map(|applied| self.generator_member_type(generator, name, applied))
            .transpose()
    }

    /// Lower one application argument of a parameterized associated type into a
    /// checked `TyArg`, dispatched by the declared parameter's kind: an origin
    /// parameter takes an `origin_of(...)`/builtin origin, a value parameter a
    /// compile-time value, and a type parameter a type.
    fn lower_assoc_application_arg(
        &self,
        param: &mojito_ast::ast::TypeParam,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use mojito_ast::ast::ParamArg;
        if is_origin_parameter(param) {
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
            ParamArg::Type(source)
                if is_value_parameter(param)
                    && let Some(read) = self.type_spelled_member_read(source) =>
            {
                return Ok(TyArg::Val(self.eval_associated_ct(&read)?));
            }
            ParamArg::Type(_) if is_value_parameter(param) => {
                return Err(TypeError::TypeMismatch {
                    expected: "a value".to_string(),
                    found: "a type".to_string(),
                    context: format!("associated value parameter '{}'", param.name),
                });
            }
            ParamArg::Type(ty) => return self.ty_from_anno(ty).map(TyArg::Ty),
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
        if matches!(param.bounds.as_slice(), [only] if only == TYPE_OF_ALL_TYPES)
            && let Some(ty) = self.trait_type_arg(argument)
        {
            return ty.map(TyArg::Ty);
        }
        // A type parameter: a bare identifier or a type-shaped member chain
        // (`M.KGENTypeType`) names a type; otherwise the argument must
        // evaluate to a compile-time type value.
        let ty = match &arg.kind {
            ExprKind::Identifier(id) => {
                self.ty_from_anno(&SourceType::Named(id.clone(), Vec::new()))?
            }
            ExprKind::Member { .. } if let Some(spelled) = arg.type_spelling() => {
                self.ty_from_anno(&spelled)?
            }
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
    ) -> Result<(Ty, BracketAttributes), TypeError> {
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
        // that spelling when it selects one unique registered alias.
        if name.starts_with("__module$") {
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
        if let Some(info) = self.structs.get(name) {
            let decls = info.decls.clone();
            let source_params = info.source_params.clone();
            let BoundParameters {
                arguments,
                attributes,
                ..
            } = self.resolve_struct_use_args(name, &decls, &source_params, args, &[], &[])?;
            return Ok((Self::struct_instance_type(name, arguments), attributes));
        }
        if matches!(
            name,
            "UnsafePointer" | "Pointer" | "MutPointer" | "ImmPointer"
        ) {
            return self.pointer_type(name, args);
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
                .ty_from_anno(&SourceType::SelfParam(name.to_string()))
                .map(|ty| (ty, Vec::new()));
        }
        // A `def`'s own `Ts[index]` while the pack is still a
        // parameter: the dependent element type.
        if let [mojito_ast::ast::ParamArg::Value(index)] = args
            && let Some(pack) =
                self.unbound_pack_type(&SourceType::Named(name.to_string(), Vec::new()))
        {
            return self
                .pack_element_type(&pack, index)
                .map(|ty| (ty, vec![None]));
        }
        Err(TypeError::UnknownType(name.to_string()))
    }

    /// The type a `Named` annotation denotes when its head names no type:
    /// the expression it spells (the head, or its subscript by the bracket
    /// arguments), built as upstream's `emitExprType` builds any annotation,
    /// so a compile-time value's subscript is its `__getitem_param__`
    /// element and a value alone is "expected a type". The spelling is asked
    /// first, so a local value never shadows a type of its name here. A head
    /// that names nothing at all stays an unknown type.
    fn value_headed_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        use mojito_ast::ast::{ParamArg, SubscriptArg};
        let unknown = || TypeError::UnknownType(name.to_string());
        let operand = |arg: &ParamArg| match arg {
            ParamArg::Value(value) => Some(value.clone()),
            ParamArg::Type(ty) => ty.expression_spelling(mojito_common::token::Span::default()),
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

    /// Upstream's `_MLIR.KGENParamListType[T]` body,
    /// ``__mlir_type[`!kgen.param_list<`, T, `>`]``: the raw parameter list
    /// of `T`. Every other bracketed MLIR type spelling is outside the subset.
    fn mlir_param_list_type(&self, args: &[mojito_ast::ast::ParamArg]) -> Result<Ty, TypeError> {
        use mojito_ast::ast::ParamArg;
        let delimiter = |arg: &ParamArg, text: &str| {
            matches!(arg, ParamArg::Value(Expr { kind: ExprKind::Identifier(spelled), .. })
                if spelled == text)
        };
        let element = match args {
            [open, element, close]
                if delimiter(open, "!kgen.param_list<") && delimiter(close, ">") =>
            {
                element
            }
            _ => return Err(unsupported_mlir_type()),
        };
        let element = match self.trait_type_arg(element) {
            Some(element) => element?,
            None => match element {
                ParamArg::Type(ty) => self.ty_from_anno(ty)?,
                ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))?,
                _ => return Err(unsupported_mlir_type()),
            },
        };
        Ok(TyKind::ParamList(element).into())
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
            ParamArg::Type(SourceType::Named(name, args)) if args.is_empty() => {
                return trait_binder(name).or_else(|| {
                    (name != TYPE_OF_ALL_TYPES
                        && self.check_trait_name(name).is_ok()
                        && self.lookup(name).is_none())
                    .then(|| Ok(TyKind::Trait(vec![name.clone()]).into()))
                });
            }
            ParamArg::Type(SourceType::SelfParam(param)) => {
                return self.self_param_value(param).and_then(|value| match value {
                    CtValue::Trait(traits) => Some(Ok(TyKind::Trait(traits).into())),
                    CtValue::Expr(expr) if value.is_trait_valued() => {
                        Some(Ok(DependentType::resolve(expr)))
                    }
                    _ => None,
                });
            }
            ParamArg::Type(_) => return None,
        };
        if let Some(traits) = self.trait_value(expr) {
            return Some(traits.map(|traits| TyKind::Trait(traits).into()));
        }
        match &expr.kind {
            ExprKind::Identifier(name) => trait_binder(name),
            _ => None,
        }
    }

    /// The type a compiler-known name applies to `args` (`Never`, `SIMD`,
    /// `Scalar`, the compiler-private storage types, a type parameter in
    /// scope), beside its bracket attributes; `None` for any other name.
    fn builtin_applied_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<(Ty, BracketAttributes)>, TypeError> {
        let existential_trait = args.first().and_then(|argument| match argument {
            mojito_ast::ast::ParamArg::Type(SourceType::Named(trait_name, trait_args))
                if trait_args.is_empty() =>
            {
                Some(trait_name)
            }
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(trait_name),
                ..
            }) => Some(trait_name),
            _ => None,
        });
        if name == "__mlir_type" {
            return self
                .mlir_param_list_type(args)
                .map(|ty| Some((ty, vec![None; args.len()])));
        }
        if name == "Some"
            && args.len() == 1
            && let Some(trait_name) = existential_trait
            && (COMPILER_TRAITS.contains(&trait_name.as_str())
                || self.traits.contains_key(trait_name)
                || self.predeclared_traits.contains(trait_name))
        {
            return Ok(Some((
                TyKind::Param {
                    binder: synthetic_binder(&format!("Some[{trait_name}]")),
                    bounds: vec![trait_name.clone()],
                    callable_bound: None,
                }
                .into(),
                vec![None],
            )));
        }
        if name == "Never" && args.is_empty() {
            return Ok(Some((Ty::never(), Vec::new())));
        }
        if name == "NoneType" && args.is_empty() {
            return Ok(Some((Ty::none(), Vec::new())));
        }
        // Upstream compatibility alias: `StringSlice` resolves to the
        // canonical `StringSpan` view (never emitted; diagnostics and
        // display always spell `StringSpan`). Resolved at the same
        // nesting depth, so a bare `StringSlice` parameter keeps the
        // origin inference a bare `StringSpan` gets.
        if name == "StringSlice" && self.structs.contains_key("StringSpan") {
            return self.applied_type_at_depth("StringSpan", args).map(Some);
        }
        if name == "StringLiteral" && args.is_empty() {
            return self.bare_string_literal().map(|ty| Some((ty, Vec::new())));
        }
        if args.is_empty()
            && let Some(parameter) = self.lookup_tparam(name)
        {
            return Ok(Some((parameter, Vec::new())));
        }
        // Annotations parse the primitive spellings to dedicated
        // `SourceType` variants; a `Named` primitive reaches this arm
        // only from an expression-derived type position (a comptime
        // alias or associated-member body).
        if args.is_empty() {
            match name {
                "Int" => return Ok(Some((Ty::int(), Vec::new()))),
                "UInt" => return Ok(Some((Ty::uint(), Vec::new()))),
                "Bool" => return Ok(Some((Ty::bool(), Vec::new()))),
                "Float64" => return Ok(Some((Ty::float64(), Vec::new()))),
                _ => {}
            }
        }
        // SIMD vector types and their fixed-width scalar aliases.
        if let Some(dtype) = Dtype::from_scalar_alias(name) {
            if !args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: args.len(),
                });
            }
            return Ok(Some((simd_ty(dtype, 1), Vec::new())));
        }
        if name == "SIMD" {
            return self.simd_type(args).map(Some);
        }
        if name == "Scalar" {
            if args.len() != 1 {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let dtype = self.dtype_from_arg(&args[0])?;
            let attribute = dtype_attribute(&args[0], &dtype);
            return Ok(Some((
                simd_of(dtype, SimdWidth::Known(1))?,
                vec![attribute],
            )));
        }
        if name == "$pack" {
            return self
                .tuple_element_types(args)
                .map(|(elements, attributes)| {
                    Some((TyKind::RuntimePack(elements).into(), attributes))
                });
        }
        if name == "__RuntimeTuple" {
            return self
                .tuple_element_types(args)
                .map(|(elements, attributes)| Some((TyKind::Tuple(elements).into(), attributes)));
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
            let (mut elements, attributes) = self.tuple_element_types(args)?;
            let element = elements.remove(0);
            return Ok(Some((
                TyKind::Struct(name.to_string(), vec![TyArg::Ty(element)]).into(),
                attributes,
            )));
        }
        if name == "_" && args.is_empty() {
            return Ok(Some((Ty::infer(), Vec::new())));
        }
        if name == "Error" && args.is_empty() {
            return Ok(Some((Ty::error(), Vec::new())));
        }
        if name == "DType" && args.is_empty() && !self.structs.contains_key(name) {
            return Ok(Some((Ty::dtype(), Vec::new())));
        }
        Ok(None)
    }

    /// Resolve a struct application's explicit compile-time arguments,
    /// accepting origin arguments in the slots the declaration's raw
    /// parameter list spells (`decls` erases Origin parameters, so
    /// `resolve_use_params` alone cannot see them). The resolved arguments
    /// are the binder prefix followed by the origin tail: origin arguments
    /// are part of the struct's checked identity, and a slot the application
    /// leaves to inference is `Origin::Unbound`. An application supplying
    /// exactly the non-origin explicit count omits the origin slots entirely.
    pub(super) fn resolve_struct_use_args(
        &self,
        name: &str,
        decls: &[ParamDecl],
        source_params: &[mojito_ast::ast::TypeParam],
        args: &[mojito_ast::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<BoundParameters, TypeError> {
        let partitioned = self.partition_struct_origin_args(name, source_params, args)?;
        let BoundParameters {
            subst,
            mut arguments,
            attributes,
        } = self.resolve_use_params(name, decls, &partitioned.forwarded, patterns, actuals)?;
        arguments.extend(partitioned.tail_arguments());
        Ok(BoundParameters {
            subst,
            arguments,
            attributes: partitioned.brackets.scatter(attributes),
        })
    }

    /// Partition a struct application's explicit compile-time arguments into
    /// the origin slots the declaration's raw parameter list spells and the
    /// arguments the origin-erased binder sees (see
    /// [`Self::resolve_struct_use_args`], the annotation-side consumer; the
    /// constructor-call funnel in `infer_construction` is the other). Each
    /// resolvable origin argument is validated against its slot, becomes the
    /// slot's tail entry, and is returned so a constructor call can check the
    /// arguments binding that slot against it.
    pub(super) fn partition_struct_origin_args(
        &self,
        name: &str,
        source_params: &[mojito_ast::ast::TypeParam],
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<PartitionedStructArgs, TypeError> {
        // Only the outermost application of a collected storage annotation
        // reports its demands; nested applications resolve uncollected.
        let collecting = self.storage_origin_demands.take();
        let result = self.partition_struct_origin_args_uncollected(name, source_params, args);
        if let Some(mut demands) = collecting {
            if let Ok(partitioned) = &result {
                demands.extend(partitioned.explicit_origins.iter().map(|explicit| {
                    (
                        source_params[explicit.id.0 as usize].name.clone(),
                        explicit.origin.clone(),
                    )
                }));
            }
            self.storage_origin_demands.replace(Some(demands));
        }
        result
    }

    fn partition_struct_origin_args_uncollected(
        &self,
        name: &str,
        source_params: &[mojito_ast::ast::TypeParam],
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<PartitionedStructArgs, TypeError> {
        use mojito_ast::ast::ParamArg;
        use mojito_types::origin::Origin;
        let is_origin = super::is_origin_type_param;
        let slot_id = |param: &mojito_ast::ast::TypeParam| {
            mojito_types::origin::OriginParamId(
                source_params
                    .iter()
                    .position(|candidate| candidate.name == param.name)
                    .expect("explicit parameter comes from the source list") as u32,
            )
        };
        let explicit: Vec<&mojito_ast::ast::TypeParam> =
            source_params.iter().filter(|p| !p.infer_only).collect();
        let origin_slots = explicit.iter().filter(|p| is_origin(p)).count();
        // The tail in slot order: each slot's explicit origin, or unbound.
        let tail_of = |explicit_origins: &[ExplicitStructOrigin]| -> Vec<Origin> {
            explicit
                .iter()
                .filter(|param| is_origin(param))
                .map(|param| {
                    let id = slot_id(param);
                    explicit_origins
                        .iter()
                        .find(|origin| origin.id == id)
                        .map_or(Origin::Unbound, |origin| origin.origin.clone())
                })
                .collect()
        };
        let forward_all = || PartitionedStructArgs {
            forwarded: args.to_vec(),
            brackets: ForwardedBrackets::all(args.len()),
            explicit_origins: Vec::new(),
            tail: vec![Origin::Unbound; origin_slots],
        };
        // A struct applied as a type argument of another application
        // (`List[RefBox]`, `List[RefBox[_]]`) must bind its origin slots in
        // every position, as at the pin; only the outermost application may
        // leave them to inference. A compiler-generated declaration rebinds
        // already-checked spellings and keeps its context's leniency.
        let generated = self.generated_declaration.get()
            || self.self_ty.as_ref().is_some_and(
                |ty| matches!(ty.kind(), TyKind::Struct(name, _) if name.contains('$')),
            );
        let nested = self.annotation_depth.get() > 1 && !generated;
        let strict = if nested {
            super::StorageStrictness::Full
        } else {
            self.strict_storage_annotation.get()
        };
        // A storage annotation must bind explicit origin slots. A bare name
        // is not concrete unless an initializer can infer the whole
        // parameter list (`AllowBare`); a partial application names the
        // first omitted slot in either strict mode.
        if strict == super::StorageStrictness::Full && origin_slots > 0 && args.is_empty() {
            return Err(TypeError::NotConcrete(name.to_string()));
        }
        // No origin slots — or a variadic explicit list, whose positional
        // alignment the erased-decl binder owns — resolves as before.
        if origin_slots == 0 || args.is_empty() || explicit.iter().any(|p| p.name.starts_with('*'))
        {
            return Ok(forward_all());
        }
        let any_named = args.iter().any(|a| matches!(a, ParamArg::Named { .. }));
        if !any_named {
            let non_origin = explicit.len() - origin_slots;
            if args.len() == non_origin {
                if strict != super::StorageStrictness::Off {
                    let omitted = explicit
                        .iter()
                        .find(|param| is_origin(param))
                        .expect("origin_slots > 0");
                    return Err(TypeError::CannotInferParam {
                        name: name.to_string(),
                        param: omitted.name.clone(),
                    });
                }
                return Ok(forward_all());
            }
            if args.len() != explicit.len() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: explicit.len(),
                    got: args.len(),
                });
            }
            // Full positional supply. Every origin slot's argument must
            // RESOLVE as an origin for this interpretation to hold; a
            // non-origin argument in an origin slot means the application is
            // an ordinary over-application (e.g. an infer-only binder spelled
            // explicitly), which the erased-decl binder diagnoses.
            let mut resolved = Vec::with_capacity(origin_slots);
            let mut brackets = ForwardedBrackets::empty(args.len());
            for (position, (param, argument)) in explicit.iter().zip(args).enumerate() {
                if !is_origin(param) {
                    brackets.forward(position);
                    continue;
                }
                // Upstream's placeholder spellings (`_`, `...`) mark the slot
                // explicitly inferred: the application is complete (no
                // partial-application rejection), the slot resolves from the
                // initializer or call context, and concrete-storage positions
                // reject exactly like an omitted slot.
                if origin_placeholder(argument) {
                    if strict == super::StorageStrictness::Full {
                        return Err(TypeError::NotConcrete(name.to_string()));
                    }
                    let attribute = self.accept_origin_argument(
                        name,
                        param,
                        argument,
                        mojito_types::origin::Origin::Unbound,
                        None,
                    )?;
                    brackets.keep(position, attribute);
                    continue;
                }
                match self.resolve_origin_param_arg(argument) {
                    Ok(origin) => resolved.push((position, *param, argument, origin)),
                    Err(error) => {
                        // A signature annotation may name places only the body
                        // can resolve (`origin_of(self.entries)`): accept an
                        // origin-shaped argument syntactically and erase it.
                        if self.signature_origin_leniency.get()
                            && syntactic_origin_argument(argument)
                        {
                            let attribute = self.accept_origin_argument(
                                name,
                                param,
                                argument,
                                mojito_types::origin::Origin::Unbound,
                                None,
                            )?;
                            brackets.keep(position, attribute);
                            continue;
                        }
                        // An origin-shaped argument that does not resolve is
                        // an origin diagnostic, never an arity mismatch
                        // against the origin-erased binder.
                        if syntactic_origin_argument(argument) {
                            return Err(error);
                        }
                        return Ok(forward_all());
                    }
                }
            }
            let mut explicit_origins = Vec::with_capacity(resolved.len());
            for (position, param, argument, (origin, mutability)) in resolved {
                let attribute =
                    self.accept_origin_argument(name, param, argument, origin.clone(), mutability)?;
                brackets.keep(position, attribute);
                explicit_origins.push(ExplicitStructOrigin {
                    id: slot_id(param),
                    origin,
                    mutability,
                });
            }
            let forwarded: Vec<mojito_ast::ast::ParamArg> = explicit
                .iter()
                .zip(args)
                .filter(|(param, _)| !is_origin(param))
                .map(|(_, argument)| argument.clone())
                .collect();
            let tail = tail_of(&explicit_origins);
            return Ok(PartitionedStructArgs {
                forwarded,
                brackets,
                explicit_origins,
                tail,
            });
        }
        // Keyword spellings: extract named origin arguments wherever they
        // appear; everything else forwards to the erased-decl binder.
        let mut forwarded = Vec::with_capacity(args.len());
        let mut brackets = ForwardedBrackets::empty(args.len());
        let mut explicit_origins = Vec::new();
        let mut supplied_origins: Vec<&str> = Vec::new();
        for (position, argument) in args.iter().enumerate() {
            if let ParamArg::Named { name: keyword, .. } = argument
                && let Some(param) = explicit.iter().find(|p| p.name == *keyword && is_origin(p))
            {
                if origin_placeholder(argument) {
                    if strict == super::StorageStrictness::Full {
                        return Err(TypeError::NotConcrete(name.to_string()));
                    }
                    let attribute = self.accept_origin_argument(
                        name,
                        param,
                        argument,
                        mojito_types::origin::Origin::Unbound,
                        None,
                    )?;
                    brackets.keep(position, attribute);
                    supplied_origins.push(&param.name);
                    continue;
                }
                match self.resolve_origin_param_arg(argument) {
                    Ok((origin, mutability)) => {
                        let attribute = self.accept_origin_argument(
                            name,
                            param,
                            argument,
                            origin.clone(),
                            mutability,
                        )?;
                        brackets.keep(position, attribute);
                        supplied_origins.push(&param.name);
                        explicit_origins.push(ExplicitStructOrigin {
                            id: slot_id(param),
                            origin,
                            mutability,
                        });
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            brackets.forward(position);
            forwarded.push(argument.clone());
        }
        if strict != super::StorageStrictness::Off
            && let Some(omitted) = explicit
                .iter()
                .find(|param| is_origin(param) && !supplied_origins.contains(&param.name.as_str()))
        {
            return Err(TypeError::CannotInferParam {
                name: name.to_string(),
                param: omitted.name.clone(),
            });
        }
        let tail = tail_of(&explicit_origins);
        Ok(PartitionedStructArgs {
            forwarded,
            brackets,
            explicit_origins,
            tail,
        })
    }

    /// Resolve a return annotation: explicit origin slots must be applied
    /// there (a bare origin-slotted generic or a placeholder is not concrete,
    /// pin-attested), while an applied origin expression naming places only
    /// the body can resolve (`origin_of(self.entries)`) is accepted
    /// syntactically and erased.
    pub(super) fn resolve_return_annotation(
        &self,
        annotation: &mojito_ast::ast::SourceType,
    ) -> Result<Ty, TypeError> {
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
        annotation: &mojito_ast::ast::SourceType,
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
        annotation: &mojito_ast::ast::SourceType,
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
        annotation: &mojito_ast::ast::SourceType,
        strictness: super::StorageStrictness,
    ) -> Result<Ty, TypeError> {
        let saved = self.strict_storage_annotation.replace(strictness);
        let result = self.ty_from_anno(annotation);
        self.strict_storage_annotation.set(saved);
        result
    }

    /// Accept one resolved origin argument against its declared slot: a slot
    /// declared `Origin[mut=True]` rejects a provably immutable argument, and
    /// a value-shaped accepted argument records `origin` as its attribute —
    /// it is compile-time data on the construction, never a runtime value. A
    /// storage annotation is never lowered, so its arguments record nothing.
    /// Answers the bracket attribute the argument bound to.
    fn accept_origin_argument(
        &self,
        struct_name: &str,
        param: &mojito_ast::ast::TypeParam,
        argument: &mojito_ast::ast::ParamArg,
        origin: mojito_types::origin::Origin,
        mutability: Option<mojito_types::origin::Mutability>,
    ) -> Result<Option<TyArg>, TypeError> {
        let requires_mut = matches!(
            param.origin_mutability.as_ref().map(|e| &e.kind),
            Some(ExprKind::Bool(true))
        );
        if requires_mut
            && matches!(
                mutability,
                Some(mojito_types::origin::Mutability::Immutable)
            )
        {
            return Err(TypeError::TypeMismatch {
                expected: format!(
                    "a mutable origin for parameter '{}' of '{struct_name}'",
                    param.name
                ),
                found: "an immutable origin".to_string(),
                context: "explicit origin argument".to_string(),
            });
        }
        let mut value = argument;
        while let mojito_ast::ast::ParamArg::Named { value: inner, .. } = value {
            value = inner;
        }
        let attribute = TyArg::Origin(origin);
        match value {
            mojito_ast::ast::ParamArg::Value(expression) => {
                let recorded =
                    self.strict_storage_annotation.get() == super::StorageStrictness::Off;
                if recorded {
                    self.record_at(expression.source_span()).argument_attribute =
                        Some(attribute.clone());
                }
                Ok(recorded.then_some(attribute))
            }
            _ => Ok(Some(attribute)),
        }
    }

    /// Resolve the type-valued sequence at the base of an indexed type
    /// projection. A source value may expose such a sequence through an
    /// associated compile-time member; its runtime value is never inspected.
    pub(super) fn dependent_type_sequence(
        &self,
        projection: &SourceType,
    ) -> Result<Vec<Ty>, TypeError> {
        let SourceType::Assoc { base, name, .. } = projection else {
            return Err(TypeError::Unsupported(
                "dependent type indexing requires a type-valued associated member".to_string(),
            ));
        };
        let base_ty = match base.as_ref() {
            SourceType::Named(binding, arguments) if arguments.is_empty() => self
                .lookup(binding)
                .cloned()
                .map_or_else(|| self.ty_from_anno(base), Ok)?,
            _ => self.ty_from_anno(base)?,
        };

        // Public Tuple exposes its concrete element pack as `element_types`.
        // Use the checked nominal arguments, never its generated symbol text.
        if (name == "element_types" || name == "Ts")
            && let Some(elements) = tuple_elements(&base_ty)
        {
            return Ok(elements.into_iter().cloned().collect());
        }

        let TyKind::Struct(struct_name, arguments) = base_ty.kind() else {
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
        let substitution = struct_subst(&info.decls, arguments);
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
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        let context = &self.param_context;
        let bindings =
            mojito_types::param_expr::ParamBindings::from_named_values(context, parameters);
        // Selecting a type needs the index's value, so it is evaluated.
        context
            .replace(index, &bindings)
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
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        Ok(match ty.kind() {
            TyKind::Dependent(dependent) => match dependent.selection() {
                Some((elements, index)) => {
                    let elements = elements
                        .iter()
                        .map(|element| self.resolve_dependent_ty(element, parameters))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.resolve_dependent_index(elements, index, parameters)?
                }
                // An element of a pack that is still a parameter closes as
                // its pack and index bind.
                None if dependent.pack_element().is_some() && !parameters.is_empty() => {
                    let context = &self.param_context;
                    let bindings = mojito_types::param_expr::ParamBindings::from_named_values(
                        context, parameters,
                    );
                    // A pack bound to a list of types selects the indexed
                    // one, whether or not each is closed (`Tuple[T, Int]`
                    // over a binder `T`).
                    if let Some((list, index)) = dependent.pack_element()
                        && let Some(CtValue::Tuple(elements)) = list
                            .as_decl_ref()
                            .and_then(|pack| parameters.get(pack.name.trim_start_matches('*')))
                        && let Some(position) = context
                            .replace(index, &bindings)
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
                        .replace(dependent.expr(), &bindings)
                        .map(DependentType::resolve)
                        .map_err(param_error)?
                }
                None => ty.clone(),
            },
            // A symbolic lane dtype or width closes under the use's bindings
            // and re-canonicalizes: `Scalar[dt]` at `dt = DType.float64` is
            // `Float64`.
            TyKind::Simd { .. }
                if !parameters.is_empty() && mojito_types::types::is_symbolic(ty) =>
            {
                let context = &self.param_context;
                let bindings =
                    mojito_types::param_expr::ParamBindings::from_named_values(context, parameters);
                mojito_types::types::replace_parameters(context, ty, &bindings, 0)
                    .map_err(param_error)?
            }
            // A spread of a list computed from packs closes as they bind.
            TyKind::Struct(name, arguments)
                if !parameters.is_empty()
                    && let Some(list) = mojito_types::types::list_spread_argument(arguments) =>
            {
                let context = &self.param_context;
                let bindings =
                    mojito_types::param_expr::ParamBindings::from_named_values(context, parameters);
                let spread = context
                    .replace(list, &bindings)
                    .and_then(|list| mojito_types::types::spread_arguments(name, &list))
                    .map_err(param_error)?;
                ty.reusing(TyKind::Struct(name.clone(), spread))
            }
            TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => self.resolve_dependent_ty(ty, parameters).map(TyArg::Ty),
                        // A residual value argument closes under the use's
                        // bindings and re-folds: `Buf[n + 1]` at `n = 3` is
                        // `Buf[4]`.
                        TyArg::Val(value @ CtValue::Expr(_)) if !parameters.is_empty() => {
                            let context = &self.param_context;
                            let bindings =
                                mojito_types::param_expr::ParamBindings::from_named_values(
                                    context, parameters,
                                );
                            mojito_types::types::replace_value_parameters(context, value, &bindings)
                                .map(TyArg::Val)
                                .map_err(param_error)
                        }
                        TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                        TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                    })
                    .collect::<Result<Vec<_>, TypeError>>()?,
            )),
            TyKind::ComptimeList(element) => {
                TyKind::ComptimeList(self.resolve_dependent_ty(element, parameters)?).into()
            }
            TyKind::ParamList(element) => {
                TyKind::ParamList(self.resolve_dependent_ty(element, parameters)?).into()
            }
            TyKind::Tuple(elements) => TyKind::Tuple(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            TyKind::RuntimePack(elements) => TyKind::RuntimePack(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            TyKind::VariadicPack(element) => {
                TyKind::VariadicPack(self.resolve_dependent_ty(element, parameters)?).into()
            }
            TyKind::Variant(alternatives) => TyKind::Variant(
                alternatives
                    .iter()
                    .map(|alternative| self.resolve_dependent_ty(alternative, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            TyKind::Pointer { element, origin } => TyKind::Pointer {
                element: self.resolve_dependent_ty(element, parameters)?,
                origin: origin.clone(),
            }
            .into(),
            TyKind::Ref(reference) => TyKind::Ref(mojito_types::origin::RefTy {
                referent: self.resolve_dependent_ty(&reference.referent, parameters)?,
                origin: reference.origin.clone(),
                mutability: reference.mutability,
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
                        .map(|parameter| self.resolve_dependent_ty(parameter, parameters))
                        .collect::<Result<Vec<_>, _>>()?,
                    names: names.clone(),
                    ret: self.resolve_dependent_ty(ret, parameters)?,
                    required: required.clone(),
                    variadic: variadic
                        .as_ref()
                        .map(|parameter| self.resolve_dependent_ty(parameter, parameters))
                        .transpose()?,
                    kw_variadic: kw_variadic
                        .as_ref()
                        .map(|parameter| self.resolve_dependent_ty(parameter, parameters))
                        .transpose()?,
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    error: error
                        .as_ref()
                        .map(|error| self.resolve_dependent_ty(error, parameters))
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
                    .map(|candidate| self.resolve_dependent_ty(candidate, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .into(),
            _ => ty.clone(),
        })
    }

    pub(super) fn value_argument_environment(
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> HashMap<String, CtValue> {
        decls
            .iter()
            .zip(arguments)
            .filter_map(|(declaration, argument)| match argument {
                TyArg::Val(value) => Some((
                    declaration.name().trim_start_matches('*').to_string(),
                    value.clone(),
                )),
                TyArg::Ty(_) | TyArg::Origin(_) => None,
            })
            .collect()
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
                CtMemberReq::Value(_) | CtMemberReq::Generator { .. } => {
                    Err(TypeError::NoSuchAssociatedType {
                        object_type: "Self".to_string(),
                        member: name.to_string(),
                    })
                }
            };
        }
        let Some(self_ty) = &self.self_ty else {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        };
        if let TyKind::Struct(sname, _) = self_ty.kind()
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
            TyKind::Struct(sname, targs) => {
                let info = self
                    .structs
                    .get(sname)
                    .ok_or_else(|| TypeError::UnknownType(sname.clone()))?;
                // A parameterized associated member instantiated by a conforming
                // struct: substitute the application's arguments into its template.
                if let Some(member) = info.parameterized_associated.get(name) {
                    let member = member.clone();
                    let decls = info.decls.clone();
                    let targs = targs.clone();
                    return self
                        .resolve_parameterized_member(base, name, &member, &decls, &targs, args);
                }
                let value =
                    info.associated
                        .get(name)
                        .ok_or_else(|| TypeError::NoSuchAssociatedType {
                            object_type: base.to_string(),
                            member: name.to_string(),
                        })?;
                if let Some(constraints) = info.associated_constraints.get(name) {
                    let environment = ConstraintEnvironment::declared(&info.decls, targs);
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
                Ok(self.resolve_assoc_ty(&substitute_at(ty, info, targs)))
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
            _ => Err(TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            }),
        }
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
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        let template = match &alias.body {
            AliasBody::Type(template) => template,
            AliasBody::Predicate(_) => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is a Bool-valued comptime alias, not a type"
                )));
            }
            AliasBody::Value(_) => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' is a value comptime alias, not a type"
                )));
            }
        };
        let BoundParameters {
            arguments: tyargs,
            attributes,
            ..
        } = self.resolve_use_params(name, &alias.decls, args, &[], &[])?;
        let mut types = HashMap::new();
        let mut values = HashMap::new();
        let mut ids = mojito_types::param_expr::ParamBindings::new();
        for (decl, argument) in alias.decls.iter().zip(&tyargs) {
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
            // Origin parameters are rejected at alias declaration.
            origins: HashMap::new(),
        };
        // The symbolic template keeps canonical `Tuple[T, ...]`; a substituted
        // application must re-select the executable nominal implementation so
        // discovery can materialize it.
        Ok((
            self.resolve_assoc_ty(&substitute_assoc(&template, &bindings)),
            attributes,
        ))
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
        let MemberTemplate::Type { decls, template } = &member.template else {
            return Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: format!("'{base}.{name}' is a compile-time value member"),
            });
        };
        // The arguments are the explicit parameters' (an abstract trait
        // signature's application), or every parameter's when a bracket list
        // bound them all ([`Self::member_application`]).
        let every = args.len() == member.params.len();
        let supplies = |param: &mojito_ast::ast::TypeParam| every || !param.infer_only;
        let explicit = member.params.iter().filter(|p| !p.infer_only).count();
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
        for (decl, targ) in struct_decls.iter().zip(struct_targs) {
            if let (ParamDecl::Value { name, .. }, TyArg::Val(value)) = (decl, targ) {
                values.insert(name.clone(), value.clone());
            }
        }
        let mut ids = mojito_types::param_expr::ParamBindings::new();
        if let Some((pack, elements)) =
            super::generics::bound_pack_elements(struct_decls, struct_targs)
            && let Some(decl) = struct_decls
                .iter()
                .find(|decl| decl.name().trim_start_matches('*') == pack)
            && let Ok(list) = self.param_context.type_list(elements)
        {
            ids.bind(decl.id().clone(), list);
        }
        // The member's own explicit parameters concretize from the application:
        // a value binder by its identity, converted at its declared type as a
        // parameter binding converts (`0` at `idx: Int` is `Int(0)`). Origin
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
                    // Origin binders classify to no slot: the declaration
                    // is found by name.
                    let decl = decls.iter().find(|decl| decl.name() == param.name);
                    let value = match decl {
                        Some(ParamDecl::Value { ty, .. }) => value
                            .clone()
                            .materialize_as(ty)
                            .unwrap_or_else(|| value.clone()),
                        _ => value.clone(),
                    };
                    if let Some(decl) = decl
                        && let Some(bound) = self.bound_parameter_value(&value)
                    {
                        ids.bind(decl.id().clone(), bound);
                    }
                    values.insert(param.name.clone(), value);
                }
                TyArg::Origin(origin) => {
                    origins.insert((member.param_base + index) as u32, origin.clone());
                }
            }
        }
        self.validate_member_availability(
            &format!("{base}.{name}"),
            &member.availability,
            struct_decls,
            struct_targs,
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
                    .map(|decl| (decl.clone(), argument.clone()))
            })
            .unzip();
        let template = super::generics::expand_solved_packs(template, &packs, &elements);
        let bindings = AssocBindings {
            types,
            values,
            ids,
            origins,
        };
        Ok(self.resolve_assoc_ty(&substitute_assoc(&template, &bindings)))
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
            } => TyKind::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound
                    .as_ref()
                    .map(|bound| self.resolve_assoc_ty(bound)),
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
            TyKind::Struct(name, args) => {
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
            TyKind::Dependent(_) | TyKind::Simd { .. } => self.answer_concrete_members(ty),
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
            TyKind::Pointer { element, origin } => TyKind::Pointer {
                element: self.resolve_assoc_ty(element),
                origin: origin.clone(),
            }
            .into(),
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
    ) -> Result<TyArg, TypeError> {
        use mojito_ast::ast::ParamArg;
        match decl {
            // A trait is a type in KGEN, so `!kgen.type` binds one
            // (`_MLIR.KGENParamListType[Tr]`, `k[Copyable]()`).
            ParamDecl::Type { bounds, .. }
                if matches!(bounds.as_slice(), [only] if only == TYPE_OF_ALL_TYPES)
                    && let Some(ty) = self.trait_type_arg(arg) =>
            {
                let ty = ty?;
                if let ParamArg::Value(expr) = arg {
                    self.record_at(expr.source_span()).argument_attribute =
                        Some(TyArg::Ty(ty.clone()));
                }
                Ok(TyArg::Ty(ty))
            }
            ParamDecl::Type { name, bounds, .. } => {
                // A bare or parameterized type argument the parser encoded as a
                // value expression records the type as its attribute, only
                // once the argument resolves and satisfies its bounds: an
                // overload probe that rejects this declaration leaves no fact
                // behind for the declaration that wins.
                let (ty, value_span) = match arg {
                    ParamArg::Type(t) => (self.ty_from_anno(t)?, None),
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
                        (reflected?, Some(expression.source_span()))
                    }
                    // Any other expression is the type it denotes, built by
                    // the one builder; once it resolves it is the call's
                    // parameter attribute, never a runtime value.
                    ParamArg::Value(expression) => (
                        self.denoted_type(expression, &format!("type parameter '{name}'"))?,
                        Some(expression.source_span()),
                    ),
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
                if let Some(span) = value_span {
                    self.record_at(span).argument_attribute = Some(TyArg::Ty(ty.clone()));
                }
                Ok(TyArg::Ty(ty))
            }
            ParamDecl::Value { ty, .. } if let TyKind::AnyTrait(bound) = ty.kind() => {
                self.resolve_trait_param_arg(decl, bound, arg)
            }
            ParamDecl::Value { ty, .. } if matches!(ty.kind(), TyKind::Generator(_)) => {
                self.resolve_generator_param_arg(decl, ty, arg)
            }
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    // Function values are compile-time parameters in source, but
                    // deliberately remain runtime values in the VM ABI: MIR
                    // evaluates the parameter argument into a register and the
                    // call frame reifies it under `name`. `CtValue::Deferred` is only
                    // the erased generic-identity marker used by this resolver.
                    if matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) {
                        let actual = self.infer(expr)?;
                        // A closure's storage is a run-time value, and so is
                        // a capturing lambda's closure, as at the pin.
                        let environment = actual.callable().map(|func| &func.environment);
                        if matches!(expr.kind, ExprKind::Lambda { .. })
                            && environment
                                .and_then(CallableEnvironment::captures)
                                .is_some()
                        {
                            return Err(TypeError::CapturingLambdaParameter);
                        }
                        if matches!(environment, Some(CallableEnvironment::Storage(_))) {
                            return Err(TypeError::DynamicParameterValue);
                        }
                        if !self.value_coerces(&actual, ty) {
                            return Err(TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: actual.to_string(),
                                context: format!("callable-value parameter '{name}'"),
                            });
                        }
                        return Ok(TyArg::Val(CtValue::Deferred(decl.binder())));
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
                    let value = match self.eval_associated_ct(expr) {
                        Ok(value) => value,
                        Err(error) => match self
                            .lifted_application(expr)
                            .filter(|expression| expression.as_constant().is_none())
                        {
                            Some(expression) => CtValue::Expr(expression),
                            None => return Err(error),
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
                    } else if !coerces(&actual, ty) && !self.converts_to_parameter(expr, &value, ty)
                    {
                        return Err(TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: actual.to_string(),
                            context: format!("value parameter '{name}'"),
                        });
                    }
                    self.record_literal_materializations(expr, &actual, ty)?;
                    // A type list (`Ts.values`), a closed raw list of
                    // values, or a KGEN string has no run-time form.
                    let closed_value_list = matches!(&value, CtValue::Tuple(elements)
                        if !elements.iter().any(|element| matches!(element, CtValue::Type(_))));
                    if value.is_compile_time_only()
                        || (closed_value_list && matches!(ty.kind(), TyKind::ParamList(_)))
                    {
                        self.record_at(expr.source_span()).argument_attribute =
                            Some(TyArg::Val(value.clone()));
                    }
                    // A symbolic enclosing-scope parameter stays symbolic here;
                    // specialization bakes it out like the callable case above.
                    if matches!(value, CtValue::Expr(_)) {
                        return Ok(TyArg::Val(value));
                    }
                    let rendered = value.to_string();
                    let value =
                        value
                            .materialize_as(ty)
                            .ok_or_else(|| TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: rendered,
                                context: format!("value parameter '{name}'"),
                            })?;
                    Ok(TyArg::Val(value))
                }
                // `Self.n` names the enclosing struct's own value parameter:
                // symbolic while checking the template (exactly the bare
                // spelling `eval_associated_ct` resolves), bound to the
                // instance value inside a per-instantiation clone.
                ParamArg::Type(SourceType::SelfParam(param))
                    if matches!(self.self_param_ct_value(param), Some(CtValue::Expr(_))) =>
                {
                    self.self_param_value(param)
                        .map(TyArg::Val)
                        .ok_or_else(|| TypeError::UnknownSelfParam(param.clone()))
                }
                ParamArg::Type(source)
                    if let Some(read) = self.type_spelled_member_read(source) =>
                {
                    self.resolve_param_arg(decl, &ParamArg::Value(read))
                }
                // A nested application of a value alias (`Twice[Twice[5]]`)
                // parses as a type argument.
                ParamArg::Type(SourceType::Named(alias, alias_args))
                    if let Some(value) = self.value_alias_application(alias, alias_args) =>
                {
                    let value = value?;
                    let actual = value.meta().as_value().cloned().unwrap_or_else(Ty::error);
                    if !coerces(&actual, ty) {
                        return Err(TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: actual.to_string(),
                            context: format!("value parameter '{name}'"),
                        });
                    }
                    Ok(TyArg::Val(match value.as_constant() {
                        Some(closed) => closed.clone().materialize_as(ty).ok_or_else(|| {
                            TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: closed.to_string(),
                                context: format!("value parameter '{name}'"),
                            }
                        })?,
                        None => CtValue::Expr(value),
                    }))
                }
                ParamArg::Type(source)
                    if let Some(element) = self.display_element_argument(source) =>
                {
                    let element = element?;
                    let actual = element.meta().as_value().cloned().unwrap_or_else(Ty::error);
                    if !coerces(&actual, ty) {
                        return Err(TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: actual.to_string(),
                            context: format!("value parameter '{name}'"),
                        });
                    }
                    Ok(TyArg::Val(CtValue::Expr(element)))
                }
                // A bare parameter name, or `Self.<field>`, in a value slot.
                ParamArg::Type(ty) if let Some(error) = self.bare_or_field_in_value_slot(ty) => {
                    Err(error)
                }
                ParamArg::Type(_) => Err(TypeError::TypeMismatch {
                    expected: "a value".to_string(),
                    found: "a type".to_string(),
                    context: format!("value parameter '{name}'"),
                }),
                ParamArg::Named { value, .. } => self.resolve_param_arg(decl, value),
            },
        }
    }

    /// Record what each bracket argument of the application at `span`
    /// bound to, as its binder returned it: nothing when every argument is
    /// a run-time value.
    pub(super) fn record_bracket_attributes(
        &self,
        span: SourceSpan,
        attributes: BracketAttributes,
    ) {
        if attributes.iter().all(Option::is_none) {
            return;
        }
        self.record_at(span).parameter_attributes = Some(attributes);
    }

    /// Resolve the type in value position `expr`, `ty`, recording the type
    /// at it ([`mojito_checked::checked::NodeFacts::type_operand`]).
    pub(super) fn type_value(&self, expr: &Expr, ty: &SourceType) -> Result<Ty, TypeError> {
        let resolved = self.ty_from_anno(ty)?;
        self.record_at(expr.source_span()).type_operand = Some(resolved.clone());
        Ok(resolved)
    }

    /// Resolve `List[T]` from its single type argument.
    pub(super) fn list_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        if args.is_empty() {
            return Ok((list_type(Ty::infer()), Vec::new()));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "List".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let (element, attributes) = self.collection_type_arguments("List", args)?;
        // `List[T: AnyType]`: only a `!kgen.type` binder falls short.
        if !self.conforms_to(&element[0], "AnyType") {
            return Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: element[0].to_string(),
                trait_name: "AnyType".to_string(),
                reason: None,
            });
        }
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
            mojito_ast::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
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
    ) -> Result<(Ty, BracketAttributes), TypeError> {
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
        Ok((ty, types_attributes(args, &types)))
    }

    pub(super) fn dict_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
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
        Ok((ty, types_attributes(args, &types)))
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
        match info.decls.last()? {
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

    /// Resolve current Mojo's `Pointer[T, origin]` (also spelled through the
    /// deprecated `UnsafePointer` alias and the mutability-fixing `MutPointer`/
    /// `ImmPointer` aliases) or the one-argument `Pointer[T]` compatibility
    /// spelling, which resolves to the mutable untracked origin of heap
    /// allocations. The inferred mutability parameter is intentionally absent
    /// from the user-facing argument list.
    pub(super) fn pointer_type(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        if !matches!(args.len(), 1 | 2) {
            return Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let elem = match &args[0] {
            mojito_ast::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
            mojito_ast::ast::ParamArg::Value(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "a type".to_string(),
                    found: "a value".to_string(),
                    context: format!("{name} element type"),
                });
            }
            mojito_ast::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "named Tuple element arguments".to_string(),
                ));
            }
        };
        let origin = if args.len() == 1 {
            mojito_types::origin::PointerOrigin::Untracked {
                mutable: name != "ImmPointer",
            }
        } else if origin_placeholder(&args[1]) {
            // A placeholder origin (`ImmPointer[UInt8, _]`, upstream's raw
            // byte-pointer parameter) accepts any provenance at the alias's
            // fixed permission; the callee holds no loan. The bare spelling
            // (`Pointer[T, _]`) is a parameter whose origin — and so whose
            // `mut` — is inferred per call: the body cannot prove the
            // capability, so upstream rejects writes and mutable-demanding
            // forwards through it, exactly the immutable alias's reading.
            // Outside parameter position the bare placeholder is not
            // concrete (upstream's verdict).
            use mojito_types::origin::PointerOrigin;
            match name {
                "ImmPointer" => PointerOrigin::UnsafeAny { mutable: false },
                "MutPointer" => PointerOrigin::UnsafeAny { mutable: true },
                _ if self.resolving_parameter_annotation.get() => {
                    PointerOrigin::UnsafeAny { mutable: false }
                }
                _ => {
                    return Err(TypeError::Unsupported(format!(
                        "'{name}[{elem}, _]' is not concrete outside parameter position: a \
                         bare placeholder origin is inferred only for a parameter; spell \
                         ImmPointer[T, _] or MutPointer[T, _] here"
                    )));
                }
            }
        } else {
            self.pointer_origin_arg(&args[1])?
        };
        // The mutability-fixing aliases constrain the origin argument's
        // statically known permission.
        let required = match name {
            "MutPointer" => Some(true),
            "ImmPointer" => Some(false),
            _ => None,
        };
        if let (Some(required), Some(actual)) = (required, origin.statically_mutable())
            && required != actual
        {
            return Err(TypeError::TypeMismatch {
                expected: format!(
                    "an origin with {} permission",
                    if required { "mutable" } else { "immutable" }
                ),
                found: format!(
                    "{} origin",
                    if actual { "a mutable" } else { "an immutable" }
                ),
                context: format!("{name} origin"),
            });
        }
        let mut attributes = vec![bracket_attribute(&args[0], &TyArg::Ty(elem.clone()))];
        if let Some(argument) = args.get(1) {
            attributes.push(
                pointer_origin_attribute(&origin)
                    .and_then(|bound| bracket_attribute(argument, &TyArg::Origin(bound))),
            );
        }
        Ok((
            TyKind::Pointer {
                element: elem,
                origin,
            }
            .into(),
            attributes,
        ))
    }

    pub(super) fn pointer_origin_arg(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::origin::PointerOrigin, TypeError> {
        use mojito_types::origin::PointerOrigin;

        let constant = match argument {
            mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(name)) => {
                return self.enclosing_origin_param(name);
            }
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => name.as_str(),
            mojito_ast::ast::ParamArg::Type(SourceType::Named(name, arguments))
                if arguments.is_empty() =>
            {
                name.as_str()
            }
            mojito_ast::ast::ParamArg::Value(expression) => {
                return self.pointer_origin_expr(expression);
            }
            // `origin._subtree` in type-annotation position parses as an
            // associated member of the origin base; expression positions
            // route through `pointer_origin_expr` instead.
            mojito_ast::ast::ParamArg::Type(SourceType::Assoc { base, name, args })
                if name == "_subtree" && args.is_empty() =>
            {
                let origin =
                    self.pointer_origin_arg(&mojito_ast::ast::ParamArg::Type((**base).clone()))?;
                return append_subtree(origin);
            }
            // `Self.origin._get_owned_interior["tag"]` in type-annotation
            // position parses as an indexed projection over an associated
            // member of the origin parameter; expression positions route
            // through `pointer_origin_expr` instead.
            mojito_ast::ast::ParamArg::Type(SourceType::IndexedProjection { base, index }) => {
                let SourceType::Assoc {
                    base: origin_base,
                    name,
                    args,
                } = base.as_ref()
                else {
                    return Err(TypeError::TypeMismatch {
                        expected: "an interior origin projection".to_string(),
                        found: "an indexed type projection".to_string(),
                        context: "Pointer origin".to_string(),
                    });
                };
                if name != "_get_owned_interior" || !args.is_empty() {
                    return Err(TypeError::TypeMismatch {
                        expected: "'_get_owned_interior[\"tag\"]'".to_string(),
                        found: format!("'{name}'"),
                        context: "Pointer origin projection".to_string(),
                    });
                }
                let ExprKind::Str(tag) = &index.kind else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a compile-time tag string".to_string(),
                        found: "a non-string index".to_string(),
                        context: "interior origin projection".to_string(),
                    });
                };
                let origin = self.pointer_origin_arg(&mojito_ast::ast::ParamArg::Type(
                    (**origin_base).clone(),
                ))?;
                return append_interior_tag(origin, tag);
            }
            _ => {
                return Err(TypeError::TypeMismatch {
                    expected: "Self.origin or a concrete Origin value".to_string(),
                    found: "a non-origin parameter argument".to_string(),
                    context: "Pointer origin".to_string(),
                });
            }
        };
        match constant {
            "MutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: true }),
            "ImmUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: false }),
            "MutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: true }),
            "ImmUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: false }),
            "ImmStaticOrigin" => Ok(PointerOrigin::Static),
            // The pre-rename alias spellings were removed upstream (2026-08);
            // give them targeted migration diagnostics.
            "ImmutUntrackedOrigin" | "ImmutExternalOrigin" => Err(TypeError::Unsupported(format!(
                "'{constant}' was removed; use 'ImmUntrackedOrigin'"
            ))),
            "ImmutUnsafeAnyOrigin" => Err(TypeError::Unsupported(
                "'ImmutUnsafeAnyOrigin' was removed; use 'ImmUnsafeAnyOrigin'".to_string(),
            )),
            "StaticConstantOrigin" => Err(TypeError::Unsupported(
                "'StaticConstantOrigin' was removed; use 'ImmStaticOrigin'".to_string(),
            )),
            "ExternalOrigin" => Err(TypeError::Unsupported(
                "'ExternalOrigin' was removed; use 'UntrackedOrigin'".to_string(),
            )),
            "MutExternalOrigin" => Err(TypeError::Unsupported(
                "'MutExternalOrigin' was removed; use 'MutUntrackedOrigin'".to_string(),
            )),
            name => self
                .enclosing_origin_param(name)
                .map_err(|_| TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// Resolve an expression-shaped `Pointer` origin argument: an
    /// `._get_owned_interior["tag"]` projection over an origin parameter or
    /// `origin_of(place)` observation. The projected tags become the
    /// pointer's interior-generation domain — the marker that the pointer
    /// legally designates multiple elements.
    fn pointer_origin_expr(
        &self,
        expression: &Expr,
    ) -> Result<mojito_types::origin::PointerOrigin, TypeError> {
        use mojito_types::origin::{Mutability, Origin, PointerOrigin};

        // `ImmOrigin(o)`: `o`'s provenance, read-only.
        if let Some(inner) = super::origins::immutable_origin_cast(expression) {
            return self.pointer_origin_expr(inner?).map(immutable_capability);
        }
        if let Some((base, tag)) = super::origins::interior_origin_syntax(expression) {
            let origin = self.pointer_origin_expr(base)?;
            return append_interior_tag(origin, tag);
        }
        if let Some(base) = super::origins::subtree_origin_syntax(expression) {
            let origin = self.pointer_origin_expr(base)?;
            return append_subtree(origin);
        }
        match &expression.kind {
            // `Self.origin` spelled in expression position (a projection base).
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                self.enclosing_origin_param(field)
            }
            // A bare origin parameter name (function-head spelling).
            ExprKind::Identifier(name) => self.enclosing_origin_param(name),
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } if name == "origin_of"
                && kwargs.is_empty()
                && param_args.is_empty()
                && args.len() == 1 =>
            {
                // `origin_of(self)` stays symbolic whether or not a `self`
                // place is bound, so a declared return annotation and the
                // body's `unsafe_origin_cast` target resolve to one comparable
                // form; call sites rebase it onto the concrete receiver.
                if matches!(&args[0].kind, ExprKind::Identifier(name) if name == "self") {
                    return Ok(PointerOrigin::SelfPlace {
                        mutability: Mutability::Param(mojito_types::origin::OriginParamId(0)),
                        interior: Vec::new(),
                        subtree: false,
                    });
                }
                let reference = self.reference_actual(&args[0])?;
                match reference.origin {
                    Origin::Place(place) => Ok(PointerOrigin::Place {
                        place,
                        mutable: matches!(reference.mutability, Mutability::Mutable),
                    }),
                    Origin::Param(id) => Ok(PointerOrigin::Param {
                        id,
                        mutability: reference.mutability,
                        interior: Vec::new(),
                        subtree: false,
                    }),
                    other => Err(TypeError::Unsupported(format!(
                        "origin_of over {other:?} is not a supported Pointer origin argument"
                    ))),
                }
            }
            _ => Err(TypeError::TypeMismatch {
                expected: "Self.origin, origin_of(place), or a builtin Origin value".to_string(),
                found: "a runtime value".to_string(),
                context: "Pointer origin".to_string(),
            }),
        }
    }

    /// Look up an enclosing `Origin`-bounded type parameter by name and
    /// resolve its declared mutability.
    pub(super) fn enclosing_origin_param(
        &self,
        name: &str,
    ) -> Result<mojito_types::origin::PointerOrigin, TypeError> {
        use mojito_types::origin::{Mutability, OriginParamId, PointerOrigin};

        let (index, parameter) = self
            .enclosing_type_params
            .iter()
            .enumerate()
            .find(|(_, parameter)| {
                parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
            })
            .ok_or_else(|| TypeError::UnknownSelfParam(name.to_string()))?;
        let id = OriginParamId(index as u32);
        let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
            Some(ExprKind::Bool(true)) => Mutability::Mutable,
            Some(ExprKind::Bool(false)) => Mutability::Immutable,
            _ => Mutability::Param(id),
        };
        Ok(PointerOrigin::Param {
            id,
            mutability,
            interior: Vec::new(),
            subtree: false,
        })
    }

    /// Resolve `Tuple[T1, …, Tn]` from its type arguments (each a type).
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
                mojito_ast::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
                // A bare-identifier arg is reinterpreted as a type (as elsewhere).
                mojito_ast::ast::ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(id),
                    ..
                }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
                mojito_ast::ast::ParamArg::Value(_) => {
                    return Err(TypeError::TypeMismatch {
                        expected: "a type".to_string(),
                        found: "a value".to_string(),
                        context: "Tuple element type".to_string(),
                    });
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
    fn unbound_pack_type(&self, base: &SourceType) -> Option<Ty> {
        match base {
            SourceType::SelfParam(name) => {
                let starred = format!("*{}", name.trim_start_matches('*'));
                self.self_decls
                    .iter()
                    .any(|decl| {
                        matches!(decl, ParamDecl::Type { name, variadic: true, .. } if *name == starred)
                    })
                    .then(|| self.ty_from_anno(&SourceType::SelfParam(starred)).ok())
                    .flatten()
                    .filter(|ty| matches!(ty.kind(), TyKind::Param { .. }))
            }
            SourceType::Named(name, arguments) if arguments.is_empty() => {
                self.lookup_tparam(&format!("*{}", name.trim_start_matches('*')))
            }
            _ => None,
        }
    }

    /// The type of the compile-time value a type expression subscripts or a
    /// member read reads through: a struct construction
    /// (`TypeList[Ts.values]()`), or one through a type's parameterized
    /// member (`M.List[Bool, Int]()`) or a generic alias (`L[Bool, Int]()`),
    /// whose struct has an `__init__` taking no argument.
    pub(super) fn compile_time_value_type(&self, value: &Expr) -> Result<Ty, TypeError> {
        let not_construction = || {
            TypeError::Unsupported("a subscript in a type takes a struct construction".to_string())
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
                let TyKind::Struct(name, _) = ty.kind() else {
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
                let base = object.type_spelling().ok_or_else(not_construction)?;
                let member = SourceType::Assoc {
                    base: Box::new(base),
                    name: field.clone(),
                    args: param_args.clone(),
                };
                let ty = self.ty_from_anno(&member)?;
                let TyKind::Struct(name, _) = ty.kind() else {
                    return Err(not_construction());
                };
                let name = name.clone();
                (ty, name, args, kwargs)
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
    }

    /// `value`, an argument of a value parameter of type `ty`, in the
    /// parameter's domain: an application computing a `String`
    /// (`label[pick(n)]()`) binds a `String` value parameter as the
    /// compile-time string the elaborator freezes its result to, which the
    /// call carries as the argument's attribute ([`bracket_attribute`]).
    fn string_parameter_value(&self, value: CtValue, ty: &Ty) -> CtValue {
        let CtValue::Expr(application) = &value else {
            return value;
        };
        let mojito_types::param_expr::ParamKind::Apply { function, args } = application.kind()
        else {
            return value;
        };
        let computes_string = application.meta().as_value().is_some_and(|computed| {
            matches!(computed.kind(), TyKind::Struct(name, computed_args)
                if computed_args.is_empty()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name))
        });
        if *ty != TyKind::StringLiteral || !computes_string {
            return value;
        }
        CtValue::Expr(self.param_context.apply(
            function,
            args,
            mojito_types::param_expr::MetaTy::value(ty.clone()),
        ))
    }

    /// Whether a nearer type parameter shadows the pack `*Ts`: a nested
    /// `def[Ts: AnyType]` binds the bare name to something that is not a
    /// pack, so a `*Ts` inside it does not reach the enclosing pack. Both
    /// compilers reject that spread at the declaration.
    /// Whether the closed value argument `expr` types at its declared
    /// parameter type `ty` as a run-time argument does, recording the
    /// implicit conversions its elements need (a literal element at a
    /// `String` is `String.__init__(StringLiteral)`).
    fn converts_to_parameter(&self, expr: &Expr, value: &CtValue, ty: &Ty) -> bool {
        !matches!(value, CtValue::Expr(_) | CtValue::Deferred(_))
            && self
                .infer_with_expected(expr, ty, true)
                .is_ok_and(|converted| coerces(&converted, ty))
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
            callee: decl.id().owner.to_string(),
            param: decl.name().to_string(),
            expected: TyKind::AnyTrait(bound.to_vec()).to_string(),
            found,
        };
        let type_argument = |source: &SourceType| {
            self.ty_from_anno(source)
                .map(|ty| mismatch(format!("AnyStruct[{ty}]")))
        };
        let value = match arg {
            ParamArg::Named { value, .. } => {
                return self.resolve_trait_param_arg(decl, bound, value);
            }
            ParamArg::Value(expr) => match self.trait_value(expr) {
                Some(traits) => CtValue::Trait(traits?),
                None => match self.compile_dependent_ct_expr(expr) {
                    Ok(value) => value.into_value(),
                    Err(error) => {
                        let source = match &expr.kind {
                            ExprKind::Identifier(name) => SourceType::Named(name.clone(), vec![]),
                            ExprKind::TypeApply { name, args } => {
                                SourceType::Named(name.clone(), args.clone())
                            }
                            _ => return Err(error),
                        };
                        return Err(type_argument(&source).unwrap_or(error));
                    }
                },
            },
            ParamArg::Type(SourceType::SelfParam(param))
                if let Some(value) = self.self_param_value(param) =>
            {
                value
            }
            ParamArg::Type(source) => return Err(type_argument(source)?),
        };
        match self.ct_value_ty(&value, &Ty::error()) {
            Some(ref ty) if let TyKind::AnyTrait(traits) = ty.kind() => {
                if self.traits_satisfy(traits, bound) {
                    if let ParamArg::Value(expr) = arg {
                        self.record_at(expr.source_span()).argument_attribute =
                            Some(TyArg::Val(value.clone()));
                    }
                    Ok(TyArg::Val(value))
                } else {
                    Err(mismatch(ty.to_string()))
                }
            }
            found => Err(mismatch(match (&value, found) {
                (CtValue::IntLiteral(literal), _) => format!("IntLiteral[{literal}]"),
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
                    self.tparams
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
        use mojito_ast::ast::ParamArg;
        // The elaborator's pack-qualification pass owns the rule that a member
        // spells its struct's pack `Self.Ts`; a spread it has normalized to
        // the bare spelling still names that pack.
        let pack = |name: &String| {
            name.starts_with('*').then(|| {
                if self.pack_is_shadowed(name) {
                    return Err(TypeError::UnknownType(name.clone()));
                }
                self.lookup_tparam(name).map_or_else(
                    || self.ty_from_anno(&SourceType::SelfParam(name.clone())),
                    Ok,
                )
            })
        };
        match arg {
            ParamArg::Type(anno @ SourceType::SelfParam(name)) if name.starts_with('*') => {
                Some(self.ty_from_anno(anno))
            }
            ParamArg::Type(SourceType::Named(name, arguments)) if arguments.is_empty() => {
                pack(name)
            }
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => pack(name),
            _ => None,
        }
    }

    pub(super) fn tuple_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        let (elements, attributes) = self.tuple_element_types(args)?;
        for element in &elements {
            reject_stored_callable_type(element, "the 'Tuple' element type")?;
        }
        Ok((mojito_types::types::tuple_type(elements), attributes))
    }

    /// Upstream's spelling of a checked type in a diagnostic, with each struct
    /// origin argument rendered as `origin_of(<place>)` through the names in
    /// scope (`P[origin_of(xs)]`), `Self.o`-style binders as `origin#N`, and an
    /// unbound slot as `_`.
    pub(super) fn display_ty_with_origin_names(&self, ty: &Ty) -> String {
        use mojito_types::origin::{Origin, OriginSeg};
        let origin_text = |origin: &Origin| -> String {
            match origin {
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
                        TyKind::Struct(name, _) => self.structs.get(name),
                        _ => None,
                    })
                    .and_then(|info| info.source_params.get(id.0 as usize))
                    .map_or_else(|| origin.to_string(), |param| param.name.clone()),
                other => other.to_string(),
            }
        };
        match ty.kind() {
            TyKind::Struct(name, args) if !args.is_empty() => {
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
            (TyKind::Simd { dtype, width }, _) if width.is_inferred() => {
                mojito_types::types::simd_slots(found)
                    .filter(|(_, found_width)| !found_width.is_expr())
                    .and_then(|(_, found_width)| {
                        mojito_types::types::simd_ty_from_slots(dtype.clone(), found_width).ok()
                    })
                    .unwrap_or_else(|| declared.clone())
            }
            (TyKind::Struct(name, declared_args), TyKind::Struct(found_name, found_args))
                if name == found_name && declared_args.len() == found_args.len() =>
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
                            ) => found.clone(),
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
    /// P(Pointer(to=ys))` over `P[origin_of(xs)]`); `None` when the two
    /// differ in anything but their origin tails.
    pub(super) fn struct_origin_mismatch(&self, found: &Ty, expected: &Ty) -> Option<TypeError> {
        let (TyKind::Struct(found_name, found_args), TyKind::Struct(expected_name, expected_args)) =
            (found.kind(), expected.kind())
        else {
            return None;
        };
        if found_name != expected_name || found_args.len() != expected_args.len() {
            return None;
        }
        let only_tails_differ =
            found_args
                .iter()
                .zip(expected_args)
                .all(|(found, expected)| match (found, expected) {
                    (TyArg::Origin(_), TyArg::Origin(_)) => true,
                    _ => found == expected,
                });
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
    /// `TyKind::Struct(name, info.self_arguments())` — the same value the checker
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
        Some(TyKind::Struct(name.to_string(), info.self_arguments()).into())
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
        (!info.decls.is_empty())
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
            decls: &info.decls,
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
    ) -> Result<(Ty, BracketAttributes), TypeError> {
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
        Ok((TyKind::Variant(alternatives).into(), attributes))
    }

    /// Resolve the case payload types of `__EnumStorage[T1, ..., Tn]`. Unlike
    /// a `Variant`'s alternatives they may repeat, since every operation
    /// selects a case by index, and an enum without cases has none.
    pub(super) fn enum_storage_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        let alternatives = args
            .iter()
            .map(|arg| self.type_param_argument(arg, "enum case payload"))
            .collect::<Result<Vec<_>, _>>()?;
        let attributes = types_attributes(args, &alternatives);
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
            mojito_ast::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
            mojito_ast::ast::ParamArg::Value(expression)
                if let Some(reflected) = self.reflected_type_operand(expression).transpose() =>
            {
                reflected
            }
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new())),
            mojito_ast::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: context.to_string(),
            }),
            mojito_ast::ast::ParamArg::Named { .. } => Err(TypeError::Unsupported(format!(
                "named arguments are not supported in {context}"
            ))),
        }
    }

    /// The type operand of a well-formed target-layout query.
    pub(super) fn size_of_operand(
        &self,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let name = mojito_types::param_expr::SIZE_OF_FUNCTION;
        if param_args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 1,
                got: param_args.len(),
            });
        }
        if !args.is_empty() {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: 0,
                got: args.len(),
            });
        }
        self.type_param_argument(&param_args[0], name)
    }

    /// Resolve `SIMD[DType.<dt>, width]` from its two parameter arguments to its
    /// `(dtype, width)` slots (raw — not canonicalized).
    pub(super) fn simd_dims(
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

    /// The (canonicalized) `Ty` for `SIMD[DType.<dt>, width]` — a width-1 `float64`
    /// resolves to `TyKind::Float64` (the unification).
    pub(super) fn simd_type(
        &self,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, BracketAttributes), TypeError> {
        let (dtype, width, attributes) = self.simd_dims(args)?;
        Ok((simd_of(dtype, width)?, attributes))
    }

    /// Evaluate a SIMD width argument: a comptime `Int` that is a power of
    /// two, or — while its declaration is a template — an `Int` expression
    /// over value parameters in scope (`width`, `2 * n`, `Self.width`), whose
    /// power-of-two check is the instantiation's.
    pub(super) fn simd_width(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<(SimdWidth, Option<TyArg>), TypeError> {
        let value = match arg {
            mojito_ast::ast::ParamArg::Value(expr) => {
                let _lifting = self.lifting_position();
                match self.eval_associated_ct(expr) {
                    Ok(value) => value,
                    Err(error) => CtValue::Expr(self.lifted_application(expr).ok_or(error)?),
                }
            }
            mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(param))
                if let Some(value) = self.self_param_value(param) =>
            {
                value
            }
            mojito_ast::ast::ParamArg::Type(source)
                if let Some(read) = self.type_spelled_member_read(source) =>
            {
                self.eval_associated_ct(&read)?
            }
            // `Self.e.rows`, a field of a struct-typed value parameter,
            // parses as an associated member of the parameter.
            mojito_ast::ast::ParamArg::Type(source @ SourceType::Assoc { base, .. })
                if matches!(&**base, SourceType::SelfParam(_))
                    && let Some(read) =
                        source.expression_spelling(mojito_common::token::DUMMY_SPAN) =>
            {
                self.eval_associated_ct(&read)?
            }
            mojito_ast::ast::ParamArg::Type(ty)
                if let Some(element) = self.display_element_argument(ty) =>
            {
                CtValue::Expr(element?)
            }
            // A lowercase name parses as a type argument: the enclosing
            // struct's own value parameter spelled bare, or `Self.<field>`.
            mojito_ast::ast::ParamArg::Type(ty) => {
                if let Some(error) = self.bare_or_field_in_value_slot(ty) {
                    return Err(error);
                }
                return Err(TypeError::BadSimdWidth("a type".to_string()));
            }
            mojito_ast::ast::ParamArg::Named { .. } => {
                return Err(TypeError::BadSimdWidth("a named argument".to_string()));
            }
        };
        let attribute = bracket_attribute(arg, &TyArg::Val(value.clone()));
        if let CtValue::Expr(expr) = value {
            if !expr.meta().is_integer() {
                return Err(TypeError::BadSimdWidth(expr.to_string()));
            }
            return Ok((SimdWidth::Expr(expr), attribute));
        }
        let w = mojito_types::param_expr::fold::integer_value(&value)
            .and_then(|width| width.to_i64())
            .ok_or_else(|| TypeError::BadSimdWidth(value.to_string()))?;
        if w >= 1 && (w & (w - 1)) == 0 {
            Ok((SimdWidth::Known(w), attribute))
        } else {
            Err(TypeError::BadSimdWidth(w.to_string()))
        }
    }

    /// The read a type-spelled bracket argument denotes in a value slot,
    /// where upstream's argument is an expression: `Self.n` or `Self.T.K`
    /// naming a compile-time value member (a struct's, or a trait's through
    /// its own `Self` or a bound). `None` for any other spelling, a type
    /// parameter or type member among them, which stays a type.
    pub(super) fn type_spelled_member_read(&self, source: &SourceType) -> Option<Expr> {
        if !matches!(source, SourceType::SelfParam(_) | SourceType::Assoc { .. }) {
            return None;
        }
        let read = source.expression_spelling(mojito_common::token::DUMMY_SPAN)?;
        let ExprKind::Member { object, field } = &read.kind else {
            return None;
        };
        self.type_member_value(object, field)
            .is_some()
            .then_some(read)
    }

    /// Upstream's rejections of two type-shaped spellings in a value slot: the
    /// enclosing struct's own parameter spelled bare (`Counter[length]`, which
    /// parses as a type argument) and a field spelled `Self.<field>`.
    pub(super) fn bare_or_field_in_value_slot(&self, ty: &SourceType) -> Option<TypeError> {
        match ty {
            SourceType::Named(name, args)
                if args.is_empty() && self.is_enclosing_struct_param(name) =>
            {
                Some(TypeError::UnqualifiedStructParam(name.clone()))
            }
            SourceType::SelfParam(field) => self.instance_field_without_instance(field),
            _ => None,
        }
    }

    /// If `name` is a generic type parameter currently in scope, return its
    /// complete checked type-parameter fact.
    pub(super) fn lookup_tparam(&self, name: &str) -> Option<Ty> {
        self.local_type_aliases
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.tparams.iter().rev().find_map(|scope| scope.get(name)))
            .cloned()
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
}

/// Whether `value` is an application computing a compile-time string
/// ([`Checker::string_parameter_value`]).
/// The owner of every generator type's own binders
/// ([`Checker::generator_type`]).
const GENERATOR_TYPE_OWNER: &str = "$generator";

fn computed_string_literal(value: &CtValue) -> bool {
    matches!(value, CtValue::Expr(expression)
        if matches!(expression.kind(), mojito_types::param_expr::ParamKind::Apply { .. })
            && expression.meta().as_value().is_some_and(|ty| *ty == TyKind::StringLiteral))
}

fn type_is_symbolic(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Param { .. }
        | TyKind::Assoc { .. }
        | TyKind::Dependent(_)
        | TyKind::SelfType
        | TyKind::Infer => true,
        TyKind::Struct(_, arguments) => arguments.iter().any(tyarg_is_symbolic),
        TyKind::Simd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        TyKind::Int
        | TyKind::UInt
        | TyKind::Bool
        | TyKind::StringLiteral
        | TyKind::Float64
        | TyKind::None
        | TyKind::Never
        | TyKind::IntLiteral
        | TyKind::FloatLiteral
        | TyKind::Dtype
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::Error => false,
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
        | TyKind::VariadicPack(element)
        | TyKind::PopArray { element, .. } => type_is_symbolic(element),
        TyKind::Pointer { element, .. } => type_is_symbolic(element),
        TyKind::Ref(reference) => type_is_symbolic(&reference.referent),
    }
}

/// Append an interior-generation tag to a tracked pointer origin — the
/// resolution of a `._get_owned_interior["tag"]` projection in a `Pointer`
/// origin argument. Untracked provenances have no place to project into.
/// Whether a parameterized member's own parameter is an origin
/// (`o: Origin[mut=m]`).
pub(super) fn is_origin_parameter(param: &mojito_ast::ast::TypeParam) -> bool {
    matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
}

/// Whether a parameterized member's own parameter is a value (`n: Int`).
pub(super) fn is_value_parameter(param: &mojito_ast::ast::TypeParam) -> bool {
    param.value_type.is_some()
        || matches!(param.bounds.as_slice(), [only] if scalar_type_name(only).is_some())
}

pub(in crate::checker) fn append_interior_tag(
    mut origin: mojito_types::origin::PointerOrigin,
    tag: &str,
) -> Result<mojito_types::origin::PointerOrigin, TypeError> {
    use mojito_types::origin::{OriginSeg, PointerOrigin};
    if origin_has_subtree_tail(&origin) {
        return Err(subtree_is_terminal_error());
    }
    match &mut origin {
        PointerOrigin::Param { interior, .. } | PointerOrigin::SelfPlace { interior, .. } => {
            interior.push(tag.to_string());
        }
        PointerOrigin::Place { place, .. } => {
            place.path.push(OriginSeg::Interior(tag.to_string()));
        }
        _ => {
            return Err(TypeError::TypeMismatch {
                expected: "an origin parameter or origin_of(place) base".to_string(),
                found: "an untracked origin".to_string(),
                context: "interior origin projection".to_string(),
            });
        }
    }
    Ok(origin)
}

/// Append the conservative `._subtree` projection to a tracked pointer origin.
/// Subtree is terminal: nothing projects below it, including another
/// `._subtree`. Untracked provenances have no place to project into.
pub(in crate::checker) fn append_subtree(
    mut origin: mojito_types::origin::PointerOrigin,
) -> Result<mojito_types::origin::PointerOrigin, TypeError> {
    use mojito_types::origin::{OriginSeg, PointerOrigin};
    if origin_has_subtree_tail(&origin) {
        return Err(subtree_is_terminal_error());
    }
    match &mut origin {
        PointerOrigin::Param { subtree, .. } | PointerOrigin::SelfPlace { subtree, .. } => {
            *subtree = true;
        }
        PointerOrigin::Place { place, .. } => {
            place.path.push(OriginSeg::Subtree);
        }
        _ => {
            return Err(TypeError::TypeMismatch {
                expected: "an origin parameter or origin_of(place) base".to_string(),
                found: "an untracked origin".to_string(),
                context: "'_subtree' origin projection".to_string(),
            });
        }
    }
    Ok(origin)
}

fn origin_has_subtree_tail(origin: &mojito_types::origin::PointerOrigin) -> bool {
    use mojito_types::origin::{OriginSeg, PointerOrigin};
    match origin {
        PointerOrigin::Param { subtree, .. } | PointerOrigin::SelfPlace { subtree, .. } => *subtree,
        PointerOrigin::Place { place, .. } => {
            matches!(place.path.last(), Some(OriginSeg::Subtree))
        }
        _ => false,
    }
}

fn unsupported_mlir_type() -> TypeError {
    TypeError::Unsupported(
        "this MLIR type spelling is outside the supported subset; only \
         '__mlir_type.index', '__mlir_type.`!kgen.type`', \
         '__mlir_type.`!kgen.string`', and \
         '__mlir_type[`!kgen.param_list<`, T, `>`]' are accepted"
            .to_string(),
    )
}

fn subtree_is_terminal_error() -> TypeError {
    TypeError::Unsupported(
        "'_subtree' is a terminal origin projection: nothing can be projected below it".to_string(),
    )
}

/// The same provenance with its capability pinned read-only: the
/// resolution of the capability cast `ImmOrigin(o)` in a `Pointer` origin
/// argument or an `unsafe_origin_cast` target.
fn immutable_capability(
    origin: mojito_types::origin::PointerOrigin,
) -> mojito_types::origin::PointerOrigin {
    use mojito_types::origin::{Mutability, PointerOrigin};
    match origin {
        PointerOrigin::Place { place, .. } => PointerOrigin::Place {
            place,
            mutable: false,
        },
        PointerOrigin::Param {
            id,
            interior,
            subtree,
            ..
        } => PointerOrigin::Param {
            id,
            mutability: Mutability::Immutable,
            interior,
            subtree,
        },
        PointerOrigin::SelfPlace {
            interior, subtree, ..
        } => PointerOrigin::SelfPlace {
            mutability: Mutability::Immutable,
            interior,
            subtree,
        },
        PointerOrigin::Untracked { .. } => PointerOrigin::Untracked { mutable: false },
        PointerOrigin::UnsafeAny { .. } => PointerOrigin::UnsafeAny { mutable: false },
        PointerOrigin::Static => PointerOrigin::Static,
    }
}

/// Whether an application argument is upstream's origin placeholder spelling
/// (`_` or `...`), possibly behind a keyword (`origin=_`): the slot is
/// explicitly marked inferred rather than omitted.
/// The struct origin a pointer provenance binds as a bracket argument.
fn pointer_origin_attribute(
    origin: &mojito_types::origin::PointerOrigin,
) -> Option<mojito_types::origin::Origin> {
    use mojito_types::origin::{Origin, PointerOrigin};
    match origin {
        PointerOrigin::Static => Some(Origin::Static),
        PointerOrigin::Untracked { mutable } | PointerOrigin::UnsafeAny { mutable } => {
            Some(Origin::Untracked { mutable: *mutable })
        }
        other => other.as_origin(),
    }
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

/// Whether an application argument is syntactically origin-shaped — an
/// `origin_of(...)` call, a binder name (`o`, `Self.o`), or a projected origin
/// expression — regardless of whether its places resolve in this context.
fn syntactic_origin_argument(argument: &mojito_ast::ast::ParamArg) -> bool {
    let mut value = argument;
    while let mojito_ast::ast::ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    match value {
        mojito_ast::ast::ParamArg::Type(mojito_ast::ast::Type::Named(_, targs)) => targs.is_empty(),
        mojito_ast::ast::ParamArg::Type(mojito_ast::ast::Type::SelfParam(_)) => true,
        mojito_ast::ast::ParamArg::Value(expression) => {
            matches!(
                &expression.kind,
                ExprKind::Call { name, .. } if name == "origin_of"
            ) || matches!(
                &expression.kind,
                ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. }
            )
        }
        _ => false,
    }
}

/// Whether a return annotation is spelled `__mlir_type.index`, the exact
/// return type upstream's `Indexer.__mlir_index__` requirement demands.
pub(super) fn spells_mlir_index(annotation: Option<&SourceType>) -> bool {
    matches!(
        annotation,
        Some(SourceType::Assoc { base, name, args })
            if name == "index"
                && args.is_empty()
                && matches!(base.as_ref(), SourceType::Named(base, base_args)
                    if base == "__mlir_type" && base_args.is_empty())
    )
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
