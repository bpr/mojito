//! Compile-time evaluation and generic-constraint compilation/evaluation
//! for the checker: `comptime`-int folding, associated-const evaluation,
//! and `where`-clause constraint lowering and checking.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ParamArg, multi_subscript_param_args, subscript_param_args};
use mojito_symbol::symbol::OverloadLookup;
use mojito_types::conformance::Conformance;
use mojito_types::ct::SimdConstructionError;
use mojito_types::param_expr::fold::comparison_holds;
use mojito_types::types::TyKind;

/// The reason a constraint neither proven nor disproven fails with, as
/// upstream reports an undischarged proposition.
pub(super) const LACKING_EVIDENCE: &str = "lacking evidence to prove correctness; ";

/// Whether a module constant folded to `value` is that attribute wherever
/// a body reads it: a closed parameter argument, a string, an untyped
/// MLIR attribute, or a target attribute.
pub(super) fn is_module_constant_attribute(value: &CtValue) -> bool {
    value.is_folded_parameter_argument()
        || matches!(
            value,
            CtValue::Str(_) | CtValue::MlirAttr(_) | CtValue::KgenTarget(_)
        )
}

impl Checker {
    /// Evaluate a compile-time `Int` expression: literals, `comptime` constants,
    /// and `+ - * // % **` / unary `-`. Rejects anything non-comptime (a value
    /// parameter, a call, a non-`Int` operation).
    pub(super) fn eval_ct(
        &self,
        expr: &Expr,
    ) -> Result<mojito_common::literal::IntLiteral, TypeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(n.clone()),
            ExprKind::Identifier(name) => self.module_comptime(name).ok_or_else(|| {
                // A bare struct value parameter (`SIMD[d, length]`) is
                // upstream's `use 'Self.length'` error, not a missing constant.
                if self.is_enclosing_struct_param(name) {
                    TypeError::UnqualifiedStructParam(name.clone())
                } else {
                    TypeError::NotComptime(name.clone())
                }
            }),
            ExprKind::Prefix(PrefixOp::Neg, e) => Ok(self.eval_ct(e)?.neg()),
            ExprKind::Prefix(PrefixOp::Pos, e) => self.eval_ct(e),
            ExprKind::Infix(op, l, r) => {
                let (a, b) = (self.eval_ct(l)?, self.eval_ct(r)?);
                let folded = mojito_types::param_expr::fold::fold_infix(
                    *op,
                    &CtValue::IntLiteral(a),
                    &CtValue::IntLiteral(b),
                )
                .map_err(param_error)?;
                match folded {
                    CtValue::IntLiteral(value) => Ok(value),
                    _ => Err(TypeError::NotComptime(
                        "unsupported comptime operation".to_string(),
                    )),
                }
            }
            _ => Err(TypeError::NotComptime(
                "not a comptime Int expression".to_string(),
            )),
        }
    }

    /// Fold every closed top-level constant of `stmts` once, in source
    /// order, with the one parameter-expression builder: an integer into the
    /// module's constants and the program's table every symbol speller
    /// reads, any other attribute into the module's values, and a literal
    /// as its initializer. One that reads a declaration not yet registered
    /// builds in the declaration pass instead.
    pub(super) fn fold_module_constants(&mut self, stmts: &[Stmt]) {
        for statement in stmts {
            let StmtKind::Comptime {
                name,
                type_params,
                ty,
                value,
                ..
            } = &statement.kind
            else {
                continue;
            };
            if !type_params.is_empty() {
                continue;
            }
            // A vector annotation converts a scalar initializer by
            // splatting it across the lanes, as `SIMD`'s implicit scalar
            // constructor does.
            if let Some(annotation) = ty
                && let Some(vector) = self
                    .closed_module_constant(value)
                    .and_then(|scalar| self.splat_at_annotation(&scalar, annotation))
            {
                self.module_values.insert(name.clone(), vector);
                continue;
            }
            // A module type alias spells, in every declaration symbol, as
            // the type it names.
            if let Some(spelling) = module_alias_spelling(value) {
                std::sync::Arc::make_mut(&mut self.program_constants)
                    .declare_type_alias(name.clone(), spelling);
            }
            match self.closed_module_constant(value) {
                Some(CtValue::IntLiteral(constant)) => {
                    self.comptimes.insert(name.clone(), constant.clone());
                    std::sync::Arc::make_mut(&mut self.program_constants)
                        .declare(name.clone(), constant);
                }
                _ if super::traits::literal_default(value) => {
                    self.comptime_literals.insert(name.clone(), value.clone());
                }
                Some(folded) => {
                    self.module_values.insert(name.clone(), folded);
                }
                None => {}
            }
        }
    }

    /// `scalar` splat across the lanes of `annotation`, a vector type of
    /// more than one lane. `None` for any other annotation or value.
    fn splat_at_annotation(&self, scalar: &CtValue, annotation: &SourceType) -> Option<CtValue> {
        let (dtype, width) = self.vector_annotation(annotation)?;
        mojito_types::ct::simd_construction_lanes(dtype, width, std::slice::from_ref(scalar))
            .ok()
            .map(|lanes| CtValue::Simd { dtype, lanes })
    }

    /// The dtype and width of `annotation` when it names a vector of more
    /// than one lane.
    pub(super) fn vector_annotation(
        &self,
        annotation: &SourceType,
    ) -> Option<(mojito_ast::ast::Dtype, usize)> {
        let ty = self.ty_from_anno(annotation).ok()?;
        let (dtype, width) = mojito_types::types::simd_shape(ty.kind())?;
        usize::try_from(width)
            .ok()
            .filter(|width| *width > 1)
            .map(|width| (dtype, width))
    }

    /// The attribute a closed module constant's initializer folds to
    /// (`3 + 1 if 3 > 2 else 0`, `Int32(4)`, `(1, 2)`, `"buf" + "fer"`), by
    /// the one builder, a call of a builtin callable included
    /// (`SIMD[DType.int32, 2](from_int=5)`), as the pin folds one. `None`
    /// for an initializer that applies any other callable, reads a
    /// declaration not yet registered, or names a type.
    pub(super) fn closed_module_constant(&self, value: &Expr) -> Option<CtValue> {
        self.ct_value(value, Recording::Silent)
            .ok()
            .or_else(|| {
                let _lifting = self.lifting_position();
                self.compile_dependent_ct_expr(value)
                    .ok()?
                    .as_constant()
                    .cloned()
            })
            .filter(is_module_constant_attribute)
    }

    /// Classify a comptime-member annotation. In Mojo terms, `comptime
    /// count: Int` requires an integer compile-time value, while `comptime
    /// Element: AnyType` requires a type-valued member whose type conforms
    /// to `AnyType`. Either may take parameters (`comptime get[idx: Int]:
    /// Int`), as upstream's alias declaration does.
    pub(super) fn ct_member_req_from_anno(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        ty: &SourceType,
    ) -> Result<CtMemberReq, TypeError> {
        if let SourceType::Named(name, args) = ty
            && name == "$trait_composition"
        {
            let mut bounds = Vec::with_capacity(args.len());
            for argument in args {
                let mojito_ast::ast::ParamArg::Type(SourceType::Named(bound, bound_args)) =
                    argument
                else {
                    return Err(TypeError::Unsupported(
                        "associated type bounds must be trait names".to_string(),
                    ));
                };
                if !bound_args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "associated type bounds cannot take arguments".to_string(),
                    ));
                }
                self.check_trait_name(bound)?;
                if !bounds.iter().any(|existing| existing == bound) {
                    bounds.push(bound.clone());
                }
            }
            return Ok(CtMemberReq::Type {
                bounds,
                params: params.to_vec(),
            });
        }
        if let SourceType::Named(name, args) = ty
            && args.is_empty()
            && (COMPILER_TRAITS.contains(&name.as_str()) || self.traits.contains_key(name))
        {
            self.check_trait_name(name)?;
            return Ok(CtMemberReq::Type {
                bounds: vec![name.clone()],
                params: params.to_vec(),
            });
        }
        Ok(CtMemberReq::Value(self.ty_from_anno(ty)?))
    }

    /// `conformance_conditions` are the struct's conditional conformances: a
    /// member that a conditional trait requires (`Iterable where
    /// conforms_to(T, Copyable)` → `IteratorType`) resolves under that
    /// condition's atoms as well as under its own `where` clause.
    pub(super) fn check_struct_associated(
        &mut self,
        associated: &[StructComptime],
        conforms: &[String],
        conformance_conditions: &[(String, Expr)],
    ) -> Result<StructAssociatedMembers, TypeError> {
        // A member the struct omits is the default a conformed trait
        // declares for it, read after the struct's own, with `Self` as
        // the struct.
        let mut defaults = Vec::new();
        for trait_name in conforms {
            self.collect_comptime_defaults(trait_name, &mut defaults);
        }
        defaults.retain(|default| associated.iter().all(|member| member.name != default.name));
        let mut out = HashMap::new();
        let mut value_types = HashMap::new();
        let mut constraints = HashMap::new();
        let mut parameterized = HashMap::new();
        let struct_name = self
            .self_ty
            .as_ref()
            .and_then(mojito_types::types::nominal_parts)
            .map_or_else(|| "Self".to_string(), |(name, _)| name.to_string());
        let members: Vec<&StructComptime> = associated.iter().chain(&defaults).collect();
        for member in read_order(&members) {
            if out.contains_key(&member.name) || parameterized.contains_key(&member.name) {
                return Err(TypeError::Redeclaration(member.name.clone()));
            }
            let generator = self
                .partially_bound_generator(&member.params, &member.value)?
                .map(|(params, value)| StructComptime {
                    params,
                    value,
                    ..member.clone()
                });
            let member = generator.as_ref().unwrap_or(member);
            let member_owner = format!("{struct_name}.{}", member.name);
            // Resolve and classify the annotation now even when the member's
            // symbolic body cannot be checked until application. This keeps
            // associated `comptime NAME: Type where ... = ...` declarations
            // from silently discarding an invalid declared type.
            let requirement = member
                .ty
                .as_ref()
                .map(|annotation| self.ct_member_req_from_anno(&member.params, annotation))
                .transpose()?;
            // The member's own clause and the conditions of the conditional
            // traits requiring it are assumed while its body resolves; every
            // application re-checks the clause (`availability`), and a
            // conditional conformance is verified to imply the member's clause.
            let compiled = member
                .where_clauses
                .iter()
                .map(|condition| self.compile_where_clause(condition))
                .collect::<Result<Vec<_>, _>>()?;
            let mut facts = Vec::new();
            for constraint in &compiled {
                self.guaranteed_facts(&self.bind_constraint(constraint, &[]), &mut facts);
            }
            for (trait_name, condition) in conformance_conditions {
                if self.trait_requires_comptime_member(trait_name, &member.name) {
                    let constraint = self.compile_condition(&self.self_decls, condition)?;
                    self.guaranteed_facts(&constraint, &mut facts);
                }
            }
            self.push_assumptions(facts);
            let resolved = (|| {
                if member.params.is_empty() {
                    // A call is the application of its callee over the
                    // struct's parameters, which each instance evaluates.
                    let value = {
                        let _lifting = self.lifting_position();
                        self.eval_associated_ct(&member.value)?
                    };
                    if let Some(CtMemberReq::Value(declared)) = &requirement {
                        self.check_raw_list_member(&member.name, &value, declared)?;
                    }
                    return Ok::<_, TypeError>((Some(value), None));
                }
                // A value annotation declares a value body; without one, a
                // body that does not lower as a type is a value, as an
                // unannotated generic alias's is.
                let as_type = match requirement {
                    Some(CtMemberReq::Value(_)) => Err(unsupported_assoc_body()),
                    _ => assoc_body_source_type(&member.value).and_then(|source_ty| {
                        let decls = self.classify_params(&member_owner, &member.params)?;
                        let template = self.lower_parameterized_member(
                            &decls,
                            member_type_scope(&member_owner, &member.params),
                            &member.params,
                            &member.value,
                            &source_ty,
                        )?;
                        Ok(MemberTemplate::Type { decls, template })
                    }),
                };
                let template = match (as_type, &requirement) {
                    (Ok(template), _) => template,
                    (Err(error), Some(CtMemberReq::Type { .. })) => return Err(error),
                    (Err(type_error), _) => {
                        let declared = match &requirement {
                            Some(CtMemberReq::Value(ty)) => Some(ty),
                            _ => None,
                        };
                        // A body spelled as a type reports why it is not
                        // one, unless its annotation declares a value.
                        let spelled_as_type =
                            declared.is_none() && assoc_body_source_type(&member.value).is_ok();
                        if member.params.iter().any(|parameter| {
                            parameter.is_origin_mutability_binder(&member.params)
                                || matches!(assoc_param_kind(parameter), AssocParamKind::Origin)
                        }) {
                            return Err(if declared.is_some() {
                                TypeError::Unsupported(
                                    "origin parameters on a parameterized comptime value"
                                        .to_string(),
                                )
                            } else {
                                type_error
                            });
                        }
                        let decls = self.classify_params(&member_owner, &member.params)?;
                        match self.lower_parametric_value(&decls, &member.value, declared) {
                            Ok(body) => MemberTemplate::Value { decls, body },
                            Err(_) if spelled_as_type => return Err(type_error),
                            Err(error) => return Err(error),
                        }
                    }
                };
                Ok((None, Some(template)))
            })();
            self.pop_assumptions();
            match resolved? {
                (Some(value), _) => {
                    if !compiled.is_empty() {
                        constraints.insert(member.name.clone(), compiled);
                    }
                    // A closed raw list is a plain tuple, so an unannotated
                    // one keeps the list type its body had.
                    if let Some(CtMemberReq::Value(declared)) = &requirement {
                        value_types.insert(member.name.clone(), declared.clone());
                    } else if let Some(inferred) = self
                        .member_value_type(&member.value)
                        .or_else(|| self.ct_value_ty(&value, &Ty::infer()))
                        .filter(|ty| matches!(ty.kind(), TyKind::ParamList(_)))
                    {
                        value_types.insert(member.name.clone(), inferred);
                    }
                    // A later member reads an earlier one through `Self`
                    // (`comptime m = Box[Self.n].w`).
                    if let Some(info) = self.structs.get_mut(&struct_name) {
                        info.associated.insert(member.name.clone(), value.clone());
                        if let Some(ty) = value_types.get(&member.name) {
                            info.associated_value_types
                                .insert(member.name.clone(), ty.clone());
                        }
                        if matches!(value, CtValue::Type(_))
                            && let Ok(spelling) = assoc_body_source_type(&member.value)
                        {
                            info.associated_spellings
                                .insert(member.name.clone(), spelling);
                        }
                        if let Some(compiled) = constraints.get(&member.name) {
                            info.associated_constraints
                                .insert(member.name.clone(), compiled.clone());
                        }
                    }
                    out.insert(member.name.clone(), value);
                }
                (_, Some(template)) => {
                    let param_base = self.enclosing_type_params.len();
                    let definition = ParameterizedMember {
                        binder_owner: member_owner.clone(),
                        params: member.params.clone(),
                        template,
                        availability: compiled,
                        param_base,
                    };
                    // A later member reads an earlier one through `Self`
                    // (`comptime both = Self.plus[1] * 10`).
                    if let Some(info) = self.structs.get_mut(&struct_name) {
                        info.parameterized_associated
                            .insert(member.name.clone(), definition.clone());
                    }
                    parameterized.insert(member.name.clone(), definition);
                }
                (None, None) => unreachable!("an associated member is a value or a template"),
            }
        }
        Ok((out, value_types, constraints, parameterized))
    }

    /// The generator an unparameterized `comptime` body over a partially
    /// bound struct names (`comptime Bytes = SIMD[.uint8, _]`, `comptime Q =
    /// B[1, ...]`): its parameters are the struct's own unbound ones, by
    /// their declared names, so `Bytes[16]` and `Bytes[length=16]` bind
    /// them, and the body is the application with each unbound slot reading
    /// its parameter. A `_` slot keeps no default, where a slot a trailing
    /// `...` unbinds keeps its own. `None` when the body binds every slot,
    /// or leaves open an origin or a pack, which keeps its own meaning.
    pub(super) fn partially_bound_generator(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        value: &Expr,
    ) -> Result<Option<(Vec<mojito_ast::ast::TypeParam>, Expr)>, TypeError> {
        let Ok(SourceType::Named(struct_name, args)) = assoc_body_source_type(value) else {
            return Ok(None);
        };
        if !params.is_empty() || !args.iter().any(|arg| unbound_slot(arg).is_some()) {
            return Ok(None);
        }
        let Some(info) = self.structs.get(&struct_name) else {
            return Ok(None);
        };
        let declared = &info.source_params;
        let explicit: Vec<&mojito_ast::ast::TypeParam> =
            declared.iter().filter(|param| !param.infer_only).collect();
        let mut slots: Vec<Option<ParamArg>> = vec![None; explicit.len()];
        let mut open: Vec<(usize, bool, Expr)> = Vec::new();
        let mut rest = None;
        for (position, arg) in args.iter().enumerate() {
            let (index, inner) = match arg {
                ParamArg::Named { name, value } => {
                    let Some(index) = explicit.iter().position(|param| &param.name == name) else {
                        return Ok(None);
                    };
                    (index, value.as_ref())
                }
                _ if position < explicit.len() => (position, arg),
                _ => return Ok(None),
            };
            match unbound_slot(inner) {
                Some((UnboundSlot::Rest, spelling)) if position + 1 == args.len() => {
                    rest = Some(spelling.clone());
                }
                Some((UnboundSlot::Rest, _)) => return Ok(None),
                Some((UnboundSlot::Hole, spelling)) => open.push((index, false, spelling.clone())),
                None => slots[index] = Some(arg.clone()),
            }
        }
        if let Some(spelling) = rest {
            for (index, slot) in slots.iter().enumerate() {
                if slot.is_none() && open.iter().all(|(open_slot, ..)| *open_slot != index) {
                    open.push((index, true, spelling.clone()));
                }
            }
        }
        open.sort_by_key(|(index, ..)| *index);
        // An open parameter's declaration reads the bound ones as their
        // arguments (`v: Scalar[dt]` over `D[DType.int8, _]`).
        let bound = BoundSiblings(
            explicit
                .iter()
                .zip(&slots)
                .filter_map(|(param, slot)| {
                    let arg = match slot.as_ref()? {
                        ParamArg::Named { value, .. } => value.as_ref(),
                        arg => arg,
                    };
                    Some((param.name.clone(), arg.clone()))
                })
                .collect(),
        );
        let read = names_spelled(value);
        let mut generator_params = Vec::new();
        for (index, keeps_default, _) in &open {
            let param = explicit[*index];
            if param.name.starts_with('*') || param.is_erased_origin_param(declared) {
                return Ok(None);
            }
            if read.contains(&param.name) {
                return Err(TypeError::Unsupported(format!(
                    "the unbound parameter '{}' of '{struct_name}' shares its name with a \
                     name its bound arguments read",
                    param.name
                )));
            }
            if param.bounds.iter().any(|name| bound.0.contains_key(name)) {
                return Err(TypeError::Unsupported(format!(
                    "the unbound parameter '{}' of '{struct_name}' whose bound names a \
                     bound parameter",
                    param.name
                )));
            }
            let mut generator_param = param.clone();
            bound.substitute(&mut generator_param);
            if !keeps_default {
                generator_param.default = None;
            }
            generator_params.push(generator_param);
        }
        // Positional arguments stay a prefix: an unbound positional slot
        // reads its parameter in place, and every other unbound slot binds
        // by keyword.
        let positional = args
            .iter()
            .take_while(|arg| {
                !matches!(arg, ParamArg::Named { .. })
                    && !matches!(unbound_slot(arg), Some((UnboundSlot::Rest, _)))
            })
            .count();
        let mut rewritten = Vec::new();
        for (index, param) in explicit.iter().enumerate() {
            let bound = match &slots[index] {
                Some(ParamArg::Named { value, .. }) => Some(value.as_ref().clone()),
                Some(arg) => Some(arg.clone()),
                None => open
                    .iter()
                    .find(|(slot, ..)| *slot == index)
                    .map(|(.., spelling)| ParamArg::Value(parameter_read(spelling, &param.name))),
            };
            match bound {
                Some(bound) if index < positional => rewritten.push(bound),
                Some(bound) => rewritten.push(ParamArg::Named {
                    name: param.name.clone(),
                    value: Box::new(bound),
                }),
                None => {}
            }
        }
        let mut body = value.clone();
        body.kind = ExprKind::TypeApply {
            name: struct_name,
            args: rewritten,
        };
        Ok(Some((generator_params, body)))
    }

    /// A raw-list member's body converts to its declared list
    /// (`comptime _types: KGENParamListType[Copyable] = Self.Ts.values`
    /// over `*Ts: AnyType` does not), as at the pin's declaration.
    fn check_raw_list_member(
        &self,
        member: &str,
        value: &CtValue,
        declared: &Ty,
    ) -> Result<(), TypeError> {
        let Some(actual) = self
            .ct_value_ty(value, declared)
            .filter(|actual| matches!(actual.kind(), TyKind::ParamList(_)))
        else {
            return Ok(());
        };
        if !matches!(declared.kind(), TyKind::ParamList(_))
            || self.param_list_converts(&actual, declared)
        {
            return Ok(());
        }
        Err(TypeError::ParamListConversion {
            param: member.to_string(),
            from: actual.to_string(),
            to: declared.to_string(),
        })
    }

    /// The defaulted `comptime` requirements `trait_name` and its parents
    /// declare, parents first, a refinement's default replacing its
    /// parent's.
    pub(super) fn collect_comptime_defaults(
        &self,
        trait_name: &str,
        out: &mut Vec<StructComptime>,
    ) {
        let Some(info) = self.traits.get(trait_name) else {
            return;
        };
        for parent in &info.refines {
            self.collect_comptime_defaults(parent, out);
        }
        for default in &info.comptime_defaults {
            match out.iter_mut().find(|member| member.name == default.name) {
                Some(slot) => slot.clone_from(default),
                None => out.push(default.clone()),
            }
        }
    }

    /// Whether `trait_name` (or a trait it refines) declares the comptime
    /// member `member` — the member a conditional conformance to it requires.
    fn trait_requires_comptime_member(&self, trait_name: &str, member: &str) -> bool {
        let Some(info) = self.traits.get(trait_name) else {
            return false;
        };
        info.comptime_members.contains_key(member)
            || info
                .refines
                .iter()
                .any(|parent| self.trait_requires_comptime_member(parent, member))
    }

    /// Lower the type-valued body of a parameterized associated type — or a
    /// generic top-level comptime alias — to its symbolic template. The
    /// declaration's own parameters are put in scope (any enclosing struct's
    /// parameters already are), so type parameters resolve to `TyKind::Param`, value
    /// parameters to `CtValue::Expr`, and origin parameters to `Origin::Param`;
    /// concrete resolution substitutes an application's arguments into the result.
    pub(super) fn lower_parameterized_member(
        &mut self,
        decls: &[ParamDecl],
        scope: HashMap<String, Ty>,
        params: &[mojito_ast::ast::TypeParam],
        body: &Expr,
        source_ty: &SourceType,
    ) -> Result<Ty, TypeError> {
        // Member type parameters resolve as `TyKind::Param` (via the `tparams` scope
        // the caller built, like a generic def's parameters); origin parameters
        // resolve to `Origin::Param` via `enclosing_type_params`; value
        // parameters resolve to a symbolic `CtValue::Expr` through the value
        // scope `decls` open.
        self.push_param_scope_as(decls, scope);
        let saved = self.enclosing_type_params.len();
        self.enclosing_type_params.extend(params.iter().cloned());
        // The body's own occurrence records what it resolved to, under the
        // member's parameters.
        let template = match &body.kind {
            ExprKind::TypeApply { name, args } => {
                self.applied_type(name, args).map(|(template, attributes)| {
                    self.record_bracket_attributes(body.source_span(), attributes);
                    template
                })
            }
            _ => self.ty_from_anno(source_ty),
        };
        if let (Ok(template), ExprKind::TypeValue(_)) = (&template, &body.kind) {
            self.record_at(body.source_span()).type_operand = Some(template.clone());
        }
        self.enclosing_type_params.truncate(saved);
        self.tparams.pop();
        template
    }

    /// Lower the value body of a parameterized `comptime` member or generic
    /// alias to its symbolic template: upstream's generator over the
    /// declaration's own binders, which every application binds by
    /// substitution. A declared type converts the body as an alias
    /// initializer is converted.
    pub(super) fn lower_parametric_value(
        &mut self,
        decls: &[ParamDecl],
        body: &Expr,
        declared: Option<&Ty>,
    ) -> Result<ParamExpr, TypeError> {
        self.push_param_scope(decls);
        let compiled = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(body)
        };
        self.tparams.pop();
        let compiled = compiled?;
        let Some(ty) = declared else {
            return Ok(compiled);
        };
        let mismatch = |found: String| TypeError::TypeMismatch {
            expected: ty.to_string(),
            found,
            context: "a compile-time value declared with a type".to_string(),
        };
        if let Some(value) = compiled.as_constant() {
            return self
                .param_context
                .constant_as(value.clone(), ty)
                .map_err(|_| mismatch(value.to_string()));
        }
        match compiled.meta().as_value() {
            Some(actual) if coerces(actual, ty) => Ok(compiled),
            Some(actual) => Err(mismatch(actual.to_string())),
            None => Err(mismatch(compiled.meta().to_string())),
        }
    }

    /// Apply a parametric value template: `args` bound to `decls` through
    /// the contract a struct application uses (arity, bounds, defaults,
    /// `where` clauses), substituted into `body` beside the `outer`
    /// bindings, and folded. Returns the bound arguments beside the value.
    pub(super) fn apply_parametric_value(
        &self,
        label: &str,
        decls: &[ParamDecl],
        body: &ParamExpr,
        args: &[mojito_ast::ast::ParamArg],
        mut bindings: mojito_types::param_expr::ParamBindings,
    ) -> Result<(ParamExpr, Vec<TyArg>), TypeError> {
        let arguments = self
            .resolve_use_params(label, decls, args, &[], &[])?
            .arguments;
        self.bind_declared(&mut bindings, decls, &arguments)?;
        let context = &self.param_context;
        let value = context
            .replace(body, &bindings)
            .and_then(|replaced| context.fold(&replaced))
            .map_err(param_error)?;
        Ok((value, arguments))
    }

    /// The parametric value `expr` applies, when it is one: a value-bodied
    /// generic alias (`twice[k]`, `Twice[3]`, `plus_one[n=3]`) or a
    /// struct's parameterized value member (`S[10].plus[2]`,
    /// `Self.get[1]`), bound at the application. `None` for any other
    /// expression.
    pub(super) fn parametric_value_application(
        &self,
        expr: &Expr,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let (object, args) = match &expr.kind {
            ExprKind::TypeApply { name, args } => {
                return self.value_alias_application(name, args).or_else(|| {
                    let object = Expr::new(ExprKind::Identifier(name.clone()), expr.span);
                    self.generator_application_of(&object, args)
                });
            }
            ExprKind::Index { object, index } => (object, subscript_param_args(index)),
            ExprKind::MultiIndex { object, args } => (object, multi_subscript_param_args(args)?),
            _ => return None,
        };
        match &object.kind {
            ExprKind::Identifier(name) => self
                .value_alias_application(name, &args)
                .or_else(|| self.generator_application_of(object, &args)),
            ExprKind::Member {
                object: base,
                field,
            } => self
                .parameterized_member_value(base, field, &args)
                .or_else(|| self.generator_application_of(object, &args)),
            _ => None,
        }
    }

    /// The application of a generator value `object` names to `args`
    /// (`g[4]` over `comptime g = T.get`, `G[3]` over a parameter `G:
    /// __generator_type[Idx: Int] Int`), upstream's `#kgen.bind_params`:
    /// the bracket list is bound against the generator type's parameters
    /// as an alias application's is. `None` when `object` denotes no
    /// generator.
    pub(super) fn generator_application_of(
        &self,
        object: &Expr,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Option<Result<ParamExpr, TypeError>> {
        let generator = match &object.kind {
            ExprKind::Identifier(name) => self.value_parameter_in_scope(name).or_else(|| {
                matches!(self.module_values.get(name), Some(CtValue::Generator(_)))
                    .then(|| self.compile_dependent_ct_expr(object).ok())
                    .flatten()
            })?,
            ExprKind::Member { object, field } => self.type_member_value(object, field)?.ok()?,
            _ => return None,
        };
        let TyKind::Generator(ty) = generator.meta().as_value()?.kind() else {
            return None;
        };
        let label = generator.to_string();
        Some((|| {
            let arguments = self
                .resolve_use_params(&label, &ty.params, args, &[], &[])?
                .arguments;
            let context = &self.param_context;
            let arguments = arguments
                .into_iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Ok(Some(context.type_shape(ty))),
                    TyArg::Val(value) => context.constant(value).map(Some).map_err(param_error),
                    TyArg::Origin(_) => Err(TypeError::Unsupported(
                        "origin parameters on a parameterized comptime value".to_string(),
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            context
                .bind_params(&generator, &arguments)
                .map_err(param_error)
        })())
    }

    /// The application of the value-bodied generic alias `name` to `args`.
    /// `None` when `name` names a binding, a value binder, or no such alias.
    pub(super) fn value_alias_application(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Option<Result<ParamExpr, TypeError>> {
        if self.lookup(name).is_some() || self.value_parameter_in_scope(name).is_some() {
            return None;
        }
        let alias = self.comptime_aliases.get(name)?;
        let AliasBody::Value(body) = &alias.body else {
            return None;
        };
        Some(
            self.apply_parametric_value(
                name,
                &alias.decls,
                body,
                args,
                mojito_types::param_expr::ParamBindings::new(),
            )
            .map(|(value, _)| value),
        )
    }

    /// The value-bodied generic alias `name` read unapplied (`Twice`): the
    /// generator its declaration is, upstream's `#kgen.gen`. `None` when
    /// `name` names a binding or no such alias.
    pub(super) fn alias_generator(&self, name: &str) -> Option<ParamExpr> {
        if self.lookup(name).is_some() {
            return None;
        }
        let alias = self.comptime_aliases.get(name)?;
        if alias.decls.is_empty() {
            return None;
        }
        let body = match &alias.body {
            AliasBody::Value(body) => body.clone(),
            AliasBody::Predicate(_) => alias.predicate_value.clone()?,
            AliasBody::Type(_) => return None,
        };
        self.param_context
            .constant(CtValue::Generator(Box::new(
                mojito_types::ct::GeneratorValue {
                    params: alias.decls.clone(),
                    body,
                },
            )))
            .ok()
    }

    /// Whether `expr` applies a value-bodied generic alias (`First[5, 6]`),
    /// in either spelling the parser gives an application.
    pub(super) fn applies_value_alias(&self, expr: &Expr) -> bool {
        let name = match &expr.kind {
            ExprKind::TypeApply { name, .. } => name,
            ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. } => {
                match &object.kind {
                    ExprKind::Identifier(name) => name,
                    _ => return false,
                }
            }
            _ => return false,
        };
        self.lookup(name).is_none()
            && self
                .comptime_aliases
                .get(name)
                .is_some_and(|alias| matches!(alias.body, AliasBody::Value(_)))
    }

    /// The parameterized value member `field` of the struct `object` names,
    /// applied to `args` at the struct's arguments; the member's own
    /// `where` clauses are checked against both. Through a type parameter
    /// (`T.get[0]`) it is the bound's generator requirement applied, which
    /// the elaborator answers per instance. `None` when `object` names no
    /// type or `field` is no parameterized value member of it.
    pub(super) fn parameterized_member_value(
        &self,
        object: &Expr,
        field: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Option<Result<ParamExpr, TypeError>> {
        let subject = self.member_subject(object)?;
        if let TyKind::Param { .. } = subject.kind() {
            let (params, result) =
                self.lookup_trait_generator(&self.refined_bound(&subject)?, field)?;
            return Some(self.generator_application(&subject, field, params, result, args));
        }
        let TyKind::Struct(name, arguments) = subject.kind() else {
            return None;
        };
        let info = self.structs.get(name)?;
        let member = info.parameterized_associated.get(field)?;
        let MemberTemplate::Value { decls, body } = &member.template else {
            return None;
        };
        let label = format!("{subject}.{field}");
        Some((|| {
            let mut outer = mojito_types::param_expr::ParamBindings::new();
            self.bind_declared(&mut outer, &info.decls, arguments)?;
            let (value, bound) = self.apply_parametric_value(&label, decls, body, args, outer)?;
            self.validate_member_availability(
                &label,
                &member.availability,
                &info.decls,
                arguments,
                decls.iter().map(ParamDecl::name).zip(&bound),
            )?;
            self.reject_generator_dependence(object, field, &value)?;
            Ok(value)
        })())
    }

    /// A bound's generator requirement `field` (`comptime get[idx: Int]:
    /// Int`) applied through the type parameter `subject` to `args`: bound
    /// against the requirement's own parameters (arity, keywords by its
    /// names, kinds, `where` clauses), the read the elaborator answers from
    /// the instance's member.
    fn generator_application(
        &self,
        subject: &Ty,
        field: &str,
        params: &[ParamDecl],
        result: &Ty,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<ParamExpr, TypeError> {
        let label = format!("{subject}.{field}");
        let arguments = self
            .resolve_use_params(&label, params, args, &[], &[])?
            .arguments;
        let context = &self.param_context;
        let arguments = arguments
            .into_iter()
            .map(|argument| match argument {
                TyArg::Ty(ty) => Ok(context.type_shape(ty)),
                TyArg::Val(value) => context.constant(value).map_err(param_error),
                TyArg::Origin(_) => Err(TypeError::Unsupported(
                    "origin parameters on a parameterized comptime value".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(context.type_member(
            &context.type_shape(subject.clone()),
            field,
            &arguments,
            mojito_types::param_expr::MetaTy::value(result.clone()),
        ))
    }

    /// Check a parameterized member's own `where` clauses at an
    /// application: the struct's arguments bound to its declarations, and
    /// the member's, which are not classified in the clause, by spelling.
    /// A clause over arguments still symbolic is checked per instance.
    pub(super) fn validate_member_availability<'a>(
        &self,
        label: &str,
        availability: &[GenericConstraint],
        struct_decls: &[ParamDecl],
        struct_arguments: &'a [TyArg],
        member_arguments: impl IntoIterator<Item = (&'a str, &'a TyArg)>,
    ) -> Result<(), TypeError> {
        if availability.is_empty() {
            return Ok(());
        }
        let mut environment = ConstraintEnvironment::declared(struct_decls, struct_arguments);
        for (name, argument) in member_arguments {
            environment.bind_named(&[], name, argument);
        }
        if environment.arguments().any(tyarg_is_symbolic) {
            return Ok(());
        }
        availability.iter().try_for_each(|constraint| {
            self.validate_constraint_in_environment(label, constraint, &environment)
        })
    }

    /// Evaluate a compile-time expression to the value a struct member, a
    /// bracket argument, or a constant holds: its parameter expression
    /// ([`Self::compile_dependent_ct_expr`]), folded, in the `CtValue`
    /// transport form, a symbolic type included. Member reads, parametric
    /// applications, and type applications record what they denote at their
    /// spans, for MIR to carry where a bracket argument is read as a value.
    pub(super) fn eval_associated_ct(&self, expr: &Expr) -> Result<CtValue, TypeError> {
        self.ct_value(expr, Recording::Record)
    }

    /// [`Self::eval_associated_ct`] of an element of a display, which the
    /// display freezes element by element (R1529).
    pub(super) fn eval_display_element(&self, element: &Expr) -> Result<CtValue, TypeError> {
        self.element_value(element, Recording::Record)
    }

    /// Compile a compile-time expression into its typed parameter
    /// expression, as upstream's parameter emitter does: one name lookup,
    /// with operator semantics and canonical form from [`ParamContext`].
    pub(super) fn compile_dependent_ct_expr(&self, expr: &Expr) -> Result<ParamExpr, TypeError> {
        self.build_ct(expr, Recording::Silent)
    }

    /// The type `expr` denotes in a compile-time position, read by the one
    /// builder: `None` when it builds to a value. A type records what it
    /// denotes at its span (a type operand's fact, its bracket attributes).
    pub(super) fn type_operand_expression(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        if *self.build_ct(expr, Recording::Silent)?.meta() != MetaTy::Type {
            return Ok(None);
        }
        self.build_ct(expr, Recording::Record)
            .map(|built| Some(mojito_types::types::DependentType::resolve(built)))
    }

    /// The type `expr` denotes where only a type is accepted (an annotation,
    /// a type parameter's bracket argument), built by the one builder, as
    /// upstream's `emitExprType` emits the expression and `emitType` rejects
    /// a value ("expected a type, not a value"). Builds silently: the caller
    /// records once the type is accepted.
    pub(super) fn denoted_type(&self, expr: &Expr, context: &str) -> Result<Ty, TypeError> {
        let built = self.build_ct(expr, Recording::Silent)?;
        if *built.meta() != MetaTy::Type {
            return Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: context.to_string(),
            });
        }
        Ok(mojito_types::types::DependentType::resolve(built))
    }

    /// `type_of(operand)`, upstream's `MagicFunctionNode::emitTypeOf`: the
    /// type of the operand, which is typed without being evaluated (inside a
    /// probe, as upstream emits it into a block it then erases). A trait or a
    /// trait composition gives the trait metatype. A type operand's metatype
    /// and a numeric literal value's value-parameterized literal type have
    /// no `Ty`, so each is rejected; a string literal's type carries its
    /// value. In a signature the operand reads the parameters
    /// declared before it ([`Self::in_signature_scope`]).
    pub(super) fn type_of_operand(&self, operand: &Expr) -> Result<Ty, TypeError> {
        if let Some(traits) = self.trait_value(operand) {
            return Ok(TyKind::AnyTrait(traits?).into());
        }
        if self.may_denote_type(operand)
            && self
                .build_ct(operand, Recording::Silent)
                .is_ok_and(|built| *built.meta() == MetaTy::Type)
        {
            return Err(TypeError::Unsupported(
                "'type_of' of a type is not supported yet".to_string(),
            ));
        }
        let depth = self.annotation_depth.replace(0);
        let typed = self.probe(|| {
            self.infer(operand)
                .map(|ty| (ty, self.exact_literal_value(operand).is_some()))
        });
        self.annotation_depth.set(depth);
        let (ty, literal) = typed?;
        // A string literal's type carries its value: `type_of("ab")` is
        // `StringLiteral["ab"]`.
        if matches!(ty.kind(), TyKind::StringLiteral(Some(_))) {
            return Ok(ty);
        }
        if literal
            || matches!(
                ty.kind(),
                TyKind::IntLiteral | TyKind::FloatLiteral | TyKind::StringLiteral(None)
            )
        {
            return Err(TypeError::Unsupported(
                "'type_of' of a literal value is not supported yet".to_string(),
            ));
        }
        Ok(ty)
    }

    /// A type comparison read as a value (`print(T == Int)`, `comptime b =
    /// T == Int`, `show[T == Int]()`): one compile-time `Bool` parameter
    /// value over type-valued operands, recorded at its span for MIR to
    /// read, as upstream's comparison of two types is. `None` when `expr`
    /// compares no type.
    pub(super) fn infer_type_comparison(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        let ExprKind::Infix(
            op
            @ (InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge),
            left,
            right,
        ) = &expr.kind
        else {
            return Ok(None);
        };
        let denotes_type = |operand: &Expr| {
            self.may_denote_type(operand)
                && self
                    .build_ct(operand, Recording::Silent)
                    .is_ok_and(|built| *built.meta() == MetaTy::Type)
        };
        if !denotes_type(left) && !denotes_type(right) {
            return Ok(None);
        }
        self.reject_type_parameter_comparison(*op, left, right)?;
        let value = self.build_ct(expr, Recording::Record)?;
        self.record_member_value(expr.source_span(), value);
        Ok(Some(Ty::bool()))
    }

    /// The member `field` the enclosing struct's shell holds, as resolved
    /// so far: `SIMD`'s too, whose `Self` is the bridged vector.
    fn self_struct_member(&self, field: &str) -> Option<&CtValue> {
        let (name, _) = mojito_types::types::nominal_parts(self.self_ty.as_ref()?)?;
        self.structs.get(name)?.associated.get(field)
    }

    /// An `enum`'s case without a payload (`comptime eof: Self =
    /// Self.__enum_init(__EnumStorage[...](None, index=0))`): the frozen enum
    /// whose storage holds that case, as upstream's parameter domain holds
    /// the value `__enum_init` builds. `None` for any other expression.
    fn frozen_enum_case(&self, expr: &Expr) -> Option<Result<CtValue, TypeError>> {
        let ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } = &expr.kind
        else {
            return None;
        };
        let [storage] = args.as_slice() else {
            return None;
        };
        let ExprKind::Call {
            name,
            param_args,
            args: payload,
            kwargs: index,
        } = &storage.kind
        else {
            return None;
        };
        let [
            mojito_ast::ast::KwArg {
                name: index_name,
                value:
                    Expr {
                        kind: ExprKind::Int(index),
                        ..
                    },
            },
        ] = index.as_slice()
        else {
            return None;
        };
        let Some(TyKind::Struct(enum_name, enum_arguments)) = self.self_ty.as_ref().map(Ty::kind)
        else {
            return None;
        };
        let index = index
            .to_u64()
            .and_then(|index| usize::try_from(index).ok())?;
        (self.enum_declaration
            && method == "__enum_init"
            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
            && kwargs.is_empty()
            && name == mojito_types::types::ENUM_STORAGE_TYPE_NAME
            && matches!(
                payload.as_slice(),
                [Expr {
                    kind: ExprKind::None,
                    ..
                }]
            )
            && index_name == "index"
            && enum_arguments.is_empty())
        .then(|| {
            let (storage, _) = self.enum_storage_type(param_args)?;
            let TyKind::Variant(alternatives) = storage.kind() else {
                return Err(TypeError::InvariantViolation(
                    "an enum's storage is not a variant".to_string(),
                ));
            };
            Ok(CtValue::Struct {
                name: enum_name.clone(),
                fields: vec![(
                    "__enum_storage".to_string(),
                    CtValue::Variant {
                        alternatives: alternatives.clone(),
                        index,
                        payload: None,
                    },
                )],
            })
        })
    }

    /// The `(dtype, width)` a call constructs when it names `SIMD[DType.d, w]`
    /// explicitly or applies a registered vector-type alias (`U256(0)`).
    fn simd_constructor_shape(
        &self,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Option<(Dtype, i64)> {
        if name == "SIMD" {
            if param_args.len() != 2 {
                return None;
            }
            return mojito_types::types::simd_shape(
                &self.simd_type(param_args).ok()?.0.into_kind(),
            );
        }
        if !param_args.is_empty() {
            return None;
        }
        match self.ty_value_from_name(name, &[]).ok().flatten()? {
            CtValue::Type(ty) => match *ty.kind() {
                TyKind::Simd { .. } => mojito_types::types::simd_shape(&ty),
                _ => None,
            },
            _ => None,
        }
    }

    /// A construction of the closed vector `SIMD[dtype, width]` spelled
    /// `name(args…)` that the checker builds itself, staged until Mojito
    /// applies its upstream initializer. `None` for any other construction.
    ///
    /// - One exact literal is upstream's `IntLiteral`/`FloatLiteral`
    ///   initializer, not yet declared (R1507): the `pop.simd.splat` of its
    ///   `pop.cast` that body folds to.
    /// - `width > 1` elements are the `*elems` initializer, whose collector
    ///   is no compile-time application yet: one lane per element.
    /// - A vector alias's one argument (`Int32(n)`), which no compile-time
    ///   call resolves through the alias yet: its lane cast and splat.
    fn staged_simd_construction(
        &self,
        name: &str,
        (dtype, width): (Dtype, i64),
        args: &[Expr],
        recording: Recording,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let vector = |lanes: Result<Vec<CtValue>, TypeError>| {
            lanes
                .and_then(|lanes| simd_construction(dtype, width as usize, &lanes))
                .and_then(|vector| self.ct_param(vector))
        };
        match args {
            [argument] => match self.ct_value(argument, recording) {
                Ok(literal @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_))) => {
                    Some(self.literal_splat(dtype, width, literal))
                }
                value if name != "SIMD" => Some(vector(value.map(|value| vec![value]))),
                _ => None,
            },
            _ if width > 1 && args.len() as i64 == width => Some(vector(
                args.iter()
                    .map(|lane| self.ct_value(lane, recording))
                    .collect(),
            )),
            _ => None,
        }
    }

    /// `pop.simd.splat` of the `pop.cast` of `literal` at `SIMD[dtype,
    /// width]`, upstream's `IntLiteral`/`FloatLiteral` initializer's body.
    fn literal_splat(
        &self,
        dtype: Dtype,
        width: i64,
        literal: CtValue,
    ) -> Result<ParamExpr, TypeError> {
        let context = &self.param_context;
        let scalar = context.type_shape(mojito_types::types::canonical_simd_ty(dtype, 1));
        let vector = context.type_shape(mojito_types::types::canonical_simd_ty(dtype, width));
        let literal = self.ct_param(literal)?;
        context
            .op(ParamOp::Cast, &[scalar, literal])
            .and_then(|cast| context.op(ParamOp::SimdSplat, &[vector, cast]))
            .map_err(|_| {
                TypeError::NotComptime(format!(
                    "a SIMD[DType.{}, {width}] lane needs a compile-time scalar",
                    dtype.name()
                ))
            })
    }

    /// The application a local `comptime NAME = f(…)` binding denotes, which
    /// each of its reads materializes as upstream's alias reference does: the
    /// binding has no storage of its own. `None` for any other binding, and
    /// for a name an inner binding shadows.
    pub(super) fn local_comptime_application(&self, name: &str) -> Option<ParamExpr> {
        let scope = self.binding_scope(name)?;
        if scope == 0 || self.function_bases.is_empty() {
            return None;
        }
        self.local_comptime_parameters
            .get(scope)?
            .get(name)
            .map(|(_, expression)| expression)
            .filter(|expression| {
                matches!(
                    expression.kind(),
                    mojito_types::param_expr::ParamKind::Apply { .. }
                )
            })
            .cloned()
    }

    /// The reference a read of the value binder `name` is, upstream's
    /// `kgen.param.constant` of the binder's declaration: the read takes
    /// no storage. A callable binder, a pack, and a raw parameter list keep
    /// their own readings. `None` for any other binding, and for a name an
    /// inner binding shadows.
    pub(super) fn value_binder_read(&self, name: &str) -> Option<ParamExpr> {
        if !self.is_value_parameter(name)
            || self
                .lookup(name)
                .is_none_or(|ty| mojito_types::types::binder_read_takes_slot(ty.kind()))
        {
            return None;
        }
        self.value_parameter_in_scope(name)
    }

    /// The parameter expression the module constant `name` denotes where a
    /// body reads it, unless a local shadows the name: a folded `Int` is its
    /// literal, an applied constant its application, and a literal its
    /// value. `None` for a constant the elaborator spells by its literal
    /// form.
    pub(super) fn module_constant_value(&self, name: &str) -> Option<ParamExpr> {
        if self.binding_scope(name)? != 0 {
            return None;
        }
        if let Some(applied) = self.comptime_applied.get(name) {
            return Some(applied.clone());
        }
        // A constant whose initializer the check lifted (a display that
        // constructs, a call of a method) is the lifted function's
        // application, bound at the module scope; a local of the name would
        // have shadowed the constant above.
        if let Some((_, lifted)) = self
            .local_comptime_parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_comptime_parameter(name))
        {
            return Some(lifted.clone());
        }
        // A folded constant is a value of its declared type
        // (`comptime TWO: Float64 = 2`).
        let declared = |value: CtValue| {
            let context = &self.param_context;
            match self.lookup(name) {
                Some(ty) if !matches!(ty.kind(), TyKind::IntLiteral) => context
                    .constant_as(value.clone(), ty)
                    .or_else(|_| context.constant(value)),
                _ => context.constant(value),
            }
            .ok()
        };
        match self.module_constant_entry(name)? {
            ModuleConstant::Applied(applied) => Some(applied.clone()),
            ModuleConstant::Int(n) => declared(CtValue::IntLiteral(n.clone())),
            ModuleConstant::Value(value) => declared(value.clone()),
            ModuleConstant::Literal(literal) => self.compile_dependent_ct_expr(literal).ok(),
        }
    }

    /// The table entry the module constant `name` reads from, unless a
    /// nearer binding shadows it: its application, its folded `Int`, its
    /// folded attribute, or its literal initializer, in that order.
    fn module_constant_entry(&self, name: &str) -> Option<ModuleConstant<'_>> {
        if self.module_constant_shadowed(name) {
            return None;
        }
        self.comptime_applied
            .get(name)
            .map(ModuleConstant::Applied)
            .or_else(|| self.comptimes.get(name).map(ModuleConstant::Int))
            .or_else(|| self.module_values.get(name).map(ModuleConstant::Value))
            .or_else(|| {
                self.comptime_literals
                    .get(name)
                    .map(ModuleConstant::Literal)
            })
    }

    /// Whether a module constant `name` has a table entry no nearer
    /// binding shadows.
    pub(super) fn has_module_constant_entry(&self, name: &str) -> bool {
        self.module_constant_entry(name).is_some()
    }

    /// Whether a binding nearer than the module scope (a local, a
    /// parameter) hides the module constant `name`.
    pub(super) fn module_constant_shadowed(&self, name: &str) -> bool {
        self.binding_scope(name).is_some_and(|scope| scope != 0)
    }

    /// The folded `Int` module constant `name`, unless a nearer binding
    /// shadows it.
    pub(super) fn module_comptime(&self, name: &str) -> Option<mojito_common::literal::IntLiteral> {
        (!self.module_constant_shadowed(name))
            .then(|| self.comptimes.get(name).cloned())
            .flatten()
    }

    /// The symbolic value of a module constant's initializer that applies a
    /// function: an application of a module `def` with a declared result to
    /// compile-time arguments, a layout query, an earlier applied constant,
    /// or arithmetic over those. `None` leaves the constant to the ordinary
    /// rejection, so nothing else changes meaning.
    pub(super) fn applied_constant_expr(&self, expr: &Expr) -> Option<ParamExpr> {
        let context = &self.param_context;
        match &expr.kind {
            ExprKind::Int(n) => context.constant(CtValue::IntLiteral(n.clone())).ok(),
            ExprKind::Identifier(name) => self
                .comptimes
                .get(name)
                .and_then(|n| context.constant(CtValue::IntLiteral(n.clone())).ok())
                .or_else(|| self.comptime_applied.get(name).cloned()),
            ExprKind::Prefix(PrefixOp::Neg, operand) => {
                context.neg(&self.applied_constant_expr(operand)?).ok()
            }
            ExprKind::Prefix(PrefixOp::Pos, operand) => self.applied_constant_expr(operand),
            ExprKind::Infix(op, left, right) => context
                .infix(
                    *op,
                    &self.applied_constant_expr(left)?,
                    &self.applied_constant_expr(right)?,
                )
                .ok(),
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty() => {
                if name == mojito_types::param_expr::SIZE_OF_FUNCTION {
                    let ty = self.size_of_operand(param_args, args).ok()?;
                    return Some(context.size_of(ty));
                }
                if !param_args.is_empty() {
                    return None;
                }
                let result = self.module_def_results.get(name)?;
                let meta = MetaTy::value(self.ty_from_anno(result).ok()?);
                let args = args
                    .iter()
                    .map(|argument| self.applied_constant_expr(argument))
                    .collect::<Option<Vec<_>>>()?;
                Some(context.apply(name, &args, meta))
            }
            _ => None,
        }
    }

    /// The typed reference a bare name denotes when it is a value parameter
    /// of an enclosing declaration, innermost first.
    pub(super) fn value_parameter_in_scope(&self, name: &str) -> Option<ParamExpr> {
        let parameter = self
            .vparams
            .iter()
            .take(self.tparams.len())
            .enumerate()
            .rev()
            .find_map(|(level, scope)| scope.get(name).map(|reference| (level, reference)));
        // A local `comptime` binding shadows a parameter of its own body or
        // an enclosing one, not one of a `def` nested below it.
        let local = self
            .local_comptime_parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_comptime_parameter(name))
            .filter(|(level, _)| parameter.is_none_or(|(declared, _)| *level >= declared));
        local
            .map(|(_, expression)| expression)
            .or_else(|| parameter.map(|(_, reference)| reference))
            .map(|expression| self.param_context.intern(expression))
    }

    /// Whether `value` is a bare name of a compile-time value in scope — a
    /// value binder, a `comptime for` index, or a local `comptime` binding of
    /// one — so a `comptime NAME = value` over it is an alias of that
    /// parameter expression, never a type alias.
    pub(super) fn names_value_binder(&self, value: &Expr) -> bool {
        matches!(&value.kind, ExprKind::Identifier(name) if self.value_parameter_in_scope(name).is_some())
    }

    /// Open the scope of a declaration's own binders: its type parameters and,
    /// level for level beside them, its value parameters. `tparams.pop()`
    /// closes both, since a value scope deeper than the type scopes is dead.
    pub(super) fn push_param_scope(&mut self, decls: &[ParamDecl]) {
        self.push_param_scope_as(decls, type_scope(decls));
    }

    /// [`Self::push_param_scope`] with the type binders `scope` spells,
    /// as a `comptime` member's own binders are keyed by member.
    pub(super) fn push_param_scope_as(&mut self, decls: &[ParamDecl], scope: HashMap<String, Ty>) {
        let depth = self.tparams.len();
        self.vparams.truncate(depth);
        self.vparams.resize_with(depth, HashMap::new);
        self.vparams.push(value_scope(decls));
        self.pack_params.truncate(depth);
        self.pack_params.resize_with(depth, HashMap::new);
        self.pack_params.push(pack_scope(decls));
        self.tparams.push(scope);
    }

    /// The list a type pack `decl` binds when its argument forwards a pack
    /// whole (`Len[*Ts]`): the forwarding declaration's own pack, or a list
    /// computed from packs (`*Ts.reverse()`). `None` for any other binding.
    pub(super) fn forwarded_type_pack(
        &self,
        decl: &ParamDecl,
        argument: &TyArg,
    ) -> Option<ParamExpr> {
        let (ParamDecl::Type { variadic: true, .. }, TyArg::Ty(ty)) = (decl, argument) else {
            return None;
        };
        if let Some(list) = mojito_types::types::list_spread(ty) {
            return Some(list.clone());
        }
        let TyKind::Param { binder, .. } =
            mojito_types::types::pack_spread(std::slice::from_ref(ty))?.kind()
        else {
            return None;
        };
        Some(
            self.param_context
                .intern(&ParamContext::detached().decl_ref(
                    binder.id.clone(),
                    binder.name.trim_start_matches('*'),
                    mojito_types::param_expr::MetaTy::type_list(),
                )),
        )
    }

    /// The parameter-list reference a bare name denotes when it is a
    /// variadic value pack of an enclosing declaration that no local
    /// `comptime` binding shadows.
    pub(super) fn value_pack_in_scope(&self, name: &str) -> Option<ParamExpr> {
        let level = (0..self.tparams.len()).rev().find(|level| {
            self.vparams
                .get(*level)
                .is_some_and(|scope| scope.contains_key(name))
        })?;
        let pack = self.pack_params.get(level)?.get(name)?;
        if *pack.meta() == MetaTy::type_list() {
            return None;
        }
        let pack = self.param_context.intern(pack);
        self.value_parameter_in_scope(name)
            .filter(|found| *found == pack)
    }

    /// The parameter-list reference a bare name denotes when it is a variadic
    /// type pack of an enclosing declaration, innermost first: a pack whose
    /// elements are types, upstream's `TypeList`.
    pub(super) fn pack_parameter_in_scope(&self, name: &str) -> Option<ParamExpr> {
        self.pack_params
            .iter()
            .take(self.tparams.len())
            .rev()
            .find_map(|scope| scope.get(name.trim_start_matches('*')))
            .filter(|reference| *reference.meta() == MetaTy::type_list())
            .map(|reference| self.param_context.intern(reference))
    }

    /// The value `Self.<name>` denotes: the instance's bound value inside a
    /// per-instantiation clone, else the template's own symbolic binder
    /// ([`Self::self_param_ct_value`]).
    pub(super) fn self_param_value(&self, name: &str) -> Option<CtValue> {
        if let Some(TyKind::Struct(_, arguments)) = self.self_ty.as_ref().map(Ty::kind)
            && arguments.len() >= self.self_decls.len()
            && let Some(index) = self
                .self_decls
                .iter()
                .position(|d| d.name() == name && matches!(d, ParamDecl::Value { .. }))
            && let Some(TyArg::Val(bound)) = arguments.get(index)
            && !matches!(bound, CtValue::Expr(_))
        {
            return Some(bound.clone());
        }
        self.self_param_ct_value(name)
    }

    pub(super) fn self_param_ct_value(&self, name: &str) -> Option<CtValue> {
        self.self_decls.iter().find_map(|decl| match decl {
            // `Self.values` names the struct's value pack unstarred too.
            ParamDecl::Value {
                name: n,
                ty,
                variadic,
                ..
            } if n == name || (*variadic && n.trim_start_matches('*') == name) => {
                Some(value_parameter(decl, ty))
            }
            // `Self.Ts` names the struct's pack by its unstarred spelling.
            ParamDecl::Type {
                name: n, variadic, ..
            } if n == name || (*variadic && n.trim_start_matches('*') == name) => {
                type_parameter(decl).map(CtValue::Type)
            }
            _ => None,
        })
    }

    /// The type value a name (with bracket arguments) denotes, `None` when
    /// the name is not a type at all. Any other resolution failure — a
    /// misspelled argument, a field in a value slot — is the diagnostic.
    pub(super) fn ty_value_from_name(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<CtValue>, TypeError> {
        Ok(self.applied_type_value(name, args)?.map(|(value, _)| value))
    }

    /// The type value `name[args]` names, beside what each of its bracket
    /// arguments bound to; `None` when `name` names no type.
    pub(super) fn applied_type_value(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<(CtValue, BracketAttributes)>, TypeError> {
        if args.is_empty() {
            // A type value names the nominal `String`, not the compile-time
            // literal type a `[text: String]` value parameter binds.
            if let Some(ty) = scalar_type_name(name).filter(|ty| !ty.is_string_literal()) {
                return Ok(Some((CtValue::Type(ty), Vec::new())));
            }
            if name == "None" {
                return Ok(Some((CtValue::Type(Ty::none()), Vec::new())));
            }
        }
        match self.applied_type(name, args) {
            Ok((ty, attributes)) => Ok(Some((CtValue::Type(ty), attributes))),
            // An unknown argument (`List[Foo]`) is the diagnostic.
            Err(TypeError::UnknownType(unknown)) if unknown == name => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Whether `name` is one of the enclosing struct's own parameters: its
    /// declared parameters, or — inside a value specialization's clone, whose
    /// body may still spell a baked value parameter bare — the template's.
    pub(super) fn is_enclosing_struct_param(&self, name: &str) -> bool {
        let declares = |decls: &[ParamDecl]| {
            decls
                .iter()
                .any(|decl| decl.name().trim_start_matches('*') == name)
        };
        if declares(&self.self_decls) {
            return true;
        }
        let Some(TyKind::Struct(struct_name, _)) = self.self_ty.as_ref().map(Ty::kind) else {
            return false;
        };
        mojito_symbol::symbol::specialization_template(struct_name)
            .and_then(|template| self.structs.get(template))
            .is_some_and(|info| declares(&info.decls))
    }

    /// Upstream's rejection of `Self.<field>` in a compile-time position:
    /// the enclosing struct declares that field, and a field needs an
    /// instance.
    pub(super) fn instance_field_without_instance(&self, field: &str) -> Option<TypeError> {
        let Some(self_ty @ TyKind::Struct(name, _)) = self.self_ty.as_ref().map(Ty::kind) else {
            return None;
        };
        self.structs
            .get(name)
            .is_some_and(|info| info.declared_field_names.iter().any(|f| f == field))
            .then(|| TypeError::InstanceFieldWithoutInstance {
                field: field.to_string(),
                ty: self_ty.to_string(),
            })
    }

    /// The value [`Self::eval_associated_ct`] transports. A tuple or list
    /// is the aggregate of its elements' values, which may be symbolic: a
    /// parameter expression has no aggregate node.
    fn ct_value(&self, expr: &Expr, recording: Recording) -> Result<CtValue, TypeError> {
        if let Some(aggregate) = self.aggregate_value(expr, recording) {
            return aggregate;
        }
        let built = self.build_ct(expr, recording)?;
        let folded = self.param_context.fold(&built).map_err(param_error)?;
        Ok(ct_transport(folded))
    }

    /// The parameter expression of `expr`; see
    /// [`Self::compile_dependent_ct_expr`].
    fn build_ct(&self, expr: &Expr, recording: Recording) -> Result<ParamExpr, TypeError> {
        let context = &self.param_context;
        let record = recording == Recording::Record;
        let build = |expr: &Expr| self.build_ct(expr, recording);
        match &expr.kind {
            _ if let Some(read) = self.display_read(expr) => read,
            // A parametric value's application, recorded for a bracket
            // argument MIR reads as a value.
            _ if let Some(value) = self.parametric_value_application(expr) => {
                let value = value?;
                if record {
                    self.record_member_value(expr.source_span(), value.clone());
                }
                Ok(value)
            }
            ExprKind::Int(n) => self.ct_param(CtValue::IntLiteral(n.clone())),
            ExprKind::Float(value) => self.ct_param(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(b) => self.ct_param(CtValue::Bool(*b)),
            ExprKind::Str(s) => self.ct_param(CtValue::Str(s.clone())),
            ExprKind::TypeValue(ty) => {
                let ty = if record {
                    self.type_value(expr, ty)?
                } else {
                    self.ty_from_anno(ty)?
                };
                Ok(context.type_shape(ty))
            }
            ExprKind::Identifier(name) => self.build_ct_name(name, recording),
            // Upstream's bracketed attribute (`` __mlir_attr[`8`, ` : index`] ``),
            // unless a binding shadows the name.
            ExprKind::TypeApply { name, args }
                if name == "__mlir_attr" && self.lookup(name).is_none() =>
            {
                self.mlir_bracket_attribute(args, recording)
            }
            ExprKind::TypeApply { name, args } => {
                let (ty, attributes) = self
                    .applied_type_value(name, args)?
                    .ok_or_else(|| self.unresolved_head(name))?;
                if record {
                    self.record_bracket_attributes(expr.source_span(), attributes);
                }
                self.ct_param(ty)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == mojito_types::param_expr::SIZE_OF_FUNCTION && kwargs.is_empty() => {
                Ok(context.size_of(self.size_of_operand(param_args, args)?))
            }
            // The length of a pack that is still a parameter, which the
            // instance's elements fix (`len(Self.Ts)`).
            _ if let Some(length) = self.pack_length_query(expr) => Ok(length),
            // `Int(3)`: a scalar conversion of a compile-time value, which is
            // the declared type's ordinary literal materialization. It makes
            // `Array[Int, Int(3)]` and `Array[Int, 3]` one type.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty()
                && param_args.is_empty()
                && args.len() == 1
                && matches!(
                    scalar_type_name(name).as_ref().map(Ty::kind),
                    Some(TyKind::INT | TyKind::UINT | TyKind::FLOAT64 | TyKind::Bool)
                ) =>
            {
                let target = scalar_type_name(name).expect("guard established a scalar type");
                let operand = build(&args[0])?;
                // `Int(m)` of a `SIMDLength` is its lane count: upstream's
                // builtin `SIMD.__init__(SIMDLength)` folded.
                if is_simd_length_value(&operand) && matches!(target.kind(), TyKind::INT) {
                    return mojito_types::types::simd_length_image(context, &operand)
                        .map_err(param_error);
                }
                match ct_transport(operand) {
                    CtValue::Expr(operand) => (*operand.meta() == MetaTy::value(target))
                        .then_some(operand.clone())
                        .ok_or_else(|| {
                            TypeError::NotComptime(format!(
                                "'{name}' conversion of a symbolic '{}' value",
                                operand.meta()
                            ))
                        }),
                    value => {
                        let rendered = value.to_string();
                        let converted = value.materialize_as(&target).ok_or_else(|| {
                            TypeError::NotComptime(format!(
                                "'{name}({rendered})' is not a compile-time {name}"
                            ))
                        })?;
                        self.ct_param(converted)
                    }
                }
            }
            // The constructions of a closed `SIMD[DType.d, w]`, or of a vector
            // alias (`U256(0)`), whose initializer Mojito does not apply yet
            // ([`Self::staged_simd_construction`]); every other one is the
            // application of its declared initializer.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty()
                && let Some((dtype, width)) = self.simd_constructor_shape(name, param_args)
                && let Some(staged) =
                    self.staged_simd_construction(name, (dtype, width), args, recording) =>
            {
                staged
            }
            _ if let Some(frozen) = self.frozen_enum_case(expr) => self.ct_param(frozen?),
            _ if let Some(aggregate) = self.aggregate_value(expr, recording) => {
                self.ct_param(aggregate?)
            }
            // An element of a literal display (`[1, 2][i]`): `param_list.get`
            // over the closed list.
            ExprKind::Index { object, index } if matches!(object.kind, ExprKind::ListLit(_)) => {
                context
                    .list_get(&build(object)?, &build(index)?)
                    .map_err(param_error)
            }
            // A type application whose bracket argument parses as runtime
            // indexing (`Scalar[DType.int32]` — the standing Index-vs-TypeApply
            // parse split for a single non-scalar argument).
            ExprKind::Index { object, index }
                if let ExprKind::Identifier(name) = &object.kind
                    && let Some(ty) = self
                        .ty_value_from_name(name, &subscript_param_args(index))
                        .transpose() =>
            {
                self.ct_param(ty?)
            }
            // An element of the struct's own type pack (`Self.Ts[0]`): its
            // `TypeList`'s `__getitem_param__`, at an index that may read an
            // earlier member (`Self.Ts[Self.width - 1]`).
            ExprKind::Index { object, index } if let Some(pack) = self.type_pack_named(object) => {
                Ok(context.type_shape(self.type_pack_element(&pack, &build(index)?)?))
            }
            // An element of a reflected list (`NAMES[1]` over `comptime
            // NAMES = reflect[P].field_names()`): `param_list.get` over the
            // query's answer.
            ExprKind::Index { object, index }
                if let Ok(Some((_, list))) = self.reflection_list(object) =>
            {
                let list = match list {
                    CtValue::Expr(symbolic) => symbolic,
                    closed => context.constant(closed).map_err(param_error)?,
                };
                context.list_get(&list, &build(index)?).map_err(param_error)
            }
            // An element of a list a constant or a member denotes (`XS[0]`
            // of a module display constant, `l[i]` of a requested local,
            // `Self.values[0]`): `param_list.get` over the list's expression.
            ExprKind::Index { object, index }
                if let Ok(list) = build(object)
                    && matches!(list.meta(), MetaTy::ParamList(_)) =>
            {
                context.list_get(&list, &build(index)?).map_err(param_error)
            }
            // A member application through a type (`TypeList.of[Trait=
            // AnyType, Int]`, `ParameterList.of[...]`).
            ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. }
                if matches!(object.kind, ExprKind::Member { .. })
                    && let Some(ty) = self.member_type_operand(expr) =>
            {
                Ok(context.type_shape(ty))
            }
            // `DType.int`: a compile-time element-type value, as it appears
            // in a `dtype: DType` argument (`_StridedRange[DType.int]`).
            ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")
                    && let Some(dtype) = Dtype::from_name(field) =>
            {
                self.ct_param(CtValue::Dtype(dtype))
            }
            // Upstream's KGEN types (`comptime KS = __mlir_type.`!kgen.string``)
            // and untyped attributes (`` __mlir_attr.`#pop.fmf<fast>` ``).
            ExprKind::Member { object, field }
                if let Some(built) = self.mlir_member(object, field) =>
            {
                built
            }
            // A compile-time `StringLiteral`'s `value` parameter
            // (`comptime none = "None".value`), or a `StringLiteral` value
            // parameter's (`s.value`), which each instance closes.
            ExprKind::Member { object, field }
                if field == mojito_types::ct::LITERAL_VALUE_MEMBER
                    && let Some(literal) = self.string_literal_operand(object, recording) =>
            {
                context
                    .field(&literal, field, MetaTy::Value(TyKind::KgenString.into()))
                    .map_err(param_error)
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") =>
            {
                if let Some(value) = self.self_param_ct_value(field) {
                    return self.ct_param(value);
                }
                // A member of the struct's shell, or a trait's requirement
                // read through its own `Self`.
                if let Some(member) = self.type_member_value(object, field) {
                    let member = member?;
                    if record {
                        self.record_member_value(expr.source_span(), member.clone());
                    }
                    return Ok(member);
                }
                // A type member the shell holds (`comptime m = Self.T2`).
                if let Some(member @ CtValue::Type(_)) = self.self_struct_member(field) {
                    return self.ct_param(member.clone());
                }
                Err(self
                    .instance_field_without_instance(field)
                    .unwrap_or_else(|| TypeError::UnknownSelfParam(field.clone())))
            }
            // A field of a struct-typed parameter value (`Self.e.rows`).
            ExprKind::Member { object, field }
                if let Some(projection) = self.struct_value_field(object, field) =>
            {
                projection
            }
            // A struct parameter of a struct-typed parameter value
            // (`target._mlir_value`).
            ExprKind::Member { object, field }
                if let Some(argument) = self.value_type_argument(object, field) =>
            {
                Ok(argument)
            }
            // A type's value member (`T.K`, `A.K`, `B[4].K`): the member read
            // the elaborator answers per instance, recorded for a bracket
            // argument MIR reads as a value.
            ExprKind::Member { object, field }
                if let Some(member) = self.type_member_value(object, field) =>
            {
                let member = member?;
                if record {
                    self.record_member_value(expr.source_span(), member.clone());
                }
                Ok(member)
            }
            ExprKind::Prefix(PrefixOp::Neg, operand) => {
                self.negated_value(expr, operand, recording)
            }
            ExprKind::Prefix(PrefixOp::Invert, operand) => {
                self.inverted_value(expr, operand, recording)
            }
            // `+operand`: a struct operand applies its `__pos__`, and any
            // other is the operand, as every builtin `__pos__` returns `self`.
            ExprKind::Prefix(PrefixOp::Pos, operand) => {
                let built = self.build_ct(operand, recording)?;
                if struct_valued(&built) {
                    return self.struct_operator_value(
                        expr,
                        PrefixOp::Pos.symbol(),
                        &[(operand, PrefixOp::Pos.dunder(), None)],
                    );
                }
                Ok(built)
            }
            // `not b` over a compile-time `Bool`; any other operand is the
            // ordinary rejection below.
            ExprKind::Prefix(PrefixOp::Not, operand)
                if let Ok(operand) = build(operand)
                    && *operand.meta() == MetaTy::bool() =>
            {
                context.not(&operand).map_err(param_error)
            }
            // `a if c else b`: the arm a closed condition selects, else the
            // selection over both arms, an integer literal arm taking the
            // `Int` the other arm or a bare literal has.
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } if let Ok(condition) = build(cond)
                && *condition.meta() == MetaTy::bool() =>
            {
                match condition.as_bool() {
                    Some(true) => build(then_branch),
                    Some(false) => build(else_branch),
                    None => {
                        let arm = |branch: &Expr| {
                            let value = build(branch)?;
                            match value.as_constant() {
                                Some(literal @ CtValue::IntLiteral(_)) => {
                                    match literal.clone().materialize_as(&Ty::int()) {
                                        Some(int) => self.ct_param(int),
                                        None => Ok(value),
                                    }
                                }
                                _ => Ok(value),
                            }
                        };
                        context
                            .op(
                                mojito_types::param_expr::ParamOp::Cond,
                                &[condition, arm(then_branch)?, arm(else_branch)?],
                            )
                            .map_err(param_error)
                    }
                }
            }
            // `a and b` / `a or b` over a non-`Bool` left operand: the
            // operand `Bool(a)` selects ([`Self::short_circuit_value`]).
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right)
                if let Ok(left) = build(left)
                    && *left.meta() != MetaTy::bool() =>
            {
                self.short_circuit_value(*op, &left, &build(right)?)
            }
            ExprKind::Infix(op, left, right) => self.infix_value(expr, *op, left, right, recording),
            _ => self.build_ct_rest(expr, recording),
        }
    }

    /// The parameter expression of `-operand`, the negation `expr` spells:
    /// a closed operand folds, a struct one applies its `__neg__`
    /// ([`Self::struct_operator_value`]), any other is the negation.
    fn negated_value(
        &self,
        expr: &Expr,
        operand: &Expr,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let built = self.build_ct(operand, recording)?;
        if struct_valued(&built) {
            return self.struct_operator_value(
                expr,
                PrefixOp::Neg.symbol(),
                &[(operand, PrefixOp::Neg.dunder(), None)],
            );
        }
        match built.as_constant() {
            Some(value) => mojito_types::param_expr::fold::fold_neg(value)
                .map_err(param_error)
                .and_then(|value| self.ct_param(value)),
            None => self.param_context.neg(&built).map_err(param_error),
        }
    }

    /// The parameter expression of `~operand`, the inversion `expr` spells:
    /// a closed operand folds, a struct one applies its `__invert__`
    /// ([`Self::struct_operator_value`]), and any other is `operand ^ -1`,
    /// `SIMD.__invert__`'s builtin body as upstream folds it.
    fn inverted_value(
        &self,
        expr: &Expr,
        operand: &Expr,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let built = self.build_ct(operand, recording)?;
        if struct_valued(&built) {
            return self.struct_operator_value(
                expr,
                PrefixOp::Invert.symbol(),
                &[(operand, PrefixOp::Invert.dunder(), None)],
            );
        }
        if let Some(value) = built.as_constant() {
            return mojito_types::param_expr::fold::fold_invert(value)
                .map_err(param_error)
                .and_then(|value| self.ct_param(value));
        }
        let minus_one = self.ct_param(CtValue::IntLiteral(
            mojito_common::literal::IntLiteral::from(-1i64),
        ))?;
        self.param_context
            .infix(InfixOp::BitXor, &built, &minus_one)
            .map_err(param_error)
    }

    /// The parameter expression of `left op right`, which `expr` spells:
    /// closed operands fold through the shared folder; a residual operand
    /// builds the canonical expression, so `Self.n + 1` stays symbolic in a
    /// template. A struct operand applies the operator's dunder on the
    /// left operand, else its reflected dunder on the right one
    /// ([`Self::struct_operator_value`]), as Mojo's parser spells `a + b`
    /// as `a.__add__(b)`.
    fn infix_value(
        &self,
        expr: &Expr,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let (left_built, right_built) = (
            self.build_ct(left, recording)?,
            self.build_ct(right, recording)?,
        );
        // A `SIMDLength` after a vector or a literal converts to the left
        // operand's type, as it does at run time (`2 * w` is `2 * Int(w)`):
        // its lane count, upstream's `SIMD.__init__(SIMDLength)` folded.
        let right_built = if is_simd_length_value(&right_built)
            && left_built
                .meta()
                .as_value()
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Simd { .. } | TyKind::IntLiteral))
        {
            mojito_types::types::simd_length_image(&self.param_context, &right_built)
                .map_err(param_error)?
        } else {
            right_built
        };
        if struct_valued(&left_built) || struct_valued(&right_built) {
            let mut candidates = Vec::new();
            if struct_valued(&left_built)
                && let Some(dunder) = op.dunder()
            {
                candidates.push((left, dunder, Some(right)));
            }
            if struct_valued(&right_built)
                && let Some(dunder) = op.reflected_dunder()
            {
                candidates.push((right, dunder, Some(left)));
            }
            return self.struct_operator_value(expr, op.symbol(), &candidates);
        }
        match (left_built.as_constant(), right_built.as_constant()) {
            (Some(left), Some(right)) => {
                mojito_types::param_expr::fold::fold_comparison(op, left, right)
                    .map_err(param_error)
                    .and_then(|value| self.ct_param(value))
            }
            _ => self
                .param_context
                .infix(op, &left_built, &right_built)
                .map_err(param_error),
        }
    }

    /// The compile-time value of `left and right` or `left or right`, as
    /// upstream folds the operator to `POC::Cond` on `Bool(left)`: `and`
    /// selects `right` when `left` is true and `left` otherwise, `or` the
    /// reverse. Operands of one type, or a literal beside the machine type
    /// it materializes to, give that type; closed operands of any other
    /// pair give the selected operand's `Bool`. A symbolic selection is the
    /// `Cond` over an integer or `Bool` left operand's truth.
    fn short_circuit_value(
        &self,
        op: InfixOp,
        left: &ParamExpr,
        right: &ParamExpr,
    ) -> Result<ParamExpr, TypeError> {
        let (left_ty, right_ty) = (left.meta().as_value(), right.meta().as_value());
        let common = if left_ty == right_ty {
            left_ty
        } else {
            [left_ty, right_ty]
                .into_iter()
                .flatten()
                .find(|ty| !matches!(ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral))
        };
        let materialized = |value: &ParamExpr| match (value.as_constant(), common) {
            (Some(constant), Some(ty)) => constant.clone().materialize_as(ty),
            _ => None,
        };
        if let (Some(lhs), Some(rhs)) = (left.as_constant(), right.as_constant()) {
            let truth = |value: &CtValue| {
                ct_truth(value).ok_or_else(|| TypeError::TypeMismatch {
                    expected: "Bool".to_string(),
                    found: value.to_string(),
                    context: format!("compile-time operand of '{}'", op.symbol()),
                })
            };
            let take_right = truth(lhs)? == (op == InfixOp::And);
            let (selected, value) = if take_right {
                (rhs, right)
            } else {
                (lhs, left)
            };
            return match materialized(value) {
                Some(value) => self.ct_param(value),
                None => self.ct_param(CtValue::Bool(truth(selected)?)),
            };
        }
        let unsupported = || {
            TypeError::Unsupported(format!(
                "'{}' over a symbolic parameter value of another type than its \
                 other operand, or neither an integer nor a Bool, is not supported \
                 yet (roadmap R1604)",
                op.symbol()
            ))
        };
        let operand = |value: &ParamExpr| -> Result<ParamExpr, TypeError> {
            match value.as_constant() {
                None if value.meta().as_value() == common => Ok(value.clone()),
                None => Err(unsupported()),
                Some(_) => materialized(value)
                    .ok_or_else(unsupported)
                    .and_then(|value| self.ct_param(value)),
            }
        };
        let (left, right) = (operand(left)?, operand(right)?);
        let condition = match common.map(Ty::kind) {
            Some(TyKind::Bool) => left.clone(),
            Some(TyKind::INT | TyKind::UINT | TyKind::Index) => {
                let zero = CtValue::IntLiteral(0.into())
                    .materialize_as(common.ok_or_else(unsupported)?)
                    .ok_or_else(unsupported)?;
                self.param_context
                    .infix(InfixOp::Ne, &left, &self.ct_param(zero)?)
                    .map_err(param_error)?
            }
            _ => return Err(unsupported()),
        };
        let (then, orelse) = if op == InfixOp::And {
            (right, left)
        } else {
            (left, right)
        };
        self.param_context
            .op(
                mojito_types::param_expr::ParamOp::Cond,
                &[condition, then, orelse],
            )
            .map_err(param_error)
    }

    /// The proposition a comparison `expr` of `left` and `right` states
    /// when either builds to a struct value: the `Bool` its operator
    /// method's application folds to ([`Self::infix_value`]) is true, as
    /// the pin decides `where L(n) < L(3)` by the builtin `__lt__`. `None`
    /// when neither operand is a struct value.
    fn struct_comparison(
        &self,
        expr: &Expr,
        left: &Expr,
        right: &Expr,
    ) -> Option<Result<GenericConstraint, TypeError>> {
        let _lifting = self.lifting_position();
        let struct_operand = |operand: &Expr| {
            self.compile_dependent_ct_expr(operand)
                .is_ok_and(|built| struct_valued(&built))
        };
        if !struct_operand(left) && !struct_operand(right) {
            return None;
        }
        // A struct value compares through an ordinary operator method (a
        // user `__eq__`) only by running it: the comparison is a value the
        // elaborator evaluates, not a proposition over the operands.
        Some(
            self.compile_dependent_ct_expr(expr)
                .ok()
                .filter(|compared| *compared.meta() == MetaTy::bool())
                .map(|compared| {
                    GenericConstraint::Eq(
                        compared.as_constant().map_or_else(
                            || ConstraintOperand::Expr(compared.clone()),
                            |value| ConstraintOperand::Value(value.clone()),
                        ),
                        ConstraintOperand::Value(CtValue::Bool(true)),
                    )
                })
                .ok_or_else(struct_comparison_rejection),
        )
    }

    /// The application of the first of `candidates`, each a receiver, the
    /// dunder called on it, and its operand, that a compile-time call
    /// applies ([`Self::called_application`]): the operator `symbol` that
    /// `expr` spells over a struct value is its dunder's call. A struct
    /// operand no candidate applies to is no compile-time value, rather
    /// than an arithmetic node over a struct.
    fn struct_operator_value(
        &self,
        expr: &Expr,
        symbol: &str,
        candidates: &[(&Expr, &str, Option<&Expr>)],
    ) -> Result<ParamExpr, TypeError> {
        candidates
            .iter()
            .find_map(|(receiver, dunder, operand)| {
                let mut call = Expr::new(
                    ExprKind::MethodCall {
                        object: Box::new((*receiver).clone()),
                        method: (*dunder).to_string(),
                        args: operand.iter().map(|operand| (*operand).clone()).collect(),
                        kwargs: Vec::new(),
                    },
                    expr.span,
                );
                call.source.clone_from(&expr.source);
                self.called_application(&call)
            })
            .ok_or_else(|| {
                TypeError::NotComptime(format!(
                    "operator '{symbol}' over a struct value applies no \
                     '@always_inline(\"builtin\")' operator method"
                ))
            })
    }

    /// The parameter expression of a compile-time scalar's storage read, as
    /// an `@always_inline("builtin")` body spells its arithmetic: a scalar's
    /// or a vector's `_mlir_value` is the value, since a vector's storage is
    /// its value, `Int(mlir_value=s)` the scalar its storage `s` holds, and
    /// ``__mlir_op.`pop.add`(a, b)`` the operator its op computes
    /// ([`super::builtin_fold::mlir_op_operator`]), and the typed
    /// ``__mlir_op.`pop.simd.splat` `` and ``__mlir_op.`pop.cast` `` are
    /// `ParamOp::SimdSplat` and `ParamOp::Cast` at the `SIMD` whose storage
    /// their `_type=` names. Between an `Int` and the
    /// MLIR builtin `index`, `x.__mlir_index__()` (`pop.cast_to_builtin`,
    /// of an `Int` or an `IntLiteral`) and `Int(mlir_value=i)` over an `index` (`pop.cast_from_builtin`)
    /// are the parameter domain's casts. `None` for any other expression.
    fn scalar_storage_expression(
        &self,
        expr: &Expr,
        recording: Recording,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let scalar = |operand: &Expr| {
            self.build_ct(operand, recording)
                .ok()
                .filter(is_scalar_value)
        };
        match &expr.kind {
            _ if let Some((op, ty, operand)) = super::builtin_fold::typed_lane_op(expr) => {
                let ty = self.lane_op_result(ty)?;
                let operand = self.build_ct(operand, recording).ok()?;
                Some(
                    self.param_context
                        .op(op, &[self.param_context.type_shape(ty), operand])
                        .map_err(param_error),
                )
            }
            _ if let Some(operator) = super::builtin_fold::mlir_op_operator(expr) => {
                Some(self.build_ct(&operator, recording))
            }
            ExprKind::Member { object, field } if field == mojito_types::ct::MLIR_VALUE_FIELD => {
                let vector = |value: &ParamExpr| {
                    value
                        .meta()
                        .as_value()
                        .is_some_and(|ty| mojito_types::types::simd_storage(ty).is_some())
                };
                scalar(object)
                    .or_else(|| self.build_ct(object, recording).ok().filter(vector))
                    .map(Ok)
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "__mlir_index__" && args.is_empty() && kwargs.is_empty() => {
                let value = match self.build_ct(object, recording) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                // A closed `UInt` stores the same bits an `Int` does.
                let value = match value.as_constant() {
                    Some(CtValue::UInt(bits)) => self.ct_param(CtValue::Int(*bits as i64)).ok()?,
                    _ => value,
                };
                if !matches!(
                    value.meta().as_value().map(Ty::kind),
                    Some(TyKind::INT | TyKind::IntLiteral)
                ) {
                    return None;
                }
                Some(
                    self.param_context
                        .cast_to_builtin(&value)
                        .map_err(param_error),
                )
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if param_args.is_empty()
                && args.is_empty()
                && let [storage] = kwargs.as_slice()
                && storage.name == "mlir_value" =>
            {
                let target = scalar_type_name(name)?;
                let value = self.build_ct(&storage.value, recording).ok()?;
                if target == Ty::int()
                    && matches!(value.meta().as_value().map(Ty::kind), Some(TyKind::Index))
                {
                    return Some(
                        self.param_context
                            .cast_from_builtin(&value)
                            .map_err(param_error),
                    );
                }
                Some(value)
                    .filter(|value| {
                        is_scalar_value(value) && value.meta().as_value() == Some(&target)
                    })
                    .map(Ok)
            }
            _ => None,
        }
    }

    /// The `SIMD` type a typed lane op's `_type=` names the storage of:
    /// `T` for `T._mlir_type` (`Self._mlir_type`,
    /// `Scalar[Self.dtype]._mlir_type`), read before the struct's own
    /// `comptime` members resolve, or the vector a spelled `!kgen.simd`
    /// stores.
    fn lane_op_result(&self, attribute: &mojito_ast::ast::ParamArg) -> Option<Ty> {
        if let mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) = attribute
            && field == "_mlir_type"
            && let Ok(ty) = self.denoted_type(object, "attribute '_type'")
            && mojito_types::types::simd_storage(&ty).is_some()
        {
            return Some(ty);
        }
        mojito_types::types::storage_simd(&self.mlir_result_type(attribute).ok()?)
    }

    /// The `StringLiteral` whose `value` parameter `object.value` reads: a
    /// closed literal, or a `StringLiteral` value parameter.
    fn string_literal_operand(&self, object: &Expr, recording: Recording) -> Option<ParamExpr> {
        let literal = self.build_ct(object, recording).ok()?;
        let closed = literal
            .as_constant()
            .is_some_and(|value| value.literal_value().is_some());
        let parameter = matches!(literal.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::StringLiteral(_)));
        (closed || parameter).then_some(literal)
    }

    /// The parameter expression a bare name denotes in a compile-time
    /// position, innermost first: a value parameter or local `comptime`
    /// binding, a module constant, a local `DType` binding, then a type.
    fn build_ct_name(&self, name: &str, recording: Recording) -> Result<ParamExpr, TypeError> {
        // A value parameter of an enclosing declaration is its typed
        // reference: `Buf[n + 1]` stays symbolic in the template. A value
        // binder in scope (a `comptime for` index) shadows a same-named
        // module constant.
        if let Some(reference) = self.value_parameter_in_scope(name) {
            return Ok(reference);
        }
        // A local type alias (`comptime F = Ts[0]`) is the type it names.
        if let Some(ty) = self
            .local_type_aliases
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
        {
            return Ok(self.param_context.type_shape(ty.clone()));
        }
        // A local `comptime` binding of a string literal's KGEN string
        // (`comptime v = "None".value`), of an untyped attribute, or of a
        // closed raw list is inlined.
        if let Some(bound) = self.local_comptime_value(name)
            && (self.reads_literal_value(bound)
                || self.builds_attribute(bound)
                || self.reads_raw_list(bound))
        {
            return self.build_ct(bound, recording);
        }
        // An applied constant is its application in a type position,
        // whatever literal the elaborator folded it to.
        if let Some(entry) = self.module_constant_entry(name) {
            return match entry {
                ModuleConstant::Applied(applied) => Ok(self.param_context.intern(applied)),
                ModuleConstant::Int(n) => self.ct_param(CtValue::IntLiteral(n.clone())),
                ModuleConstant::Value(value) => self.ct_param(value.clone()),
                ModuleConstant::Literal(literal) => self.build_ct(literal, recording),
            };
        }
        // A local `comptime` `DType` binding (`comptime c = pick(True)`) is
        // its dtype, known or the expression a template names.
        if let Some(dtype) = self.comptime_dtype(name) {
            return match dtype {
                SimdDtype::Known(dtype) => self.ct_param(CtValue::Dtype(*dtype)),
                SimdDtype::Expr(expr) => Ok(self.param_context.intern(expr)),
            };
        }
        // The enclosing struct's own parameter is spelled `Self.<name>`
        // inside the body; the bare spelling is upstream's error. Its
        // parameter list reads an earlier type parameter bare.
        if self.is_enclosing_struct_param(name) && self.lookup_tparam(name).is_none() {
            return Err(TypeError::UnqualifiedStructParam(name.to_string()));
        }
        if let Some(generator) = self.alias_generator(name) {
            return Ok(generator);
        }
        // `Self` is the enclosing struct over its own parameters, which a
        // `comptime` member names as its value (`comptime current = Self`).
        if name == "Self"
            && self.lookup(name).is_none()
            && let Some(self_ty) = &self.self_ty
        {
            return self.ct_param(CtValue::Type(self_ty.clone()));
        }
        self.ty_value_from_name(name, &[])?
            .ok_or_else(|| TypeError::NotComptime(name.to_string()))
            .and_then(|ty| self.ct_param(ty))
    }

    /// The rejection of an application or subscript headed by `name` that
    /// reads no compile-time value: an unknown type when `name` names no
    /// declaration in scope, as upstream rejects the use of an unknown
    /// declaration.
    fn unresolved_head(&self, name: &str) -> TypeError {
        let declared = self.lookup(name).is_some()
            || self.local_comptime_value(name).is_some()
            || self.overload_sets.is_function(name)
            || self.traits.contains_key(name)
            || mojito_types::conformance::COMPILER_TRAITS.contains(&name)
            || !matches!(
                self.build_ct_name(name, Recording::Silent),
                Err(TypeError::NotComptime(unknown)) if unknown == name
            );
        if declared {
            TypeError::NotComptime(name.to_string())
        } else {
            TypeError::UnknownType(name.to_string())
        }
    }

    /// [`Self::build_ct`] for a shape no direct arm reads: a `TypeList`
    /// read, a reflection query (`reflect[T].field_count()`, a constant for
    /// a struct subject and a query node for a symbolic one), or a
    /// positioned application, else the shape's rejection.
    fn build_ct_rest(&self, expr: &Expr, recording: Recording) -> Result<ParamExpr, TypeError> {
        // A scalar's storage: `n._mlir_value`, `Int(mlir_value=s)`, and the
        // `__mlir_op` arithmetic over it.
        if let Some(storage) = self.scalar_storage_expression(expr, recording) {
            return storage;
        }
        if let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
            && param_args.is_empty()
            && kwargs.is_empty()
        {
            match (name.as_str(), args.as_slice()) {
                ("conforms_to", [subject, traits]) => {
                    return self.conformance_query(subject, traits);
                }
                ("type_of", [operand]) => {
                    return Ok(self
                        .param_context
                        .type_shape(self.type_of_operand(operand)?));
                }
                ("conforms_to" | "type_of", _) => {
                    return Err(TypeError::ArityMismatch {
                        name: name.clone(),
                        expected: if name == "type_of" { 1 } else { 2 },
                        got: args.len(),
                    });
                }
                _ => {}
            }
        }
        // An element or the length of a `TypeList` value (`r[0]`,
        // `Self.Ts.reverse()[0]`, `TypeList[Ts.values]().length`).
        if let Some(read) = self.type_list_read(expr, recording) {
            return read;
        }
        // A compile-time struct value's subscript is its `__getitem_param__`
        // application, as upstream's `SubscriptNode` over a value is.
        if let ExprKind::Index { object, index } = &expr.kind
            && let Some(value_ty) = self.compile_time_struct_value(object)
        {
            let value_ty = value_ty?;
            return self
                .parameterized_assoc_application(
                    &value_ty,
                    "__getitem_param__",
                    &[mojito_ast::ast::ParamArg::Value(index.as_ref().clone())],
                )?
                .map(|ty| self.param_context.type_shape(ty))
                .ok_or_else(|| TypeError::NotIndexable(value_ty.to_string()));
        }
        if let Some(value) = self.eval_reflection_expr(expr)? {
            return self.ct_param(value);
        }
        // A type's type member (`Meta.KGENTypeType`, `S[Int].Of[Bool]`) is
        // the type it names.
        if matches!(expr.kind, ExprKind::Member { .. })
            && let Some(member) = member_source_type(expr)
                .filter(|member| !matches!(member, SourceType::SelfParam(_)))
            && let Ok(ty) = self.ty_from_anno(&member)
        {
            return Ok(self.param_context.type_shape(ty));
        }
        if let ExprKind::Member { object, field } = &expr.kind {
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType") {
                return Err(TypeError::Unsupported(format!(
                    "unknown DType member '{field}'"
                )));
            }
            // A type application whose own resolution fails reports
            // why (`Box[Self.B].w` over a type member `B`).
            if !matches!(&object.kind, ExprKind::Identifier(_))
                && let Some(Err(error)) = self.member_type_operand_resolution(object)
            {
                return Err(error);
            }
        }
        self.positioned_application(expr).ok_or_else(|| {
            if let Some(violated) = self.violated_application_constraint(expr) {
                return violated;
            }
            match (self.calls_raising_application(expr), &expr.kind) {
                (Some(function), _) => TypeError::BadCall {
                    func: function.to_string(),
                    reason: "cannot call raising function in type parameter".to_string(),
                },
                (None, ExprKind::Member { .. }) => TypeError::NotComptime(
                    "unsupported associated comptime member access".to_string(),
                ),
                (None, ExprKind::Index { object, .. })
                    if let ExprKind::Identifier(name) = &object.kind =>
                {
                    self.unresolved_head(name)
                }
                (None, _) => {
                    TypeError::NotComptime("not an associated comptime expression".to_string())
                }
            }
        })
    }

    /// An element (`r[0]`, `Self.Ts.reverse()[0]`) or the length
    /// (`TypeList[Ts.values]().length`, `len(...)` of the same list) of a
    /// `TypeList` value. `None` when `expr` reads no `TypeList` value.
    fn type_list_read(
        &self,
        expr: &Expr,
        recording: Recording,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let receiver = |object: &Expr| self.typelist_receiver(object).transpose();
        match &expr.kind {
            ExprKind::Index { object, index } => {
                let receiver = match receiver(object)? {
                    Ok(TypeListReceiver::Pack(_)) => return None,
                    receiver => receiver,
                };
                Some(receiver.and_then(|receiver| {
                    let list = self.receiver_list(receiver)?;
                    let index =
                        self.build_ct(index, recording)
                            .map_err(|_| TypeError::TypeMismatch {
                                expected: "a compile-time Int index".to_string(),
                                found: "a runtime value".to_string(),
                                context: "TypeList index".to_string(),
                            })?;
                    self.param_context
                        .list_get(&list, &index)
                        .map_err(param_error)
                }))
            }
            ExprKind::Member { object, field } if field == "length" => {
                Some(receiver(object)?.and_then(|receiver| self.type_list_length(receiver)))
            }
            ExprKind::Call { name, args, .. } if name == "len" && args.len() == 1 => {
                Some(receiver(&args[0])?.and_then(|receiver| self.type_list_length(receiver)))
            }
            _ => None,
        }
    }

    /// The type of the compile-time struct value `expr` denotes: a
    /// construction ([`Self::compile_time_value_type`]), or a name bound to
    /// one or to any compile-time value of a struct type. `None` for any
    /// other expression.
    fn compile_time_struct_value(&self, expr: &Expr) -> Option<Result<Ty, TypeError>> {
        match &expr.kind {
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.compile_time_struct_value(bound),
                None => self
                    .build_ct_name(name, Recording::Silent)
                    .ok()?
                    .meta()
                    .as_value()
                    .filter(|ty| matches!(ty.kind(), TyKind::Struct(..)))
                    .cloned()
                    .map(Ok),
            },
            ExprKind::Call { name, .. }
                if self.structs.contains_key(name) || self.comptime_aliases.contains_key(name) =>
            {
                Some(self.compile_time_value_type(expr))
            }
            ExprKind::Invoke { .. } => Some(self.compile_time_value_type(expr)),
            _ => None,
        }
    }

    /// The length of a `TypeList` value.
    fn type_list_length(&self, receiver: TypeListReceiver) -> Result<ParamExpr, TypeError> {
        let list = self.receiver_list(receiver)?;
        self.param_context.list_length(&list).map_err(param_error)
    }

    /// Whether `expr` is spelled as something that may denote a type: a
    /// name, an application, a type value, or a member or subscript rooted
    /// at a name, none of them a runtime binding.
    fn may_denote_type(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => self.lookup(name).is_none(),
            ExprKind::TypeApply { .. } | ExprKind::TypeValue(_) => true,
            ExprKind::Call { name, .. } => name == "type_of",
            ExprKind::Member { object, .. }
            | ExprKind::Index { object, .. }
            | ExprKind::MultiIndex { object, .. } => self.may_denote_type(object),
            _ => false,
        }
    }

    /// A tuple's or list's value over its elements' values, which may be
    /// symbolic. `None` for any other expression.
    fn aggregate_value(
        &self,
        expr: &Expr,
        recording: Recording,
    ) -> Option<Result<CtValue, TypeError>> {
        let elements = |values: &[Expr]| {
            values
                .iter()
                .map(|value| self.element_value(value, recording))
                .collect::<Result<Vec<_>, _>>()
        };
        match &expr.kind {
            ExprKind::TupleLit(values) => Some(elements(values).map(CtValue::Tuple)),
            ExprKind::ListLit(values) => Some(elements(values).map(CtValue::List)),
            _ => None,
        }
    }

    /// `conforms_to(subject, Tr)`, upstream's compile-time conformance
    /// query, as a `Bool` parameter expression: a closed type's answer, or
    /// the conjunction of `Conforms` propositions over a type the instance
    /// closes, which `native::mono` decides.
    pub(super) fn conformance_query(
        &self,
        subject: &Expr,
        traits: &Expr,
    ) -> Result<ParamExpr, TypeError> {
        let trait_names = mojito_ast::ast::trait_conjunction_names(traits).ok_or_else(|| {
            TypeError::Unsupported(
                "conforms_to takes a trait name, or a '&' conjunction of trait names, as its \
                 second argument"
                    .to_string(),
            )
        })?;
        trait_names
            .iter()
            .try_for_each(|trait_name| self.check_trait_name(trait_name))?;
        let operand = self.comptime_type_operand(subject)?.ok_or_else(|| {
            TypeError::NotComptime("conforms_to takes a type as its first argument".to_string())
        })?;
        let context = &self.param_context;
        let Some(subject) = self.conformance_subject(&operand) else {
            let conforms = !mojito_types::conformance::reflects_no_conformance(&operand)
                && trait_names
                    .iter()
                    .all(|trait_name| self.conforms_to(&operand, trait_name));
            return self.ct_param(CtValue::Bool(conforms));
        };
        let mut propositions = trait_names
            .iter()
            .map(|trait_name| context.conforms(&subject, trait_name).map_err(param_error));
        let first = propositions.next().ok_or_else(|| {
            TypeError::Unsupported("conforms_to takes at least one trait".to_string())
        })??;
        propositions.try_fold(first, |all, proposition| {
            context
                .infix(InfixOp::And, &all, &proposition?)
                .map_err(param_error)
        })
    }

    /// The subject of a `conforms_to` proposition over `ty`: a dependent
    /// type's expression or an open type's shape. `None` for a closed type,
    /// whose conformance is decided now.
    pub(super) fn conformance_subject(&self, ty: &Ty) -> Option<ParamExpr> {
        match ty.kind() {
            TyKind::Dependent(dependent) => Some(dependent.expr().clone()),
            _ if mojito_types::types::has_free_parameters(ty) => {
                Some(self.param_context.type_shape(ty.clone()))
            }
            _ => None,
        }
    }

    /// The value of an element of a display or of a frozen construction,
    /// each frozen element by element (R1529): a closed
    /// construction among them is its frozen struct value, as an aggregate
    /// of applications has no parameter expression.
    fn element_value(&self, value: &Expr, recording: Recording) -> Result<CtValue, TypeError> {
        self.frozen_construction(value, recording)
            .and_then(Result::ok)
            .map_or_else(|| self.ct_value(value, recording), Ok)
    }

    /// `value` as a parameter expression: a type as its shape, closed or
    /// not, and any other value as its constant.
    fn ct_param(&self, value: CtValue) -> Result<ParamExpr, TypeError> {
        match value {
            CtValue::Type(ty) => Ok(self.param_context.type_shape(ty)),
            value => self.param_context.constant(value).map_err(param_error),
        }
    }

    /// The KGEN type `` __mlir_type.`name` `` names, or the attribute
    /// `` __mlir_attr.`text` ``: an untyped one upstream's `!kgen.deferred`
    /// constant, a typed one a constant of its type. `None` for any other
    /// member.
    fn mlir_member(&self, object: &Expr, field: &str) -> Option<Result<ParamExpr, TypeError>> {
        let ExprKind::Identifier(base) = &object.kind else {
            return None;
        };
        match base.as_str() {
            "__mlir_type" => mojito_types::types::mlir_type_named(field)
                .map(|ty| Ok(self.param_context.type_shape(ty))),
            "__mlir_attr" => Some(
                super::comptime_validation::mlir_attribute_expr(&self.param_context, field)
                    .map(|(value, _)| value),
            ),
            _ => None,
        }
    }

    /// The attribute a bracketed `__mlir_attr[…]` spells, its text pieces
    /// concatenated with each operand spliced in, as KGEN reads it. With no
    /// operand it is the attribute the dotted form spells
    /// ([`super::comptime_validation::mlir_attribute`]). With operands, a
    /// `#kgen.param_list.size` of a raw list is its length as an `index`,
    /// and a `#kgen.param_list.get` the list's element at an `index`; any
    /// other attribute with operands is outside the subset.
    fn mlir_bracket_attribute(
        &self,
        args: &[ParamArg],
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let (text, operands) = mojito_types::types::mlir_attribute_spelling(args);
        if operands.is_empty() {
            return super::comptime_validation::mlir_attribute_expr(&self.param_context, &text)
                .map(|(value, _)| value);
        }
        let context = &self.param_context;
        let shape: String = text.split_whitespace().collect();
        match (shape.as_str(), operands.as_slice()) {
            ("#kgen.param_list.size<:{}{}>:index", [list_type, list]) => {
                let list = self.bracket_raw_list(list_type, list, recording)?;
                context
                    .list_length(&list)
                    .and_then(|length| context.cast_to_builtin(&length))
                    .map_err(param_error)
            }
            ("#kgen.param_list.get<:{}{},{}>:{}", [list_type, list, index, element]) => {
                let list = self.bracket_raw_list(list_type, list, recording)?;
                let index = self.bracket_operand(index, recording)?;
                if *index.meta() != MetaTy::value(TyKind::Index.into()) {
                    return Err(TypeError::TypeMismatch {
                        expected: "an 'index' value".to_string(),
                        found: index.meta().to_string(),
                        context: "the index of '#kgen.param_list.get'".to_string(),
                    });
                }
                let element = self.bracket_operand(element, recording)?;
                let element = MetaTy::value(mojito_types::types::DependentType::resolve(element));
                if list.meta() != &MetaTy::ParamList(Box::new(element.clone())) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("an element of {}", list.meta()),
                        found: element.to_string(),
                        context: "the result type of '#kgen.param_list.get'".to_string(),
                    });
                }
                context
                    .cast_from_builtin(&index)
                    .and_then(|index| context.list_get(&list, &index))
                    .map_err(param_error)
            }
            ("#kgen.param.expr<data_to_str,{},{}>:!kgen.string", [string, rest]) => {
                let string = self.bracket_operand(string, recording)?;
                let is_view = |meta: &MetaTy| {
                    matches!(meta, MetaTy::Value(ty) if matches!(
                        ty.kind(),
                        TyKind::Struct(name, _)
                            if mojito_types::types::is_stdlib_string_span_struct(name)
                    ))
                };
                if !is_view(string.meta()) {
                    return Err(TypeError::TypeMismatch {
                        expected: "a 'StringSpan' value".to_string(),
                        found: string.meta().to_string(),
                        context: "the string operand of '#kgen.param.expr<data_to_str>'"
                            .to_string(),
                    });
                }
                let rest = self.bracket_operand(rest, recording)?;
                if !matches!(rest.meta(), MetaTy::ParamList(element) if is_view(element)) {
                    return Err(TypeError::TypeMismatch {
                        expected: "a parameter list of 'StringSpan' values".to_string(),
                        found: rest.meta().to_string(),
                        context: "the list operand of '#kgen.param.expr<data_to_str>'".to_string(),
                    });
                }
                context
                    .op(ParamOp::DataToStr, &[string, rest])
                    .map_err(param_error)
            }
            ("#kgen.param.expr<target_has_feature,{},{}>:i1", [target, name]) => {
                let target = self.bracket_target(target, recording)?;
                let name = self.bracket_operand(name, recording)?;
                if *name.meta() != MetaTy::value(TyKind::KgenString.into()) {
                    return Err(TypeError::TypeMismatch {
                        expected: "a '!kgen.string' value".to_string(),
                        found: name.meta().to_string(),
                        context: "the feature operand of '#kgen.param.expr<target_has_feature>'"
                            .to_string(),
                    });
                }
                context
                    .op(ParamOp::TargetHasFeature, &[target, name])
                    .map_err(param_error)
            }
            (shape, [target]) if let Some(field) = target_field_read(shape) => {
                let target = self.bracket_target(target, recording)?;
                let name = context
                    .constant(CtValue::KgenString(field.to_string()))
                    .map_err(param_error)?;
                context
                    .op(ParamOp::TargetGetField, &[target, name])
                    .map_err(param_error)
            }
            // Whole-value identity of two parameters of one type, a
            // `Bool` (`StringLiteral._is_identical`).
            ("#kgen.param.identical<{},{}>:!kgen.scalar<bool>", [left, right]) => {
                let left = self.bracket_operand(left, recording)?;
                let right = self.bracket_operand(right, recording)?;
                if left.meta() != right.meta() {
                    return Err(TypeError::TypeMismatch {
                        expected: left.meta().to_string(),
                        found: right.meta().to_string(),
                        context: "the operands of '#kgen.param.identical'".to_string(),
                    });
                }
                Ok(context.identical(&left, &right))
            }
            _ => Err(TypeError::Unsupported(format!(
                "the attribute '__mlir_attr[`{text}`]' is outside the subset; only a closed \
                 attribute, '#kgen.param_list.size', '#kgen.param_list.get', \
                 '#kgen.param.expr<data_to_str>', '#kgen.param.expr<target_has_feature>', \
                 '#kgen.param.expr<target_get_field>' of a known field, and \
                 '#kgen.param.identical' are read"
            ))),
        }
    }

    /// An operand of a bracketed `__mlir_attr[…]`, built as a compile-time
    /// expression, where a call is its application
    /// (`_get_kgen_string[name]()`). A `+` before it (`+Self.values`) is
    /// KGEN's splice of the operand into the attribute, not a `__pos__`
    /// call.
    fn bracket_operand(
        &self,
        operand: &ParamArg,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let _lifting = self.lifting_position();
        match operand {
            ParamArg::Value(Expr {
                kind: ExprKind::Prefix(PrefixOp::Pos, spliced),
                ..
            }) => self.build_ct(spliced, recording),
            ParamArg::Value(expr) => self.build_ct(expr, recording),
            // `Self.values`, which a bracket parses as a type argument.
            ParamArg::Type(SourceType::SelfParam(field))
                if let Some(list) = self.self_raw_list(field) =>
            {
                Ok(list)
            }
            // `Self.value`, a value parameter a bracket parses as a type.
            ParamArg::Type(source) if let Some(read) = self.type_spelled_member_read(source) => {
                self.build_ct(&read, recording)
            }
            ParamArg::Type(ty) => self
                .trait_type_arg(operand)
                .unwrap_or_else(|| self.ty_from_anno(ty))
                .map(|ty| self.param_context.type_shape(ty)),
            ParamArg::Named { name, .. } => Err(TypeError::Unsupported(format!(
                "a keyword operand '{name}=' of a bracketed '__mlir_attr' is not supported"
            ))),
        }
    }

    /// The `!kgen.target` operand of a `#kgen.param.expr<target_…>` read.
    fn bracket_target(
        &self,
        target: &ParamArg,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let target = self.bracket_operand(target, recording)?;
        if *target.meta() != MetaTy::value(TyKind::KgenTarget.into()) {
            return Err(TypeError::TypeMismatch {
                expected: "a '!kgen.target' value".to_string(),
                found: target.meta().to_string(),
                context: "the target operand of a '#kgen.param.expr<target_…>' read".to_string(),
            });
        }
        Ok(target)
    }

    /// The raw parameter list a `#kgen.param_list` attribute reads, and the
    /// `KGENParamListType[T]` it spells before it, which must be the list's
    /// own type.
    fn bracket_raw_list(
        &self,
        list_type: &ParamArg,
        list: &ParamArg,
        recording: Recording,
    ) -> Result<ParamExpr, TypeError> {
        let list = self.bracket_operand(list, recording)?;
        if !matches!(list.meta(), MetaTy::ParamList(_)) {
            return Err(TypeError::TypeMismatch {
                expected: "a parameter list".to_string(),
                found: list.meta().to_string(),
                context: "the list operand of a '#kgen.param_list' attribute".to_string(),
            });
        }
        let list_type = self.bracket_operand(list_type, recording)?;
        let list_type = MetaTy::value(mojito_types::types::DependentType::resolve(list_type));
        if list_type != *list.meta() {
            return Err(TypeError::TypeMismatch {
                expected: list.meta().to_string(),
                found: list_type.to_string(),
                context: "the list type of a '#kgen.param_list' attribute".to_string(),
            });
        }
        Ok(list)
    }

    /// The fieldwise construction of a non-generic `@fieldwise_init`
    /// struct from compile-time values (`Q(1, "x")`, or `Self(1, "x")`
    /// inside `Q`) as a frozen struct value, field by field. `None` for any
    /// other expression.
    fn frozen_construction(
        &self,
        expr: &Expr,
        recording: Recording,
    ) -> Option<Result<CtValue, TypeError>> {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return None;
        };
        let name = match name.as_str() {
            "Self" => mojito_types::types::nominal_parts(self.self_ty.as_ref()?)?.0,
            name => name,
        };
        let info = self.structs.get(name).filter(|info| {
            param_args.is_empty()
                && kwargs.is_empty()
                && info.fieldwise_init
                && info.decls.is_empty()
                && args.len() == info.declared_field_names.len()
        })?;
        let fields = info
            .declared_field_names
            .iter()
            .zip(args)
            .map(|(field, argument)| {
                let value = self.element_value(argument, recording)?;
                let value = match info.fields.iter().find(|(name, _)| name == field) {
                    Some((_, ty)) => {
                        let rendered = value.to_string();
                        value.materialize_as(ty).ok_or_else(|| {
                            TypeError::NotComptime(format!(
                                "field '{field}' of '{name}' expects {ty}, got {rendered}"
                            ))
                        })?
                    }
                    None => value,
                };
                Ok((field.clone(), value))
            })
            .collect::<Result<Vec<_>, TypeError>>();
        Some(fields.map(|fields| CtValue::Struct {
            name: name.to_string(),
            fields,
        }))
    }

    /// The parameter expression of an element or the length of a local
    /// display binding (`L[0]`, `len(L)`), which the elaborator evaluates:
    /// a query on the binding's sequence. `None` for any other expression.
    fn display_read(&self, expr: &Expr) -> Option<Result<ParamExpr, TypeError>> {
        match &expr.kind {
            ExprKind::Index { object, index } => {
                self.display_element(self.bound_display(object)?, index)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && kwargs.is_empty() => {
                let [list] = args.as_slice() else {
                    return None;
                };
                Some(self.bound_display(list)?.length(&self.param_context))
            }
            _ => None,
        }
    }

    /// The parameter expression of element `index` of `display`, when a
    /// subscript reads it by position.
    fn display_element(
        &self,
        display: &super::comptime_validation::BoundDisplay,
        index: &Expr,
    ) -> Option<Result<ParamExpr, TypeError>> {
        match self.compile_dependent_ct_expr(index) {
            Ok(index) => display.element(&self.param_context, &index),
            Err(error) => Some(Err(error)),
        }
    }

    /// The projection of `field` out of `object` when `object` compiles to a
    /// struct-typed parameter value declaring that field: upstream's struct
    /// extract on a parameter attribute, folded when the value is closed or
    /// is the struct value a builtin `__init__` stores, whose fields a
    /// builtin body reads before its struct's field types are resolved.
    pub(super) fn struct_value_field(
        &self,
        object: &Expr,
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let base = self.compile_dependent_ct_expr(object).ok()?;
        if let mojito_types::param_expr::ParamKind::StructValue { .. } = base.kind() {
            return Some(
                self.param_context
                    .field(&base, field, base.meta().clone())
                    .map_err(param_error),
            );
        }
        let Some(TyKind::Struct(owner, _)) = base.meta().as_value().map(Ty::kind) else {
            return None;
        };
        // A builtin body folds before the struct's fields register.
        let field_ty = [
            self.structs.get(owner).map(|info| &info.fields),
            self.builtin_struct_fields.get(owner),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .find(|(name, _)| name == field)
        .map(|(_, ty)| ty.clone())?;
        Some(
            self.param_context
                .field(
                    &base,
                    field,
                    mojito_types::param_expr::MetaTy::value(field_ty),
                )
                .map_err(param_error),
        )
    }

    /// The struct parameter `field` read off the compile-time value `object`
    /// of a struct type (`target._mlir_value` over `target:
    /// CompilationTarget`): the argument the value's type binds it to, as
    /// upstream resolves the name against the type's parameters. `None`
    /// when `object` is no such value or its struct has no such parameter.
    fn value_type_argument(&self, object: &Expr, field: &str) -> Option<ParamExpr> {
        let base = self.compile_dependent_ct_expr(object).ok()?;
        let TyKind::Struct(owner, arguments) = base.meta().as_value()?.kind() else {
            return None;
        };
        let owner = mojito_symbol::symbol::specialization_template(owner).unwrap_or(owner);
        let decls = &self.structs.get(owner)?.decls;
        let index = decls.iter().position(|decl| decl.name() == field)?;
        match arguments.get(index)? {
            TyArg::Val(CtValue::Expr(argument)) => Some(argument.clone()),
            TyArg::Val(value) => self.param_context.constant(value.clone()).ok(),
            TyArg::Ty(ty) => Some(self.param_context.type_shape(ty.clone())),
            TyArg::Origin(_) => None,
        }
    }

    /// The compile-time value member `field` of the type `object` names:
    /// upstream's member lookup on a type. Through a type parameter (`T.K`,
    /// `Self.T.K`) it is the member read the elaborator answers per instance,
    /// typed by the bound's requirement; on a struct (`A.K`, `B[4].K`,
    /// `Self.K`) it is the struct's member at the application's arguments,
    /// folded when they are closed. `None` when `object` names no type, or
    /// the member is not a value (an associated type, a method).
    pub(super) fn type_member_value(
        &self,
        object: &Expr,
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        if let Some(pack) = self.value_pack_named(object) {
            return self.parameter_list_member(pack, field);
        }
        if let Some(pack) = self.type_pack_named(object) {
            return self.type_list_member(&pack, field);
        }
        let subject = self.member_subject(object)?;
        match subject.kind() {
            TyKind::Param { .. } => {
                let bounds = self.refined_bound(&subject).unwrap_or_default();
                if let Some(ty) = self.lookup_trait_assoc_value_ty(&bounds, field) {
                    let subject = self.param_context.type_shape(subject.clone());
                    return Some(Ok(self.param_context.type_member(
                        &subject,
                        field,
                        &[],
                        mojito_types::param_expr::MetaTy::value(ty),
                    )));
                }
                // A parameterized requirement read unapplied is the
                // generator the instance's member is (`comptime g = T.get`).
                if let Some((params, result)) = self.lookup_trait_generator(&bounds, field) {
                    let subject = self.param_context.type_shape(subject.clone());
                    return Some(Ok(self.param_context.type_member(
                        &subject,
                        field,
                        &[],
                        mojito_types::param_expr::MetaTy::value(Ty::generator(
                            params.to_vec(),
                            result.clone(),
                        )),
                    )));
                }
                let declared = bounds
                    .iter()
                    .filter_map(|b| self.traits.get(b))
                    .any(|info| {
                        info.comptime_members.contains_key(field)
                            || info.methods.contains_key(field)
                    });
                (!declared).then(|| {
                    Err(TypeError::NoTraitAttribute {
                        trait_name: if bounds.is_empty() {
                            "AnyType".to_string()
                        } else {
                            bounds.join(" & ")
                        },
                        attribute: field.to_string(),
                    })
                })
            }
            // A trait's own `comptime` requirement, read in its scope.
            TyKind::SelfType => {
                let ty = self.trait_self_value_ty(field)?;
                let subject = self.param_context.type_shape(subject.clone());
                Some(Ok(self.param_context.type_member(
                    &subject,
                    field,
                    &[],
                    mojito_types::param_expr::MetaTy::value(ty),
                )))
            }
            // A declared struct's member, a bridged type's (`StringLiteral`'s
            // `value`) included.
            _ => {
                let (name, arguments) = self.nominal_struct(&subject)?;
                let info = self.structs.get(name)?;
                // A type member is no value, but read through a generator it
                // is first rejected for depending on the unbound parameters.
                if let ExprKind::Identifier(base) = &object.kind
                    && let Some(member @ CtValue::Type(_)) = info.associated.get(field)
                    && self.unbound_generator(base).is_some()
                    && let Err(error) = self.reject_unbound_dependence(base, field, member)
                {
                    return Some(Err(error));
                }
                let value = self
                    .struct_value_member(info, &arguments, field)
                    .or_else(|| self.struct_generator_member(&subject, info, &arguments, field))?;
                Some(value.and_then(|value| {
                    self.reject_generator_dependence(object, field, &value)
                        .map(|()| value)
                }))
            }
        }
    }

    /// `ty`, a trait requirement's type, with its abstract `Self` replaced
    /// by `subject`: the conformer whose witness it is matched against, or
    /// the bound type parameter a call reaches it through. A read of the
    /// trait's own value requirement (`Box[Self.K]`) becomes the subject's
    /// member: the conformer's value, or `T.K` through the bound.
    pub(super) fn substitute_self_members(&self, ty: &Ty, subject: &Ty) -> Ty {
        let substituted = super::generics::substitute_self(ty, subject);
        self.answer_member_reads(&substituted, |shape| {
            matches!(
                shape.kind(),
                mojito_types::param_expr::ParamKind::TypeShape(shape)
                    if matches!(shape.kind(), TyKind::SelfType)
            )
            .then(|| subject.clone())
        })
    }

    /// `ty` with every value member read on a concrete struct
    /// (`S[Int, Bool]._types`, `S.K`), as a binder substitution leaves one,
    /// answered by the struct's member at its arguments.
    pub(super) fn answer_concrete_members(&self, ty: &Ty) -> Ty {
        self.answer_member_reads(ty, |shape| {
            let (mojito_types::param_expr::ParamKind::TypeShape(subject)
            | mojito_types::param_expr::ParamKind::Constant(CtValue::Type(subject))) = shape.kind()
            else {
                return None;
            };
            (matches!(subject.kind(), TyKind::Struct(..))
                && !mojito_types::types::is_symbolic(subject))
            .then(|| subject.clone())
        })
    }

    /// The declared type of a type's value member or value parameter read
    /// (`T._types` through a bound, `S[Int, Bool]._types`,
    /// `TypeList.of[Trait=AnyType, Int].values`), at the type's arguments.
    /// `None` when `expr` reads no member whose annotation names a type; a
    /// pack's `ParameterList` / `TypeList` members type through their
    /// values. A local `comptime` binding of a raw list read types as the
    /// read.
    pub(super) fn member_value_type(&self, expr: &Expr) -> Option<Ty> {
        if let ExprKind::Identifier(name) = &expr.kind {
            return self
                .local_comptime_value(name)
                .filter(|bound| self.reads_raw_list(bound))
                .and_then(|bound| self.member_value_type(bound));
        }
        let ExprKind::Member { object, field } = &expr.kind else {
            return None;
        };
        if self.value_pack_named(object).is_some() || self.type_pack_named(object).is_some() {
            return None;
        }
        let subject = self.member_subject(object)?;
        match subject.kind() {
            TyKind::Param { .. } => {
                self.lookup_trait_assoc_value_ty(&self.refined_bound(&subject)?, field)
            }
            TyKind::SelfType => self.trait_self_value_ty(field),
            TyKind::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let declared = info.associated_value_types.get(field).or_else(|| {
                    info.decls.iter().find_map(|decl| match decl {
                        ParamDecl::Value { name, ty, .. } if name == field => Some(ty),
                        _ => None,
                    })
                })?;
                Some(super::generics::substitute_at(declared, info, arguments))
            }
            _ => None,
        }
    }

    /// The value member or value parameter `field` of the `ParameterList` a
    /// value pack is (`vs.size`, `vs.values`), at the pack. `None` when the
    /// bundled struct declares no such value.
    fn parameter_list_member(
        &self,
        pack: ParamExpr,
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let info = self
            .structs
            .get(super::comptime_validation::PARAMETER_LIST)?;
        let mojito_types::param_expr::MetaTy::ParamList(element) = pack.meta() else {
            return None;
        };
        let element = element.as_value()?.clone();
        self.struct_value_member(
            info,
            &[TyArg::Ty(element), TyArg::Val(CtValue::Expr(pack))],
            field,
        )
    }

    /// The value member or value parameter `field` of the `TypeList` a type
    /// pack is (`Ts.length`, `Ts.values`), at the pack. `None` when the
    /// bundled struct declares no such value.
    fn type_list_member(
        &self,
        pack: &ParamExpr,
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let type_list = self.type_pack_type_list(pack)?;
        let TyKind::Struct(name, arguments) = type_list.kind() else {
            return None;
        };
        self.struct_value_member(self.structs.get(name)?, arguments, field)
    }

    /// The value `field` names on the struct application `arguments`: a
    /// value member at the arguments, else the argument a value parameter
    /// of that name holds (`S[7].n`). `None` when `field` names neither.
    pub(super) fn struct_value_member(
        &self,
        info: &StructInfo,
        arguments: &[TyArg],
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        match info.associated.get(field) {
            Some(CtValue::Type(_)) => None,
            Some(value) => Some(self.struct_member_at(info, arguments, value)),
            None => self.struct_parameter_at(info, arguments, field),
        }
    }

    /// A struct's parameterized value member `field` read unapplied
    /// (`A.get`, `S[3].plus`): the generator its declaration is, upstream's
    /// `#kgen.gen`, at the struct's arguments. Over arguments still
    /// symbolic it is the member read the elaborator answers per instance.
    /// `None` when `field` is no parameterized value member.
    fn struct_generator_member(
        &self,
        subject: &Ty,
        info: &StructInfo,
        arguments: &[TyArg],
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let member = info.parameterized_associated.get(field)?;
        let MemberTemplate::Value { decls, body } = &member.template else {
            return None;
        };
        Some((|| {
            let mut outer = mojito_types::param_expr::ParamBindings::new();
            self.bind_declared(&mut outer, &info.decls, arguments)?;
            let context = &self.param_context;
            let body = context
                .replace(body, &outer)
                .and_then(|replaced| context.fold(&replaced))
                .map_err(param_error)?;
            let generator = mojito_types::ct::GeneratorValue {
                params: decls.clone(),
                body,
            };
            if generator_is_closed(&generator) {
                return context
                    .constant(CtValue::Generator(Box::new(generator)))
                    .map_err(param_error);
            }
            Ok(context.type_member(
                &context.type_shape(subject.clone()),
                field,
                &[],
                mojito_types::param_expr::MetaTy::value(generator.ty()),
            ))
        })())
    }

    /// The pack a collector named by `object` spreads (`args` of `*args:
    /// *Ts`), while the pack is still a parameter.
    pub(super) fn collector_pack(&self, object: &Expr) -> Option<ParamExpr> {
        let ExprKind::Identifier(name) = &object.kind else {
            return None;
        };
        let pack = match (self.lookup(name)?).kind() {
            TyKind::VariadicPack(element) => {
                mojito_types::types::pack_spread(std::slice::from_ref(element))
            }
            TyKind::Tuple(elements) => mojito_types::types::pack_spread(elements),
            _ => None,
        }?;
        let TyKind::Param { binder, .. } = pack.kind() else {
            return None;
        };
        self.pack_reference(&binder.name)
    }

    /// The length query of a pack that is still a parameter, when `expr`
    /// asks one ([`Self::pack_length_binder`]).
    pub(super) fn pack_length_query(&self, expr: &Expr) -> Option<ParamExpr> {
        let pack = self.pack_length_binder(expr)?;
        Some(
            self.param_context
                .pack_query(&pack, mojito_types::param_expr::PackQuery::Length),
        )
    }

    /// The pack whose length `expr` asks, as the pin reads it at compile
    /// time: `args.__len__()` of a collector spreading the pack, or `len(Ts)`
    /// of the pack itself. `len(args)` is a runtime value, as it is
    /// upstream; `Ts.length` is the pack's `TypeList` member.
    pub(super) fn pack_length_binder(&self, expr: &Expr) -> Option<ParamRef> {
        let pack = match &expr.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && kwargs.is_empty() => {
                match args.as_slice() {
                    [pack] => self.type_pack_named(pack),
                    _ => None,
                }
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "__len__" && args.is_empty() && kwargs.is_empty() => {
                self.collector_pack(object)
            }
            _ => None,
        }?;
        pack.as_decl_ref().cloned()
    }

    /// Compile a declaration-level `where` clause, retaining the optional
    /// diagnostic carried by current Mojo's `(condition, "message")` form.
    /// Only the outer clause may carry a message; tuple expressions nested
    /// inside a proposition remain unsupported constraint syntax.
    /// `constraint` with each operand [`Self::compile_where_clause`] left
    /// unbound bound to its binder: one of `own`, the declaration the clause
    /// belongs to, else the innermost open scope's, else the enclosing
    /// struct's.
    pub(super) fn bind_constraint(
        &self,
        constraint: &GenericConstraint,
        own: &[ParamDecl],
    ) -> GenericConstraint {
        constraint.map(
            &|reference| self.bind_binder(reference, own),
            &ConstraintOperand::clone,
        )
    }

    /// [`Self::bind_constraint`] over the clauses of every declaration of
    /// `decls`, and their callable defaults bound within `decls`.
    pub(super) fn bind_declared_constraints(&self, decls: &[ParamDecl]) -> Vec<ParamDecl> {
        decls
            .iter()
            .map(|decl| {
                let mut decl = decl.bind_references(&[decls]);
                match &mut decl {
                    ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                        for constraint in constraints {
                            *constraint = self.bind_constraint(constraint, &[]);
                        }
                    }
                }
                decl
            })
            .collect()
    }

    /// A condition of the declaration owning `decls` (a struct's conditional
    /// conformance), compiled and bound to those binders alone. It compiles
    /// the same wherever it is judged: a bare `Ts.all_conforms_to[Trait]()`
    /// is read against `decls`' packs, not the packs the current scope
    /// encloses.
    pub(super) fn compile_condition(
        &self,
        decls: &[ParamDecl],
        condition: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        let compiled = match &condition.kind {
            ExprKind::TupleLit(elements) => match elements.as_slice() {
                [
                    inner,
                    Expr {
                        kind: ExprKind::Str(message),
                        ..
                    },
                ] => GenericConstraint::WithMessage(
                    Box::new(self.compile_declaration_proposition(decls, inner)?),
                    message.clone(),
                ),
                _ => self.compile_where_clause(condition)?,
            },
            _ => self.compile_declaration_proposition(decls, condition)?,
        };
        Ok(compiled.bind(&[decls]))
    }

    /// Compile a `where` clause. An operand names its parameter unbound
    /// ([`ParamRef::unbound`]), except `Self.<name>`, which is the enclosing
    /// struct's; [`Self::bind_constraint`] binds the rest.
    pub(super) fn compile_where_clause(&self, expr: &Expr) -> Result<GenericConstraint, TypeError> {
        let ExprKind::TupleLit(elements) = &expr.kind else {
            return self.compile_generic_constraint(expr);
        };
        let [condition, message] = elements.as_slice() else {
            return Err(TypeError::Unsupported(
                "a diagnostic where clause must be `(condition, \"message\")`".to_string(),
            ));
        };
        let ExprKind::Str(message) = &message.kind else {
            return Err(TypeError::Unsupported(
                "a where-clause diagnostic message must be a string literal".to_string(),
            ));
        };
        Ok(GenericConstraint::WithMessage(
            Box::new(self.compile_generic_constraint(condition)?),
            message.clone(),
        ))
    }

    pub(super) fn compile_generic_constraint(
        &self,
        expr: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        // A predicate-alias application (`MyPred[T]`) inlines the alias's
        // compiled Bool body with the arguments substituted, so the consuming
        // proposition needs no new constraint form. (Registration forbids an
        // alias shadowing the builtin `IsTrivially*` spellings, so this check
        // never preempts them.)
        if let Some((name, args)) = self.predicate_alias_application(expr) {
            let name = name.to_string();
            return self.apply_predicate_alias(&name, &args);
        }
        // A `TypeList` proposition (`TypeList[Ts.values]().all[P]()`,
        // `TypeList.of[...]().contains[T]()`, ...): symbolic pack receivers
        // lower to pack constraint forms, concrete receivers fold eagerly.
        if let Some(constraint) = self.compile_typelist_proposition(expr)? {
            return Ok(constraint);
        }
        let binary = |left: &Expr, right: &Expr| {
            Ok((
                self.constraint_operand(left)?,
                self.constraint_operand(right)?,
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Bool(value) => GenericConstraint::Bool(*value),
            ExprKind::TypeApply { name, args }
                if mojito_types::types::trivial_predicate_name(name).is_some() =>
            {
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                if args.len() != 1 {
                    return Err(TypeError::Unsupported(format!(
                        "{name}[T] takes exactly one type argument"
                    )));
                }
                let (operand, attribute) = self.predicate_operand(&args[0])?;
                self.record_bracket_attributes(expr.source_span(), vec![attribute]);
                GenericConstraint::Trivial(kind, operand)
            }
            // A single non-scalar bracket argument (`IsTriviallyMovable[T]`)
            // parses as runtime indexing; recognize the predicate here too.
            ExprKind::Index { object, index }
                if matches!(
                    &object.kind,
                    ExprKind::Identifier(name)
                        if mojito_types::types::trivial_predicate_name(name).is_some()
                ) =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("guarded above");
                };
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                GenericConstraint::Trivial(kind, self.constraint_operand(index)?)
            }
            ExprKind::Prefix(PrefixOp::Not, value) => {
                GenericConstraint::Not(Box::new(self.compile_generic_constraint(value)?))
            }
            ExprKind::Infix(InfixOp::And, left, right) => GenericConstraint::And(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(InfixOp::Or, left, right) => GenericConstraint::Or(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(op, left, right) => {
                self.reject_type_parameter_comparison(*op, left, right)?;
                if let Some(compared) = self.struct_comparison(expr, left, right) {
                    return compared;
                }
                let (left, right) = binary(left, right)?;
                if [&left, &right].into_iter().any(struct_valued_operand) {
                    return Err(struct_comparison_rejection());
                }
                // A KGEN string implements no comparison, which the shared
                // folder reports (`"a".value == "a".value`).
                if let (ConstraintOperand::Value(a), ConstraintOperand::Value(b)) = (&left, &right)
                    && [a, b]
                        .iter()
                        .any(|value| matches!(value, CtValue::KgenString(_)))
                {
                    mojito_types::param_expr::fold::fold_infix(*op, a, b).map_err(param_error)?;
                }
                match op {
                    InfixOp::Eq => GenericConstraint::Eq(left, right),
                    InfixOp::Ne => GenericConstraint::Ne(left, right),
                    InfixOp::Lt => GenericConstraint::Lt(left, right),
                    InfixOp::Le => GenericConstraint::Le(left, right),
                    InfixOp::Gt => GenericConstraint::Gt(left, right),
                    InfixOp::Ge => GenericConstraint::Ge(left, right),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "unsupported generic where proposition".to_string(),
                        ));
                    }
                }
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let (param, pack) = match (pack_values_projection(&args[0]), &args[0].kind) {
                    (Some(pack), _) => (ParamRef::unbound(pack), true),
                    (None, ExprKind::Identifier(param)) => (ParamRef::unbound(param), false),
                    (None, ExprKind::Member { object, field }) if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                        (self.self_binder(field), false)
                    }
                    _ => {
                        return Err(TypeError::Unsupported(
                            "conforms_to requires a parameter name".to_string(),
                        ));
                    }
                };
                // A composition (`Deinitable & Movable`) is the conjunction
                // of its traits' conformances.
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Err(TypeError::Unsupported(
                        "conforms_to requires a trait name".to_string(),
                    ));
                };
                let conjuncts = trait_names
                    .into_iter()
                    .map(|trait_name| {
                        self.check_trait_name(trait_name)?;
                        let operand = ConstraintOperand::Param(param.clone());
                        let trait_name = trait_name.to_string();
                        Ok(if pack {
                            GenericConstraint::ConformsPack {
                                operand,
                                trait_name,
                            }
                        } else {
                            GenericConstraint::Conforms {
                                operand,
                                trait_name,
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, TypeError>>()?;
                overload_support::fold_constraint_conjunction(&conjuncts)
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic where proposition".to_string(),
                ));
            }
        })
    }

    /// Recognize and lower a `TypeList` Bool proposition in a constraint
    /// position, or `None` when the expression is not TypeList-shaped. The
    /// supported members are the current-vocabulary subset: `any`/`all`
    /// (per-element predicates), `all_conforms_to` (the trait form), and
    /// `contains`; `length` (with its deprecated `size` alias) is an operand,
    /// handled by `constraint_operand`.
    pub(super) fn compile_typelist_proposition(
        &self,
        expr: &Expr,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return Ok(None);
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return Ok(None);
        }
        let ExprKind::Member { object, field } = &callee.kind else {
            return Ok(None);
        };
        let Some(receiver) = self.typelist_receiver(object)? else {
            return Ok(None);
        };
        let single = |what: &str| -> Result<&mojito_ast::ast::ParamArg, TypeError> {
            match param_args.as_slice() {
                [only] => Ok(only),
                _ => Err(TypeError::Unsupported(format!(
                    "TypeList.{what} takes exactly one compile-time argument"
                ))),
            }
        };
        Ok(Some(match field.as_str() {
            "all_conforms_to" => {
                let trait_name = match single("all_conforms_to")? {
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    })
                    | mojito_ast::ast::ParamArg::Type(SourceType::Named(name, _)) => name.as_str(),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "TypeList.all_conforms_to requires a trait name".to_string(),
                        ));
                    }
                };
                self.check_trait_name(trait_name)?;
                match receiver {
                    TypeListReceiver::Pack(param) => GenericConstraint::ConformsPack {
                        operand: ConstraintOperand::Param(param),
                        trait_name: trait_name.to_string(),
                    },
                    TypeListReceiver::Concrete(types) => GenericConstraint::Bool(
                        types.iter().all(|ty| self.conforms_to(ty, trait_name)),
                    ),
                    TypeListReceiver::Derived(list) => {
                        return Err(derived_list_query(&list, field));
                    }
                }
            }
            member @ ("any" | "all") => {
                let all = member == "all";
                let predicate = match single(member)? {
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    })
                    | mojito_ast::ast::ParamArg::Type(SourceType::Named(name, _)) => {
                        if let Some(kind) = mojito_types::types::trivial_predicate_name(name) {
                            mojito_types::types::PackPredicateRef::Trivial(kind)
                        } else if matches!(self.predicate_alias(name), Some((decls, _)) if decls.len() == 1)
                        {
                            mojito_types::types::PackPredicateRef::Alias(name.clone())
                        } else {
                            return Err(TypeError::Unsupported(format!(
                                "TypeList.{member} requires an IsTrivially* predicate or a \
                                 one-parameter Bool-bodied comptime alias"
                            )));
                        }
                    }
                    _ => {
                        return Err(TypeError::Unsupported(format!(
                            "TypeList.{member} requires a predicate name"
                        )));
                    }
                };
                match receiver {
                    TypeListReceiver::Pack(param) => GenericConstraint::PackPredicate {
                        operand: ConstraintOperand::Param(param),
                        predicate,
                        all,
                    },
                    TypeListReceiver::Concrete(types) => {
                        let holds = |ty: &Ty| self.eval_pack_predicate(&predicate, ty);
                        GenericConstraint::Bool(if all {
                            types.iter().all(holds)
                        } else {
                            types.iter().any(holds)
                        })
                    }
                    TypeListReceiver::Derived(list) => {
                        return Err(derived_list_query(&list, field));
                    }
                }
            }
            "contains" => {
                let (element, attribute) = self.predicate_operand(single("contains")?)?;
                self.record_bracket_attributes(expr.source_span(), vec![attribute]);
                match receiver {
                    TypeListReceiver::Pack(param) => GenericConstraint::PackContains {
                        operand: ConstraintOperand::Param(param),
                        element,
                    },
                    TypeListReceiver::Concrete(types) => match element {
                        ConstraintOperand::Type(needle) => {
                            GenericConstraint::Bool(types.contains(&needle))
                        }
                        _ => {
                            return Err(TypeError::Unsupported(
                                "TypeList.contains on a concrete list requires a concrete type"
                                    .to_string(),
                            ));
                        }
                    },
                    TypeListReceiver::Derived(list) => {
                        return Err(derived_list_query(&list, field));
                    }
                }
            }
            _ => return Ok(None),
        }))
    }

    /// The parameter list a `TypeList` receiver denotes.
    pub(super) fn receiver_list(&self, receiver: TypeListReceiver) -> Result<ParamExpr, TypeError> {
        match receiver {
            TypeListReceiver::Pack(pack) => self.pack_reference(&pack.name).ok_or_else(|| {
                TypeError::SymbolicBoundary(format!("pack '{}' is not in scope", pack.name))
            }),
            TypeListReceiver::Concrete(types) => {
                self.param_context.type_list(types).map_err(param_error)
            }
            TypeListReceiver::Derived(list) => Ok(list),
        }
    }

    /// Whether `expr` reads a compile-time `StringLiteral`'s `value`
    /// parameter (`"None".value`, or `s.value` over a value parameter `s`),
    /// a KGEN string with no run-time form.
    pub(super) fn reads_literal_value(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Member { field, .. }
            if field == mojito_types::ct::LITERAL_VALUE_MEMBER)
            && self.compile_dependent_ct_expr(expr).is_ok_and(|value| {
                matches!(value.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::KgenString))
            })
    }

    /// Whether `expr` builds an untyped attribute, upstream's compile-time
    /// `!kgen.deferred` value: `` __mlir_attr.`#…` ``, its bracketed form,
    /// or a call returning one (`flag._mlir_attr()`).
    pub(super) fn builds_attribute(&self, expr: &Expr) -> bool {
        let candidate = match &expr.kind {
            ExprKind::Member { object, .. } => {
                matches!(&object.kind, ExprKind::Identifier(name) if name == "__mlir_attr")
            }
            ExprKind::TypeApply { name, .. } => name == "__mlir_attr",
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => true,
            _ => false,
        };
        // Asking compiles the expression, so a call that builds no
        // attribute (`comptime d = L(6)`) leaves nothing of the question
        // behind for its own binding.
        candidate
            && self.probe(|| {
                let _lifting = self.lifting_position();
                self.compile_dependent_ct_expr(expr).is_ok_and(|value| {
                    matches!(value.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::KgenDeferred))
                })
            })
    }

    /// Whether `expr` reads a closed raw list off a struct's parameter
    /// (`ParameterList.of[4, 5].values`), a compile-time-only value.
    pub(super) fn reads_raw_list(&self, expr: &Expr) -> bool {
        self.member_value_type(expr)
            .is_some_and(|ty| matches!(ty.kind(), TyKind::ParamList(_)))
            && self
                .eval_associated_ct(expr)
                .is_ok_and(|value| matches!(value, CtValue::Tuple(_)))
    }

    /// The list a spread's operand denotes (`Ts.reverse()` in
    /// `Tuple[*Ts.reverse()]`), or `None` when it is no type list.
    pub(super) fn type_list_operand(&self, operand: &Expr) -> Result<Option<ParamExpr>, TypeError> {
        self.typelist_receiver(operand)?
            .map(|receiver| self.receiver_list(receiver))
            .transpose()
    }

    /// The list a `_concat` operand denotes: the `.values` of a type list,
    /// or the element types a specialization wrote in a bound pack's place.
    fn type_list_values(&self, values: &Expr) -> Result<ParamExpr, TypeError> {
        let operand = match &values.kind {
            ExprKind::Member { object, field } if field == "values" => object,
            _ => values,
        };
        self.type_list_operand(operand)?.ok_or_else(|| {
            TypeError::Unsupported("TypeList._concat takes the '.values' of type lists".to_string())
        })
    }

    /// A `TypeList` receiver in a constraint position: an enclosing pack
    /// parameter itself (upstream's `Ts.all_conforms_to[..]()` / `Self.Ts`),
    /// a construction of the bundled struct (`TypeList[Ts.values]()`,
    /// `TypeList[Trait=AnyType, T._types]()`) over the raw list its
    /// `values` binds, or the concrete constructor
    /// (`TypeList.of[Trait=..., T1, ..., Tn]()`) whose element types resolve
    /// immediately.
    pub(super) fn typelist_receiver(
        &self,
        expr: &Expr,
    ) -> Result<Option<TypeListReceiver>, TypeError> {
        let is_enclosing_pack = |name: &str| {
            self.enclosing_type_params
                .iter()
                .any(|parameter| parameter.name.strip_prefix('*') == Some(name))
        };
        let derived = |list: Result<ParamExpr, mojito_types::param_expr::ParamError>| {
            list.map(|list| Some(type_list_receiver(list)))
                .map_err(param_error)
        };
        match &expr.kind {
            ExprKind::Identifier(name) if !is_enclosing_pack(name) => {
                match self.local_comptime_value(name) {
                    Some(bound) => self.typelist_receiver(bound),
                    None => Ok(None),
                }
            }
            ExprKind::Identifier(name) => Ok(Some(TypeListReceiver::Pack(ParamRef::unbound(name)))),
            // The element types a specialization wrote in a bound pack's
            // place.
            ExprKind::TupleLit(elements) => Ok(elements
                .iter()
                .map(|element| self.comptime_type_operand(element).ok().flatten())
                .collect::<Option<Vec<_>>>()
                .map(TypeListReceiver::Concrete)),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "reverse" && args.is_empty() && kwargs.is_empty() => {
                match self.type_list_operand(object)? {
                    Some(list) => derived(self.param_context.list_reverse(&list)),
                    None => Ok(None),
                }
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty()
                && kwargs.is_empty()
                && matches!(&callee.kind, ExprKind::Member { object, field }
                    if field == "_concat"
                        && matches!(&object.kind, ExprKind::Identifier(name) if name == "TypeList")) =>
            {
                let lists = param_args
                    .iter()
                    .map(|argument| match argument {
                        mojito_ast::ast::ParamArg::Value(values) => self.type_list_values(values),
                        _ => Err(TypeError::Unsupported(
                            "TypeList._concat takes the '.values' of type lists".to_string(),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                derived(self.param_context.list_concat(&lists))
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                Ok(is_enclosing_pack(field)
                    .then(|| TypeListReceiver::Pack(self.self_binder(field))))
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == super::comptime_validation::TYPE_LIST
                && args.is_empty()
                && kwargs.is_empty() =>
            {
                let (ty, _) = self.applied_type(name, param_args)?;
                self.type_list_construction(&ty).map_or(Ok(None), |values| {
                    derived(self.param_context.constant(values))
                })
            }
            // A construction through a member alias (`TypeList.of[Trait=
            // AnyType, Int]()`): the list the type it names binds.
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                // The member's base names a struct, applied or not.
                let ExprKind::Member { object, .. } = &callee.kind else {
                    return Ok(None);
                };
                let root = match &object.kind {
                    ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. } => object,
                    _ => object,
                };
                if !matches!(&root.kind, ExprKind::Identifier(name)
                    if self.lookup(name).is_none() && self.structs.contains_key(name))
                {
                    return Ok(None);
                }
                let Some(SourceType::Assoc { base, name, .. }) = member_source_type(callee) else {
                    return Ok(None);
                };
                let ty = self.ty_from_anno(&SourceType::Assoc {
                    base,
                    name,
                    args: param_args.clone(),
                })?;
                self.type_list_construction(&ty).map_or(Ok(None), |values| {
                    derived(self.param_context.constant(values))
                })
            }
            _ => Ok(None),
        }
    }

    /// Lower a Bool-bodied generic comptime alias (a predicate alias). The
    /// body compiles through the ordinary constraint algebra with the alias's
    /// own parameters symbolic. The declaration must stay inside the predicate
    /// subset — no packs, no defaults, no bounds beyond `AnyType` — because an
    /// application inlines only the body: a declared bound the inlined
    /// proposition would not re-check cannot be silently dropped.
    pub(super) fn compile_predicate_alias_body(
        &self,
        decls: &[ParamDecl],
        value: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        for decl in decls {
            let unsupported = |what: &str| {
                TypeError::Unsupported(format!("{what} on a Bool-bodied comptime alias parameter"))
            };
            match decl {
                ParamDecl::Type {
                    bounds,
                    callable_bound,
                    default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        return Err(unsupported("a variadic pack"));
                    }
                    if callable_bound.is_some() {
                        return Err(unsupported("a callable bound"));
                    }
                    if default.is_some() {
                        return Err(unsupported("a default"));
                    }
                    if bounds.iter().any(|bound| bound != "AnyType") {
                        return Err(TypeError::Unsupported(
                            "a Bool-bodied comptime alias parameter takes no bound beyond \
                             AnyType; spell the requirement in the body"
                                .to_string(),
                        ));
                    }
                }
                ParamDecl::Value {
                    default,
                    callable_default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        return Err(unsupported("a variadic pack"));
                    }
                    if default.is_some() || callable_default.is_some() {
                        return Err(unsupported("a default"));
                    }
                }
            }
        }
        let constraint = self
            .compile_generic_constraint(value)
            .map_err(|error| match &error {
                TypeError::Unsupported(message)
                    if message == "unsupported generic where proposition" =>
                {
                    TypeError::Unsupported(
                        "a generic comptime alias must be defined by a type or a Bool proposition"
                            .to_string(),
                    )
                }
                _ => error,
            })?;
        let constraint = constraint.bind(&[decls]);
        let declared: HashSet<&ParamId> = decls
            .iter()
            .map(mojito_types::types::ParamDecl::id)
            .collect();
        validate_predicate_params(&constraint, &declared)?;
        Ok(constraint)
    }

    /// Recognize a predicate-alias application expression (`MyPred[T]`,
    /// spelled as a type application or as bracket indexing, with a tuple
    /// index carrying multiple arguments), returning the alias name and its
    /// arguments. `None` when the expression is not an application of a
    /// registered predicate alias.
    pub(super) fn predicate_alias_application<'e>(
        &self,
        expr: &'e Expr,
    ) -> Option<(&'e str, Vec<mojito_ast::ast::ParamArg>)> {
        match &expr.kind {
            ExprKind::TypeApply { name, args } if self.predicate_alias(name).is_some() => {
                Some((name, args.clone()))
            }
            ExprKind::Index { object, index } => {
                let ExprKind::Identifier(name) = &object.kind else {
                    return None;
                };
                self.predicate_alias(name)?;
                Some((name, subscript_param_args(index)))
            }
            _ => None,
        }
    }

    /// The Bool a predicate-alias application read as a value denotes
    /// (`IsSmall[Int]`, `positive[k]`): its proposition at the arguments,
    /// a constant when they decide it. `None` when `expr` applies no
    /// predicate alias.
    pub(super) fn predicate_alias_value(
        &self,
        expr: &Expr,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let (name, args) = self.predicate_alias_application(expr)?;
        // A value binder an argument spells stays its symbolic reference.
        let symbolic: Vec<(String, TyArg)> = self
            .vparams
            .iter()
            .take(self.tparams.len())
            .flat_map(HashMap::keys)
            .filter_map(|binder| {
                self.value_parameter_in_scope(binder)
                    .map(|reference| (binder.clone(), TyArg::Val(CtValue::Expr(reference))))
            })
            .collect();
        let environment = ConstraintEnvironment::named(
            &[],
            symbolic
                .iter()
                .map(|(binder, argument)| (binder.as_str(), argument)),
        );
        Some(
            self.apply_predicate_alias(name, &args)
                .and_then(|constraint| {
                    let proposition = self
                        .constraint_proposition(&constraint, &environment)
                        .map_err(param_error)?;
                    let mut undecided = false;
                    proposition.visit(&mut |node| {
                        undecided |= matches!(
                            node.kind(),
                            mojito_types::param_expr::ParamKind::Hole { .. }
                        );
                    });
                    if undecided {
                        return Err(TypeError::Unsupported(format!(
                            "the Bool-valued comptime alias '{name}' read as a value over a type \
                     parameter"
                        )));
                    }
                    Ok(proposition)
                }),
        )
    }

    /// The compiled Bool body of a registered predicate alias, or `None` when
    /// the name is unknown or names a type- or value-bodied alias.
    pub(super) fn predicate_alias(&self, name: &str) -> Option<(&[ParamDecl], &GenericConstraint)> {
        let alias = self.comptime_aliases.get(name)?;
        match &alias.body {
            AliasBody::Predicate(constraint) => Some((&alias.decls, constraint)),
            AliasBody::Type(_) | AliasBody::Value(_) => None,
        }
    }

    /// Expand a predicate-alias application into the consuming proposition:
    /// bind each argument to the declared parameter and substitute it through
    /// the alias's compiled body. A type binding folds `conforms_to` to the
    /// answer the binding's type already has.
    pub(super) fn apply_predicate_alias(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<GenericConstraint, TypeError> {
        let (decls, template) = self
            .predicate_alias(name)
            .expect("guarded by predicate_alias");
        if args.len() != decls.len() {
            return Err(TypeError::Unsupported(format!(
                "{name}[...] takes exactly {} argument(s)",
                decls.len()
            )));
        }
        let (decls, template) = (decls.to_vec(), template.clone());
        let mut bindings = HashMap::new();
        for (decl, argument) in decls.iter().zip(args) {
            bindings.insert(decl.id().clone(), self.predicate_operand(argument)?.0);
        }
        self.substitute_predicate(&template, &bindings)
    }

    /// Substitute predicate-alias application bindings through the compiled
    /// body. Operand parameters rename or bake in the bound operand; a
    /// `Conforms` over a parameter bound to a type folds to its truth value.
    fn substitute_predicate(
        &self,
        constraint: &GenericConstraint,
        bindings: &HashMap<ParamId, ConstraintOperand>,
    ) -> Result<GenericConstraint, TypeError> {
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
        let operand = |operand: &ConstraintOperand| match operand {
            ConstraintOperand::Param(param) => bindings.get(&param.id).cloned().ok_or_else(|| {
                TypeError::Unsupported(format!(
                    "a Bool-bodied comptime alias may reference only its own parameters \
                     ('{param}' is not declared)"
                ))
            }),
            other => Ok(other.clone()),
        };
        Ok(match constraint {
            WithMessage(inner, message) => WithMessage(
                Box::new(self.substitute_predicate(inner, bindings)?),
                message.clone(),
            ),
            Bool(value) => Bool(*value),
            Not(inner) => Not(Box::new(self.substitute_predicate(inner, bindings)?)),
            And(left, right) => And(
                Box::new(self.substitute_predicate(left, bindings)?),
                Box::new(self.substitute_predicate(right, bindings)?),
            ),
            Or(left, right) => Or(
                Box::new(self.substitute_predicate(left, bindings)?),
                Box::new(self.substitute_predicate(right, bindings)?),
            ),
            Conforms {
                operand: subject,
                trait_name,
            } => match operand(subject)? {
                ConstraintOperand::Param(param) => Conforms {
                    operand: ConstraintOperand::Param(param),
                    trait_name: trait_name.clone(),
                },
                ConstraintOperand::Type(ty) => Bool(self.conforms_to(&ty, trait_name)),
                ConstraintOperand::Value(_)
                | ConstraintOperand::PackLength(_)
                | ConstraintOperand::Expr(_) => {
                    return Err(TypeError::Unsupported(format!(
                        "conforms_to in a comptime alias requires a type argument for '{subject}'"
                    )));
                }
            },
            ConformsPack { .. } | PackPredicate { .. } | PackContains { .. } => {
                return Err(TypeError::Unsupported(
                    "a pack projection in a Bool-bodied comptime alias".to_string(),
                ));
            }
            Trivial(kind, inner) => Trivial(*kind, operand(inner)?),
            Eq(left, right) => Eq(operand(left)?, operand(right)?),
            Ne(left, right) => Ne(operand(left)?, operand(right)?),
            Lt(left, right) => Lt(operand(left)?, operand(right)?),
            Le(left, right) => Le(operand(left)?, operand(right)?),
            Gt(left, right) => Gt(operand(left)?, operand(right)?),
            Ge(left, right) => Ge(operand(left)?, operand(right)?),
        })
    }

    /// A single argument of a comptime predicate (`IsTrivially*` or a
    /// predicate-alias application): a bare name is a generic parameter
    /// (as `conforms_to` compiles its subject) unless it names a
    /// scalar; any other annotation resolves as a concrete type.
    fn predicate_operand(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<(ConstraintOperand, Option<TyArg>), TypeError> {
        let operand = match argument {
            mojito_ast::ast::ParamArg::Value(expr) => self.constraint_operand(expr)?,
            mojito_ast::ast::ParamArg::Type(SourceType::Named(name, args)) if args.is_empty() => {
                if let Some(ty) = scalar_type_name(name) {
                    ConstraintOperand::Type(ty)
                } else {
                    let attribute = self.lookup_tparam(name).map(TyArg::Ty);
                    return Ok((ConstraintOperand::Param(ParamRef::unbound(name)), attribute));
                }
            }
            // `Self.T` names the struct's binder, as in `conforms_to`.
            mojito_ast::ast::ParamArg::Type(ty @ SourceType::SelfParam(name)) => {
                let attribute = self.ty_from_anno(ty).ok().map(TyArg::Ty);
                return Ok((ConstraintOperand::Param(self.self_binder(name)), attribute));
            }
            mojito_ast::ast::ParamArg::Type(ty) => ConstraintOperand::Type(self.ty_from_anno(ty)?),
            mojito_ast::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "a comptime predicate takes positional arguments".to_string(),
                ));
            }
        };
        let attribute = match &operand {
            ConstraintOperand::Type(ty) => bracket_attribute(argument, &TyArg::Ty(ty.clone())),
            _ => None,
        };
        Ok((operand, attribute))
    }

    pub(super) fn constraint_operand(&self, expr: &Expr) -> Result<ConstraintOperand, TypeError> {
        // `TypeList[Ts.values]().length`, or `len(...)` of the same `Sized`
        // list, is an Int operand (the removed `size` alias rejects like any
        // unknown member).
        let length_receiver = match &expr.kind {
            ExprKind::Member { object, field } if field == "length" => Some(&**object),
            ExprKind::Call { name, args, .. } if name == "len" && args.len() == 1 => Some(&args[0]),
            _ => None,
        };
        // Any other member of a `TypeList` value is unknown, `size` included.
        if let ExprKind::Member { object, field } = &expr.kind
            && field != "length"
            && self.typelist_receiver(object)?.is_some()
        {
            return Err(TypeError::Unsupported(format!(
                "compile-time 'TypeList' value has no field '{field}'"
            )));
        }
        // A type pack's `Ts.length` is its `TypeList`'s member.
        if let ExprKind::Member { object, field } = &expr.kind
            && field == "length"
            && self.type_pack_named(object).is_some()
            && let Some(member) = self.type_member_value(object, field)
        {
            return Ok(ConstraintOperand::Expr(member?));
        }
        if let Some(object) = length_receiver
            && let Some(receiver) = self.typelist_receiver(object)?
        {
            return Ok(match receiver {
                TypeListReceiver::Pack(param) => {
                    ConstraintOperand::PackLength(Box::new(ConstraintOperand::Param(param)))
                }
                TypeListReceiver::Concrete(types) => {
                    ConstraintOperand::Value(CtValue::Int(types.len() as i64))
                }
                TypeListReceiver::Derived(list) => ConstraintOperand::Expr(
                    self.param_context.list_length(&list).map_err(param_error)?,
                ),
            });
        }
        // `args.__len__()` of a collector spreading a pack that is still a
        // parameter is that pack's length.
        if matches!(&expr.kind, ExprKind::MethodCall { .. })
            && let Some(pack) = self.pack_length_binder(expr)
        {
            return Ok(ConstraintOperand::PackLength(Box::new(
                ConstraintOperand::Param(pack),
            )));
        }
        Ok(match &expr.kind {
            // A type operand is the type the one builder reads (`String` the
            // nominal struct, as a `T == String` comparison means it), a type
            // parameter its binder.
            _ if let Some(ty) = self.comptime_type_operand(expr)? => match ty.kind() {
                TyKind::Param { binder, .. } => ConstraintOperand::Param(binder.clone()),
                _ => ConstraintOperand::Type(ty),
            },
            // A local `comptime` over the binders (`comptime k = n + 1`) is
            // the parameter expression it names.
            ExprKind::Identifier(name)
                if let Some(expression) = self.value_parameter_in_scope(name)
                    && expression
                        .as_decl_ref()
                        .is_none_or(|reference| &*reference.name != name) =>
            {
                match expression.as_decl_ref() {
                    Some(reference) => ConstraintOperand::Param(reference.clone()),
                    None => ConstraintOperand::Expr(expression),
                }
            }
            ExprKind::Identifier(name) => ConstraintOperand::Param(ParamRef::unbound(name)),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                ConstraintOperand::Param(self.self_binder(field))
            }
            ExprKind::Int(value) => ConstraintOperand::Value(CtValue::IntLiteral(value.clone())),
            ExprKind::Bool(value) => ConstraintOperand::Value(CtValue::Bool(*value)),
            ExprKind::Str(value) => ConstraintOperand::Value(CtValue::Str(value.clone())),
            // Arithmetic over value parameters (`n + 1`), an element of a
            // compile-time list (`XS[0]`), through the same builder a
            // dependent type argument uses. A name it cannot type — a
            // parameter whose scope is not open here — is the explicit
            // unsupported operand, never an untyped symbol.
            ExprKind::Index { .. }
            | ExprKind::Infix(
                InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Pow
                | InfixOp::Shl,
                _,
                _,
            )
            | ExprKind::Prefix(PrefixOp::Neg, _)
            | ExprKind::Member { .. } => match self.compile_dependent_ct_expr(expr) {
                Ok(expression) => match expression.as_constant() {
                    Some(value) => ConstraintOperand::Value(value.clone()),
                    None => ConstraintOperand::Expr(expression),
                },
                Err(TypeError::NotComptime(_)) => {
                    return Err(TypeError::Unsupported(
                        "unsupported generic constraint operand".to_string(),
                    ));
                }
                Err(error) => return Err(error),
            },
            // A call of a builtin callable is its body's expression.
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Invoke { .. }
                if let Some(expression) = self.called_application(expr).filter(|expression| {
                    !matches!(
                        expression.kind(),
                        mojito_types::param_expr::ParamKind::Apply { .. }
                            | mojito_types::param_expr::ParamKind::StructValue { .. }
                    )
                }) =>
            {
                match expression.as_constant() {
                    Some(value) => ConstraintOperand::Value(value.clone()),
                    None => ConstraintOperand::Expr(expression),
                }
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic constraint operand".to_string(),
                ));
            }
        })
    }

    pub(super) fn validate_generic_constraints(
        &self,
        name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let environment = ConstraintEnvironment::declared(decls, arguments);
        for constraint in decls.iter().flat_map(|decl| match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                constraints.as_slice()
            }
        }) {
            self.validate_constraint_in_environment(name, constraint, &environment)?;
        }
        Ok(())
    }

    /// What a declaration assumes inside its own body: the conjuncts of each
    /// of its `where` clauses that is still a proposition once its own
    /// parameters stand for themselves.
    pub(super) fn declared_propositions(&self, decls: &[ParamDecl]) -> Vec<ParamExpr> {
        let arguments = params_as_args(decls);
        let environment = ConstraintEnvironment::declared(decls, &arguments);
        decls
            .iter()
            .flat_map(|decl| match decl {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.as_slice()
                }
            })
            .filter_map(
                |constraint| match self.constraint_verdict(constraint, &environment) {
                    Ok(ConstraintVerdict::Residual(proposition)) => Some(conjuncts(&proposition)),
                    _ => None,
                },
            )
            .flatten()
            .collect()
    }

    /// [`Self::declared_propositions`] for a method: its own `where`
    /// clauses, over the struct's parameters (`Self.n`) and its own.
    pub(super) fn method_propositions(
        &self,
        where_clauses: &[mojito_ast::ast::Expr],
        method_decls: &[ParamDecl],
    ) -> Vec<ParamExpr> {
        let struct_decls = self.self_decls.clone();
        let struct_arguments = params_as_args(&struct_decls);
        let method_arguments = params_as_args(method_decls);
        let mut environment = ConstraintEnvironment::declared(&struct_decls, &struct_arguments);
        environment.bind_declared(method_decls, &method_arguments);
        where_clauses
            .iter()
            .filter_map(|condition| self.compile_where_clause(condition).ok())
            .map(|constraint| self.bind_constraint(&constraint, method_decls))
            .filter_map(
                |constraint| match self.constraint_verdict(&constraint, &environment) {
                    Ok(ConstraintVerdict::Residual(proposition)) => Some(conjuncts(&proposition)),
                    _ => None,
                },
            )
            .flatten()
            .collect()
    }

    /// The conformance facts a compiled, bound `where` clause guarantees
    /// inside its declaration's body, each the atom upstream records: a
    /// `conforms_to` or `IsTrivially*` over a type parameter, or a pack's
    /// `conforms_to(Ts.values, Trait)`. A disjunction, negation, comparison,
    /// or other pack predicate refines no parameter.
    pub(super) fn guaranteed_facts(
        &self,
        constraint: &GenericConstraint,
        facts: &mut Vec<ParamExpr>,
    ) {
        let context = &self.param_context;
        let shape = |param: &ParamRef| {
            context.type_shape(
                TyKind::Param {
                    binder: param.clone(),
                    bounds: Vec::new(),
                    callable_bound: None,
                }
                .into(),
            )
        };
        match constraint {
            GenericConstraint::WithMessage(condition, _) => {
                self.guaranteed_facts(condition, facts);
            }
            GenericConstraint::And(left, right) => {
                self.guaranteed_facts(left, facts);
                self.guaranteed_facts(right, facts);
            }
            // `conforms_to(Ts.values, Trait)` and `Ts.all_conforms_to[Trait]()`
            // guarantee the trait of every element of the pack.
            GenericConstraint::Conforms {
                operand: ConstraintOperand::Param(param),
                trait_name,
            }
            | GenericConstraint::ConformsPack {
                operand: ConstraintOperand::Param(param),
                trait_name,
            } => facts.extend(if param.name.starts_with('*') {
                Some(context.pack_query(
                    param,
                    mojito_types::param_expr::PackQuery::Conforms(trait_name.clone()),
                ))
            } else {
                context.conforms(&shape(param), trait_name).ok()
            }),
            GenericConstraint::Trivial(kind, ConstraintOperand::Param(param)) => {
                facts.extend(context.trivial(*kind, &shape(param)).ok());
            }
            _ => {}
        }
    }

    /// Open one assumption level holding `level`'s propositions. Every
    /// push is paired with a [`Self::pop_assumptions`].
    pub(super) fn push_assumptions(&self, mut level: Vec<ParamExpr>) {
        let mut seen = HashSet::new();
        level.retain(|proposition| seen.insert(proposition.clone()));
        self.assumptions.borrow_mut().push(level);
    }

    pub(super) fn pop_assumptions(&self) {
        self.assumptions.borrow_mut().pop();
    }

    /// Open one assumption level holding what the compile-time condition
    /// `cond` makes known below it, as a `comptime if` arm and the rest of a
    /// `comptime assert`'s block both know it (the pin's "introducing
    /// knowledge"): the conformances its `conforms_to` atoms under `and`
    /// prove, and each conjunct of the residual proposition `constraint`
    /// (the condition's bound reading, when it closes) states over the
    /// binders in scope. The caller pops the level.
    pub(super) fn assume_condition(
        &self,
        cond: &Expr,
        constraint: Option<&GenericConstraint>,
    ) -> Result<(), TypeError> {
        let mut known = self.conformance_arm_assumptions(cond)?;
        known.extend(
            constraint
                .map(|constraint| self.condition_propositions(constraint))
                .unwrap_or_default(),
        );
        self.push_assumptions(known);
        Ok(())
    }

    /// Whether every conjunct of a residual proposition is one an enclosing
    /// scope assumes. Canonical node identity is the whole proof: there is
    /// no solver behind it.
    pub(super) fn assumptions_prove(&self, proposition: &ParamExpr) -> bool {
        let assumptions = self.assumptions.borrow();
        conjuncts(proposition)
            .iter()
            .all(|conjunct| assumptions.iter().flatten().any(|known| known == conjunct))
    }

    /// The conjuncts of the residual proposition `constraint` states with
    /// every binder in scope standing for itself — the enclosing struct's,
    /// then each open parameter scope's — as a declaration's own `where`
    /// clause is read inside its body. A verdict the binders decide states
    /// nothing to assume.
    fn condition_propositions(&self, constraint: &GenericConstraint) -> Vec<ParamExpr> {
        let declared = |expression: &ParamExpr| match expression.kind() {
            mojito_types::param_expr::ParamKind::DeclRef(reference) => Some(reference.id.clone()),
            _ => None,
        };
        let types = self
            .tparams
            .iter()
            .flat_map(HashMap::values)
            .filter_map(|ty| {
                let TyKind::Param { binder, .. } = ty.kind() else {
                    return None;
                };
                Some((binder.id.clone(), TyArg::Ty(ty.clone())))
            });
        let values = self
            .vparams
            .iter()
            .flat_map(HashMap::values)
            .filter_map(|value| {
                declared(value).map(|id| (id, TyArg::Val(CtValue::Expr(value.clone()))))
            });
        let arguments: Vec<(ParamId, TyArg)> = self
            .self_decls
            .iter()
            .map(|decl| (decl.id().clone(), decl.own_argument()))
            .chain(types)
            .chain(values)
            .collect();
        let mut environment = ConstraintEnvironment::default();
        environment.bound.extend(
            arguments
                .iter()
                .map(|(id, argument)| (id.clone(), argument)),
        );
        match self.constraint_verdict(constraint, &environment) {
            Ok(ConstraintVerdict::Residual(proposition)) => conjuncts(&proposition),
            _ => Vec::new(),
        }
    }

    /// Require `constraint` at one application. Proven passes and Disproven
    /// is the violated constraint. A residual is neither, and the pinned Mojo
    /// requires a proof even under symbolic arguments
    /// (`assets/type_error/param_expr_where_residual.mojo`): it passes only
    /// when the enclosing declaration's own `where` assumes the same
    /// proposition, and is otherwise reported as lacking evidence, never as
    /// false.
    pub(super) fn validate_constraint_in_environment(
        &self,
        name: &str,
        constraint: &GenericConstraint,
        environment: &ConstraintEnvironment<'_>,
    ) -> Result<(), TypeError> {
        let reason = match self.constraint_verdict(constraint, environment)? {
            ConstraintVerdict::Proven => return Ok(()),
            ConstraintVerdict::Residual(proposition) if self.assumptions_prove(&proposition) => {
                return Ok(());
            }
            ConstraintVerdict::Residual(_) => {
                format!("{LACKING_EVIDENCE}cannot prove constraint '{constraint}'")
            }
            ConstraintVerdict::Disproven => match constraint {
                GenericConstraint::WithMessage(_, message) => {
                    format!("constraint failed: {message}")
                }
                violated => super::generics::violated_constraint_reason(violated),
            },
        };
        Err(TypeError::BadCall {
            func: name.to_string(),
            reason,
        })
    }

    /// Validate a constraint on a declaration with no generic argument list,
    /// such as a non-generic `comptime` constant. Parameterized declarations
    /// attach the same constraint to their final [`ParamDecl`] instead so it is
    /// evaluated against each concrete application. A closed declaration has
    /// no later instantiation, so its verdict must be concrete.
    pub(super) fn validate_declaration_constraint(
        &self,
        name: &str,
        constraint: &GenericConstraint,
    ) -> Result<(), TypeError> {
        self.validate_constraint_in_environment(name, constraint, &ConstraintEnvironment::default())
    }

    /// Whether `constraint` is proven under `environment`. A residual is not
    /// a proof, and neither is its negation: a consumer that must choose
    /// (an overload, a conditional conformance) does not choose on it.
    pub(super) fn eval_generic_constraint(
        &self,
        constraint: &GenericConstraint,
        environment: &ConstraintEnvironment<'_>,
    ) -> bool {
        self.constraint_verdict(constraint, environment)
            .is_ok_and(|verdict| verdict.is_proven())
    }

    /// The three-valued verdict of `constraint` under `environment`.
    pub(super) fn constraint_verdict(
        &self,
        constraint: &GenericConstraint,
        environment: &ConstraintEnvironment<'_>,
    ) -> Result<ConstraintVerdict, TypeError> {
        self.constraint_proposition(constraint, environment)
            .map(ConstraintVerdict::from)
            .map_err(param_error)
    }

    /// Lower `constraint` to its Bool proposition under `environment`. Every
    /// leaf the bindings decide is a constant; a leaf over a residual value is
    /// its canonical expression; a leaf whose parameter has no binding is an
    /// unknown proposition. The connectives are the context's, so there is one
    /// three-valued `and`/`or`/`not`.
    fn constraint_proposition(
        &self,
        constraint: &GenericConstraint,
        environment: &ConstraintEnvironment<'_>,
    ) -> Result<ParamExpr, ParamError> {
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
        let context = &self.param_context;
        let unknown = || context.hole(HoleKind::Unknown, MetaTy::bool());
        // A parameter bound to the wrong kind of argument decides the leaf
        // false; only a parameter with no binding at all is unknown.
        // A pack bound to another declaration's pack that is still a
        // parameter (`Tuple[*Self.Ts]`): its elements are that pack's.
        let forwarded = |param: &ParamRef| match environment.get(param) {
            Some(TyArg::Ty(pack)) if matches!(pack.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')) => {
                Some(pack)
            }
            _ => None,
        };
        // A pack query's operand is a pack binder, or an inlined query's
        // bound element list.
        let pack = |operand: &ConstraintOperand, holds: &dyn Fn(&[Ty]) -> bool| match operand {
            ConstraintOperand::Param(param) => match (
                bound_pack_types(environment, param),
                environment.get(param).is_some(),
            ) {
                (Some(types), _) => context.boolean(holds(&types)),
                (None, true) if forwarded(param).is_some() => unknown(),
                (None, true) => context.boolean(false),
                (None, false) => unknown(),
            },
            operand => match self.constraint_value(operand, environment) {
                Some(TyArg::Val(CtValue::Expr(_))) | None => unknown(),
                Some(TyArg::Val(list)) => {
                    context.boolean(type_list(&list).is_some_and(|types| holds(&types)))
                }
                Some(TyArg::Ty(_) | TyArg::Origin(_)) => context.boolean(false),
            },
        };
        Ok(match constraint {
            WithMessage(condition, _) => self.constraint_proposition(condition, environment)?,
            Bool(value) => context.boolean(*value),
            Not(value) => context.not(&self.constraint_proposition(value, environment)?)?,
            And(left, right) => context.op(
                ParamOp::BoolAnd,
                &[
                    self.constraint_proposition(left, environment)?,
                    self.constraint_proposition(right, environment)?,
                ],
            )?,
            Or(left, right) => context.op(
                ParamOp::BoolOr,
                &[
                    self.constraint_proposition(left, environment)?,
                    self.constraint_proposition(right, environment)?,
                ],
            )?,
            Conforms {
                operand,
                trait_name,
            } => match self.constraint_value(operand, environment) {
                // An unknown answer stays the `conforms_to` proposition, as
                // upstream's fold leaves an un-foldable conformance attribute.
                Some(TyArg::Ty(ty)) => match self.conformance(&ty, trait_name) {
                    Conformance::Unknown => match self.conformance_subject(&ty) {
                        Some(subject) => context.conforms(&subject, trait_name)?,
                        None => context.boolean(false),
                    },
                    answer => context.boolean(answer.is_proven()),
                },
                Some(TyArg::Val(_) | TyArg::Origin(_)) => context.boolean(false),
                None => unknown(),
            },
            Trivial(kind, operand) => match self.constraint_value(operand, environment) {
                Some(TyArg::Ty(ty)) if self.is_trivially(*kind, &ty) => context.boolean(true),
                // An archetype's triviality is unknown: its proposition.
                Some(TyArg::Ty(ty))
                    if matches!(
                        ty.kind(),
                        TyKind::Param { .. } | TyKind::Assoc { .. } | TyKind::Dependent(_)
                    ) =>
                {
                    match self.conformance_subject(&ty) {
                        Some(subject) => context.trivial(*kind, &subject)?,
                        None => context.boolean(false),
                    }
                }
                Some(_) => context.boolean(false),
                None => unknown(),
            },
            // Every element of a forwarded pack conforms when that pack's
            // bound or an enclosing `where` guarantees the trait; otherwise
            // nothing is known of them.
            ConformsPack {
                operand: ConstraintOperand::Param(param),
                trait_name,
            } if forwarded(param).is_some_and(|pack| self.conforms_to(pack, trait_name)) => {
                context.boolean(true)
            }
            ConformsPack {
                operand,
                trait_name,
            } => pack(operand, &|types| {
                types.iter().all(|ty| self.conforms_to(ty, trait_name))
            }),
            PackPredicate {
                operand,
                predicate,
                all,
            } => pack(operand, &|types| {
                let mut holds = types
                    .iter()
                    .map(|ty| self.eval_pack_predicate(predicate, ty));
                if *all {
                    holds.all(|held| held)
                } else {
                    holds.any(|held| held)
                }
            }),
            PackContains { operand, element } => {
                match self.constraint_value(element, environment) {
                    Some(TyArg::Ty(needle)) => pack(operand, &|types| types.contains(&needle)),
                    Some(_) => context.boolean(false),
                    None => unknown(),
                }
            }
            Eq(left, right) | Ne(left, right) => {
                let equal = match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right)))
                        if matches!(left, CtValue::Expr(_))
                            || matches!(right, CtValue::Expr(_)) =>
                    {
                        context.infix(
                            InfixOp::Eq,
                            &context.constant(left)?,
                            &context.constant(right)?,
                        )?
                    }
                    (Some(left), Some(right)) => context.boolean(ty_args_equal(&left, &right)),
                    _ => unknown(),
                };
                if matches!(constraint, Ne(..)) {
                    context.not(&equal)?
                } else {
                    equal
                }
            }
            Lt(left, right) | Le(left, right) | Gt(left, right) | Ge(left, right) => {
                let op = match constraint {
                    Lt(..) => InfixOp::Lt,
                    Le(..) => InfixOp::Le,
                    Gt(..) => InfixOp::Gt,
                    _ => InfixOp::Ge,
                };
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right)))
                        if matches!(left, CtValue::Expr(_))
                            || matches!(right, CtValue::Expr(_)) =>
                    {
                        context.infix(op, &context.constant(left)?, &context.constant(right)?)?
                    }
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right))) => {
                        context.boolean(comparison_holds(op, &left, &right).unwrap_or(false))
                    }
                    (Some(_), Some(_)) => context.boolean(false),
                    _ => unknown(),
                }
            }
        })
    }

    pub(super) fn constraint_value(
        &self,
        operand: &ConstraintOperand,
        environment: &ConstraintEnvironment<'_>,
    ) -> Option<TyArg> {
        match operand {
            ConstraintOperand::Param(param) => environment.get(param).cloned(),
            ConstraintOperand::Value(value) => Some(TyArg::Val(value.clone())),
            ConstraintOperand::Type(ty) => Some(TyArg::Ty(ty.clone())),
            ConstraintOperand::PackLength(list) => {
                match self.constraint_value(list, environment)? {
                    TyArg::Val(list) => type_list(&list),
                    TyArg::Ty(_) | TyArg::Origin(_) => None,
                }
                .map(|types| TyArg::Val(CtValue::Int(types.len() as i64)))
            }
            // Replacement binds the application's values and re-folds; what
            // stays symbolic is returned residual.
            ConstraintOperand::Expr(expression) => {
                let context = &self.param_context;
                let mut bindings = mojito_types::param_expr::ParamBindings::new();
                for (id, argument) in &environment.bound {
                    if let TyArg::Val(value) = argument
                        && let Ok(value) = context.constant(value.clone())
                    {
                        match id.owner.strip_prefix(UNBOUND_BINDER_PREFIX) {
                            Some(erased) => bindings.bind_name(erased, value),
                            None => bindings.bind(id.clone(), value),
                        }
                    }
                }
                // As for a type argument, an opaque atom stays unfolded: the
                // pin finds no evidence for `n // -2 == -4` even at `n = 7`
                // (`assets/type_error/param_expr_where_unfolded_atom.mojo`).
                context
                    .replace(expression, &bindings)
                    .ok()
                    .map(|replaced| TyArg::Val(replaced.into_value()))
            }
        }
    }

    /// Evaluate a `TypeList` per-element predicate against one concrete
    /// element type.
    fn eval_pack_predicate(
        &self,
        predicate: &mojito_types::types::PackPredicateRef,
        ty: &Ty,
    ) -> bool {
        match predicate {
            mojito_types::types::PackPredicateRef::Trivial(kind) => self.is_trivially(*kind, ty),
            mojito_types::types::PackPredicateRef::Alias(name) => {
                let Some((decls, template)) = self.predicate_alias(name) else {
                    return false;
                };
                let [only] = decls else {
                    return false;
                };
                let bindings =
                    HashMap::from([(only.id().clone(), ConstraintOperand::Type(ty.clone()))]);
                let template = template.clone();
                self.substitute_predicate(&template, &bindings)
                    .is_ok_and(|constraint| {
                        self.eval_generic_constraint(&constraint, &ConstraintEnvironment::default())
                    })
            }
        }
    }

    /// The enclosing struct's binder `Self.<name>` names.
    fn self_binder(&self, name: &str) -> ParamRef {
        mojito_types::types::bind_reference(&ParamRef::unbound(name), &[&self.self_decls])
    }

    fn bind_binder(&self, reference: &ParamRef, own: &[ParamDecl]) -> ParamRef {
        if !reference.is_unbound() {
            return reference.clone();
        }
        let bound = mojito_types::types::bind_reference(reference, &[own]);
        if !bound.is_unbound() {
            return bound;
        }
        self.binder_in_scope(&reference.name).map_or_else(
            || mojito_types::types::bind_reference(reference, &[&self.self_decls]),
            |id| ParamRef {
                id,
                name: reference.name.clone(),
            },
        )
    }

    /// The binder a bare name denotes in the open parameter scopes,
    /// innermost first: a type parameter, a value parameter, or a pack.
    fn binder_in_scope(&self, name: &str) -> Option<ParamId> {
        let spelling = name.trim_start_matches('*');
        let declared = |expression: &ParamExpr| match expression.kind() {
            mojito_types::param_expr::ParamKind::DeclRef(reference) => Some(reference.id.clone()),
            _ => None,
        };
        (0..self.tparams.len()).rev().find_map(|level| {
            let types = &self.tparams[level];
            types
                .get(spelling)
                .or_else(|| types.get(&format!("*{spelling}")))
                .and_then(|ty| match ty.kind() {
                    TyKind::Param { binder, .. } => Some(binder.id.clone()),
                    _ => None,
                })
                .or_else(|| self.vparams.get(level)?.get(spelling).and_then(declared))
                .or_else(|| {
                    self.pack_params
                        .get(level)?
                        .get(spelling)
                        .and_then(declared)
                })
        })
    }

    /// One proposition of [`Self::compile_condition`]: its connectives
    /// recursed into, and a pack conformance over a pack `decls` declares
    /// compiled against that pack.
    fn compile_declaration_proposition(
        &self,
        decls: &[ParamDecl],
        expr: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        let declares_pack = |pack: &str| {
            decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Type { variadic: true, .. })
                    && decl.name().trim_start_matches('*') == pack
            })
        };
        Ok(match &expr.kind {
            ExprKind::Prefix(PrefixOp::Not, value) => GenericConstraint::Not(Box::new(
                self.compile_declaration_proposition(decls, value)?,
            )),
            ExprKind::Infix(InfixOp::And, left, right) => GenericConstraint::And(
                Box::new(self.compile_declaration_proposition(decls, left)?),
                Box::new(self.compile_declaration_proposition(decls, right)?),
            ),
            ExprKind::Infix(InfixOp::Or, left, right) => GenericConstraint::Or(
                Box::new(self.compile_declaration_proposition(decls, left)?),
                Box::new(self.compile_declaration_proposition(decls, right)?),
            ),
            _ => match pack_conformance_atom(expr) {
                Some((pack, trait_name)) if declares_pack(pack) => {
                    self.check_trait_name(trait_name)?;
                    GenericConstraint::ConformsPack {
                        operand: ConstraintOperand::Param(ParamRef::unbound(pack)),
                        trait_name: trait_name.to_string(),
                    }
                }
                _ => self.compile_generic_constraint(expr)?,
            },
        })
    }

    /// [`Self::reject_unbound_dependence`] of a member read whose base
    /// `object` is a bare parameterized struct, read as its generator.
    fn reject_generator_dependence(
        &self,
        object: &Expr,
        field: &str,
        value: &ParamExpr,
    ) -> Result<(), TypeError> {
        match &object.kind {
            ExprKind::Identifier(name) if self.unbound_generator(name).is_some() => {
                self.reject_unbound_dependence(name, field, &CtValue::Expr(value.clone()))
            }
            _ => Ok(()),
        }
    }

    /// The value member `field` of `subject`: a struct's at its arguments,
    /// or the member read through a type parameter's bound. `None` when
    /// `subject` declares no such value.
    /// `ty` with every argument-free member read whose subject `subject_of`
    /// maps to a type answered by that type's value member; `ty` as it was
    /// when an answer fails to rebuild.
    fn answer_member_reads(&self, ty: &Ty, subject_of: impl Fn(&ParamExpr) -> Option<Ty>) -> Ty {
        struct Answer<'a, F> {
            checker: &'a Checker,
            subject_of: F,
        }

        impl<F: Fn(&ParamExpr) -> Option<Ty>> mojito_types::types::TyRewrite for Answer<'_, F> {
            fn expr(
                &mut self,
                expr: &ParamExpr,
            ) -> Result<ParamExpr, mojito_types::param_expr::ParamError> {
                self.checker
                    .param_context
                    .answer_type_members(expr, &mut |shape, name, args| {
                        Ok(args
                            .is_empty()
                            .then(|| (self.subject_of)(shape))
                            .flatten()
                            .and_then(|subject| self.checker.subject_value_member(&subject, name))
                            .map(ParamExpr::into_value))
                    })
            }
        }

        let mut answer = Answer {
            checker: self,
            subject_of,
        };
        mojito_types::types::rewrite_ty(ty, &mut answer).unwrap_or_else(|_| ty.clone())
    }

    fn subject_value_member(&self, subject: &Ty, field: &str) -> Option<ParamExpr> {
        match subject.kind() {
            TyKind::Struct(name, arguments) => self
                .struct_value_member(self.structs.get(name)?, arguments, field)?
                .ok(),
            TyKind::Param { .. } => {
                let ty = self.lookup_trait_assoc_value_ty(&self.refined_bound(subject)?, field)?;
                Some(self.param_context.type_member(
                    &self.param_context.type_shape(subject.clone()),
                    field,
                    &[],
                    mojito_types::param_expr::MetaTy::value(ty),
                ))
            }
            _ => None,
        }
    }

    /// The declared type of the value requirement `field` of the trait
    /// whose scope is being checked, which its own `Self` reads.
    fn trait_self_value_ty(&self, field: &str) -> Option<Ty> {
        match self.trait_self_comptime.last()?.get(field)? {
            CtMemberReq::Value(ty) => Some(ty.clone()),
            CtMemberReq::Type { .. } | CtMemberReq::Generator { .. } => None,
        }
    }

    /// The type `object` names as the subject of a member read: `Self`, a
    /// type parameter (`T`, `Self.T`), a struct application (`A`, `B[4]`),
    /// or a bare parameterized struct's generator (`S`); `None` for an
    /// expression that names a value.
    fn member_subject(&self, object: &Expr) -> Option<Ty> {
        match &object.kind {
            ExprKind::Identifier(name) if name == "Self" => self.self_ty.clone(),
            ExprKind::Identifier(name) if self.lookup(name).is_some() => None,
            ExprKind::Identifier(name) if let Some(parameter) = self.lookup_tparam(name) => {
                Some(parameter)
            }
            ExprKind::Identifier(name) if let Some(generator) = self.unbound_generator(name) => {
                Some(generator)
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                match self.self_param_ct_value(field)? {
                    CtValue::Type(ty) => Some(ty),
                    _ => None,
                }
            }
            _ => self.member_type_operand(object),
        }
    }

    /// A struct's value member at the application's arguments: its binders
    /// replaced, so a closed application folds (`Self.n + 1` at `n = 4`).
    pub(super) fn struct_member_at(
        &self,
        info: &StructInfo,
        arguments: &[TyArg],
        value: &CtValue,
    ) -> Result<ParamExpr, TypeError> {
        let context = &self.param_context;
        let mut bindings = mojito_types::param_expr::ParamBindings::new();
        self.bind_declared(&mut bindings, &info.decls, arguments)?;
        let value = context.constant(value.clone()).map_err(param_error)?;
        context
            .replace(&value, &bindings)
            .and_then(|replaced| context.fold(&replaced))
            .map_err(param_error)
    }

    /// The argument a struct application `arguments` holds at its value
    /// parameter `field`: upstream's parameter read through a type
    /// (`S[7].n`, `ParameterList[vs.values].values`), folded when closed.
    /// `None` when `field` names no value parameter of the struct.
    pub(super) fn struct_parameter_at(
        &self,
        info: &StructInfo,
        arguments: &[TyArg],
        field: &str,
    ) -> Option<Result<ParamExpr, TypeError>> {
        let slot = info.decls.iter().position(|decl| {
            matches!(decl, ParamDecl::Value { .. }) && decl.name().trim_start_matches('*') == field
        })?;
        let TyArg::Val(value) = arguments.get(slot)? else {
            return None;
        };
        let context = &self.param_context;
        Some(
            context
                .constant(value.clone())
                .and_then(|value| context.fold(&value))
                .map_err(param_error),
        )
    }

    /// Bind each of `decls` to its argument in `arguments` by binder id: a
    /// type binder to its type, a value binder or a type pack to its value.
    pub(super) fn bind_declared(
        &self,
        bindings: &mut mojito_types::param_expr::ParamBindings,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        for (decl, argument) in decls.iter().zip(arguments) {
            if let Some(list) = self.forwarded_type_pack(decl, argument) {
                bindings.bind(decl.id().clone(), list);
            }
            match (decl, argument) {
                (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => {
                    bindings.bind_type(id.clone(), ty.clone());
                }
                (ParamDecl::Value { id, .. } | ParamDecl::Type { id, .. }, TyArg::Val(value)) => {
                    bindings.bind(
                        id.clone(),
                        self.param_context
                            .constant(value.clone())
                            .map_err(param_error)?,
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The raw list a construction of `ty` binds when `ty` is a
    /// `TypeList` application: the argument at its `values` parameter.
    fn type_list_construction(&self, ty: &Ty) -> Option<CtValue> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return None;
        };
        if name != super::comptime_validation::TYPE_LIST {
            return None;
        }
        let slot = self
            .structs
            .get(name)?
            .decls
            .iter()
            .position(|decl| decl.name() == "values")?;
        match arguments.get(slot)? {
            TyArg::Val(values) => Some(values.clone()),
            _ => None,
        }
    }

    /// Reject a comparison the pin's types do not define: a type compared
    /// with a value (`T == 1`, `3 == T`), or a type ordered (`T < Int`). The
    /// comparison calls the operator's method on the type's metatype, which
    /// a parameter's bound does not implement and a struct's metatype does
    /// not convert to; only `==` and `!=` are defined between two types.
    pub(super) fn reject_type_parameter_comparison(
        &self,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), TypeError> {
        let method = match op {
            InfixOp::Eq => "__eq__",
            InfixOp::Ne => "__ne__",
            InfixOp::Lt => "__lt__",
            InfixOp::Le => "__le__",
            InfixOp::Gt => "__gt__",
            InfixOp::Ge => "__ge__",
            _ => return Ok(()),
        };
        let builds_value = |operand: &Expr| {
            self.build_ct(operand, Recording::Silent)
                .is_ok_and(|built| *built.meta() != MetaTy::Type)
        };
        let left_value = builds_value(left);
        let (left, right) = (
            self.comptime_type_operand(left)?,
            self.comptime_type_operand(right)?,
        );
        let left = match (left, right) {
            (None, None) => return Ok(()),
            (Some(_), Some(_)) if matches!(op, InfixOp::Eq | InfixOp::Ne) => return Ok(()),
            // An operand the builder cannot read is the operand compiler's
            // diagnostic.
            (None, Some(_)) if !left_value => return Ok(()),
            (None, Some(_)) => {
                return Err(TypeError::MatchPattern(format!(
                    "no matching method in call to '{method}'"
                )));
            }
            (Some(left), _) => left,
        };
        Err(TypeError::MatchPattern(match left.kind() {
            TyKind::Param { bounds, .. } => format!(
                "'{}' does not implement the '{method}' method",
                bounds.first().map_or("AnyType", String::as_str)
            ),
            _ => format!(
                "invalid call to '{method}': value passed to 'self' cannot be converted from \
                 'AnyStruct[{left}]'"
            ),
        }))
    }
}

/// The conjuncts of a proposition: the operands of a canonical `and`, or the
/// proposition itself.
/// Whether [`Checker::eval_associated_ct`]'s builder records what a read
/// denotes at its span.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recording {
    Record,
    Silent,
}

/// A parameter expression in the `CtValue` transport form: a type shape is
/// its type, closed or not.
fn ct_transport(expr: ParamExpr) -> CtValue {
    match expr.kind() {
        mojito_types::param_expr::ParamKind::TypeShape(ty) => CtValue::Type(ty.clone()),
        _ => expr.into_value(),
    }
}

/// Where a module constant's compile-time value is recorded; see
/// [`Checker::module_constant_entry`].
enum ModuleConstant<'a> {
    Applied(&'a ParamExpr),
    Int(&'a mojito_common::literal::IntLiteral),
    Value(&'a CtValue),
    Literal(&'a Expr),
}

/// Whether `value` is an `Int`, `UInt`, `Bool`, or `Float64` value: a
/// width-one vector, whose `_mlir_value` storage is the value itself.
/// The rejection of a comparison of struct values as a generic
/// proposition ([`Checker::struct_comparison`]).
fn struct_comparison_rejection() -> TypeError {
    TypeError::Unsupported("a comparison of struct values is no generic proposition".to_string())
}

/// Whether `value` is a struct-typed compile-time value.
fn struct_valued(value: &ParamExpr) -> bool {
    value
        .meta()
        .as_value()
        .is_some_and(|ty| matches!(ty.kind(), TyKind::Struct(..)))
}

fn is_simd_length_value(value: &ParamExpr) -> bool {
    value
        .meta()
        .as_value()
        .is_some_and(mojito_types::types::is_simd_length)
}

fn is_scalar_value(value: &ParamExpr) -> bool {
    value.meta().as_value().is_some_and(|ty| {
        matches!(
            ty.kind(),
            TyKind::INT | TyKind::UINT | TyKind::Bool | TyKind::FLOAT64
        )
    })
}

fn conjuncts(proposition: &ParamExpr) -> Vec<ParamExpr> {
    match proposition.kind() {
        mojito_types::param_expr::ParamKind::Op {
            op: ParamOp::BoolAnd,
            operands,
        } => operands.clone(),
        _ => vec![proposition.clone()],
    }
}

/// The element types of a pack parameter bound in a constraint environment.
fn bound_pack_types(environment: &ConstraintEnvironment<'_>, param: &ParamRef) -> Option<Vec<Ty>> {
    let TyArg::Val(list) = environment.get(param)? else {
        return None;
    };
    type_list(list)
}

/// The element types of a type list value.
fn type_list(list: &CtValue) -> Option<Vec<Ty>> {
    let CtValue::Tuple(values) = list else {
        return None;
    };
    values
        .iter()
        .map(|value| match value {
            CtValue::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .collect()
}

/// The arguments a constraint is judged under, each under the identity of
/// the binder it binds. A binder erased from its declaration (an origin's
/// mutability) has no identity of its own and is bound under its spelling's
/// unbound reference.
#[derive(Default)]
pub(super) struct ConstraintEnvironment<'a> {
    bound: HashMap<ParamId, &'a TyArg>,
}

impl<'a> ConstraintEnvironment<'a> {
    /// `decls` bound to `arguments`, position by position.
    pub(super) fn declared(decls: &[ParamDecl], arguments: &'a [TyArg]) -> Self {
        let mut environment = Self::default();
        environment.bind_declared(decls, arguments);
        environment
    }

    /// The arguments a call site holds under their parameters' spellings,
    /// each bound to the binder of that spelling in every one of `scopes`.
    pub(super) fn named(
        scopes: &[&[ParamDecl]],
        arguments: impl IntoIterator<Item = (&'a str, &'a TyArg)>,
    ) -> Self {
        let mut environment = Self::default();
        for (name, argument) in arguments {
            environment.bind_named(scopes, name, argument);
        }
        environment
    }

    pub(super) fn bind_declared(&mut self, decls: &[ParamDecl], arguments: &'a [TyArg]) {
        self.bound.extend(
            decls
                .iter()
                .zip(arguments)
                .map(|(decl, argument)| (decl.id().clone(), argument)),
        );
    }

    pub(super) fn bind_named(&mut self, scopes: &[&[ParamDecl]], name: &str, argument: &'a TyArg) {
        let spelling = name.trim_start_matches('*');
        let declared: Vec<&ParamId> = scopes
            .iter()
            .flat_map(|decls| decls.iter())
            .filter(|decl| decl.name().trim_start_matches('*') == spelling)
            .map(ParamDecl::id)
            .collect();
        if declared.is_empty() {
            self.bound.insert(ParamRef::unbound(spelling).id, argument);
        }
        self.bound
            .extend(declared.into_iter().map(|id| (id.clone(), argument)));
    }

    pub(super) fn get(&self, reference: &ParamRef) -> Option<&'a TyArg> {
        self.bound.get(&reference.id).copied()
    }

    pub(super) fn arguments(&self) -> impl Iterator<Item = &'a TyArg> {
        self.bound.values().copied()
    }
}

/// A recognized `TypeList` receiver expression in a constraint position.
pub(super) enum TypeListReceiver {
    /// The pack adapter `TypeList[Ts.values]()`, naming the pack parameter.
    Pack(ParamRef),
    /// The concrete constructor `TypeList.of[...]()`, with resolved elements.
    Concrete(Vec<Ty>),
    /// A list computed from packs still open: a reversal or a concatenation.
    Derived(ParamExpr),
}

/// The type a parameterized associated type's body denotes. The body parses as
/// an ordinary compile-time expression; the type-valued forms are a bare name
/// (`Element`), a type application (`List[T]`, `Fixed[n]`), an explicit type
/// value, or a `Self.` member.
pub(super) fn assoc_body_source_type(value: &Expr) -> Result<SourceType, TypeError> {
    match &value.kind {
        ExprKind::TypeValue(ty) => Ok(ty.clone()),
        // `comptime IteratorType[...] = Self` — a self-iterating struct
        // (current Mojo's range family) names itself as the member's body.
        ExprKind::Identifier(name) if name == "Self" => Ok(SourceType::SelfType),
        ExprKind::Identifier(name) => Ok(SourceType::Named(name.clone(), Vec::new())),
        // A bracketed attribute is a compile-time value, whatever its type.
        ExprKind::TypeApply { name, .. } if name == "__mlir_attr" => Err(unsupported_assoc_body()),
        ExprKind::TypeApply { name, args } => Ok(SourceType::Named(name.clone(), args.clone())),
        // A subscript of the struct's own member or parameter
        // (`Self.values[idx]`, `Self.Get[0]`).
        ExprKind::Index { object, index }
            if let ExprKind::Member {
                object: base,
                field,
            } = &object.kind
                && matches!(&base.kind, ExprKind::Identifier(name) if name == "Self") =>
        {
            Ok(SourceType::IndexedProjection {
                base: Box::new(SourceType::SelfParam(field.clone())),
                index: index.clone(),
            })
        }
        ExprKind::Index { object, index } if matches!(object.kind, ExprKind::Call { .. }) => {
            Ok(SourceType::IndexedProjection {
                base: Box::new(SourceType::Value(object.clone())),
                index: index.clone(),
            })
        }
        // A type application such as `List[T]` parses as a subscript over the
        // type's name; recover the parameter-argument list from the index.
        ExprKind::Index { object, index } if let ExprKind::Identifier(name) = &object.kind => {
            Ok(SourceType::Named(name.clone(), subscript_param_args(index)))
        }
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(s) if s == "Self") => {
            Ok(SourceType::SelfParam(field.clone()))
        }
        // Another type's member or its application (`R.Of[Bool]`,
        // `_MLIR.KGENParamListType[Self.type]`).
        _ => member_source_type(value).ok_or_else(unsupported_assoc_body),
    }
}

/// The type a member read through a type spells as an expression: the
/// member (`M.E`, `S[Int].X`) or its application (`S[Int].Of[Bool]`,
/// `M.Pair[A, B]`, `M.Buf[n]`). `None` for any other expression.
pub(super) fn member_source_type(expr: &Expr) -> Option<SourceType> {
    // A lowercase root (a type parameter `t`) is a type here too.
    let member = |object: &Expr| {
        let ExprKind::Member {
            object: base,
            field,
        } = &object.kind
        else {
            return None;
        };
        let base = base.type_spelling().or_else(|| match &base.kind {
            ExprKind::Identifier(name) => Some(SourceType::Named(name.clone(), Vec::new())),
            _ => None,
        })?;
        Some(match base {
            SourceType::SelfType => SourceType::SelfParam(field.clone()),
            base => SourceType::Assoc {
                base: Box::new(base),
                name: field.clone(),
                args: Vec::new(),
            },
        })
    };
    let applied = |object: &Expr, args: Vec<mojito_ast::ast::ParamArg>| match member(object)? {
        SourceType::Assoc { base, name, .. } => Some(SourceType::Assoc { base, name, args }),
        // A sibling member's application (`Self.of[type=type]`).
        SourceType::SelfParam(name) => Some(SourceType::Assoc {
            base: Box::new(SourceType::SelfType),
            name,
            args,
        }),
        _ => None,
    };
    match &expr.kind {
        ExprKind::Member { .. } => member(expr),
        ExprKind::Index { object, index } if !matches!(index.kind, ExprKind::TypeValue(_)) => {
            Some(SourceType::IndexedProjection {
                base: Box::new(member(object)?),
                index: index.clone(),
            })
        }
        ExprKind::Index { object, index } => applied(object, subscript_param_args(index)),
        ExprKind::MultiIndex { object, args } => applied(object, multi_subscript_param_args(args)?),
        _ => None,
    }
}

/// The pack a `.values` projection names: `Ts.values` (a `def`'s own pack)
/// or `Self.Ts.values` (a struct's pack inside its members).
pub(super) fn pack_values_projection(expression: &Expr) -> Option<&str> {
    let ExprKind::Member { object, field } = &expression.kind else {
        return None;
    };
    if field != "values" {
        return None;
    }
    match &object.kind {
        ExprKind::Identifier(pack) => Some(pack),
        ExprKind::Member {
            object: base,
            field: pack,
        } if matches!(&base.kind, ExprKind::Identifier(name) if name == "Self") => Some(pack),
        _ => None,
    }
}

/// The parameter kind of one associated-type parameter, classified from its raw
/// declaration (origin parameters are erased by `classify_params`, so the arity
/// and template lowering read the raw `TypeParam` directly).
pub(super) enum AssocParamKind {
    Origin,
    Value,
    Type,
}

pub(super) fn assoc_param_kind(param: &mojito_ast::ast::TypeParam) -> AssocParamKind {
    if matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
        AssocParamKind::Origin
    } else if param.value_type.is_some()
        || matches!(param.bounds.as_slice(), [only] if scalar_type_name(only).is_some())
    {
        AssocParamKind::Value
    } else {
        AssocParamKind::Type
    }
}

/// The receiver a parameter list is: its closed elements, the pack it
/// names, or the computed list itself.
fn type_list_receiver(list: ParamExpr) -> TypeListReceiver {
    let closed = match list.as_constant() {
        Some(CtValue::Tuple(values)) => values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    match (closed, list.kind()) {
        (Some(types), _) => TypeListReceiver::Concrete(types),
        (None, mojito_types::param_expr::ParamKind::DeclRef(pack)) => {
            TypeListReceiver::Pack(pack.clone())
        }
        (None, _) => TypeListReceiver::Derived(list),
    }
}

/// The rejection of a per-element query over a list computed from packs
/// still open.
fn derived_list_query(list: &ParamExpr, query: &str) -> TypeError {
    TypeError::Unsupported(format!(
        "TypeList.{query} over the computed list '{list}' whose packs are still parameters"
    ))
}

fn unsupported_assoc_body() -> TypeError {
    TypeError::Unsupported("a parameterized associated type must be defined by a type".to_string())
}

/// Every parameter a predicate-alias body references must be declared, so an
/// application can never leave a dangling operand that would silently evaluate
/// false. Pack projections are rejected here, since an application's
/// substitution binds no pack.
fn validate_predicate_params(
    constraint: &GenericConstraint,
    declared: &HashSet<&ParamId>,
) -> Result<(), TypeError> {
    use GenericConstraint::{
        And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
        PackPredicate, Trivial, WithMessage,
    };
    let check_param = |param: &ParamRef| {
        if declared.contains(&param.id) {
            Ok(())
        } else {
            Err(TypeError::Unsupported(format!(
                "a Bool-bodied comptime alias may reference only its own parameters \
                 ('{param}' is not declared)"
            )))
        }
    };
    let check_operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Param(param) => check_param(param),
        ConstraintOperand::Value(_) | ConstraintOperand::Type(_) => Ok(()),
        ConstraintOperand::PackLength(_) => Err(TypeError::Unsupported(
            "a pack projection in a Bool-bodied comptime alias".to_string(),
        )),
        ConstraintOperand::Expr(_) => Err(TypeError::Unsupported(
            "an arithmetic operand in a Bool-bodied comptime alias".to_string(),
        )),
    };
    match constraint {
        WithMessage(inner, _) | Not(inner) => validate_predicate_params(inner, declared),
        And(left, right) | Or(left, right) => {
            validate_predicate_params(left, declared)?;
            validate_predicate_params(right, declared)
        }
        Conforms { operand, .. } => check_operand(operand),
        ConformsPack { .. } | PackPredicate { .. } | PackContains { .. } => Err(
            TypeError::Unsupported("a pack projection in a Bool-bodied comptime alias".to_string()),
        ),
        Trivial(_, operand) => check_operand(operand),
        Eq(left, right)
        | Ne(left, right)
        | Lt(left, right)
        | Le(left, right)
        | Gt(left, right)
        | Ge(left, right) => {
            check_operand(left)?;
            check_operand(right)
        }
        Bool(_) => Ok(()),
    }
}

/// The pack and trait of `Ts.all_conforms_to[Trait]()` (or `Self.Ts…`), read
/// off syntax.
fn pack_conformance_atom(expr: &Expr) -> Option<(&str, &str)> {
    use mojito_ast::ast::ParamArg;
    let ExprKind::Invoke {
        callee,
        param_args,
        args,
        kwargs,
    } = &expr.kind
    else {
        return None;
    };
    let ExprKind::Member { object, field } = &callee.kind else {
        return None;
    };
    if field != "all_conforms_to" || !args.is_empty() || !kwargs.is_empty() {
        return None;
    }
    let pack = match &object.kind {
        ExprKind::Identifier(pack) => pack.as_str(),
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
            field.as_str()
        }
        _ => return None,
    };
    let trait_name = match param_args.as_slice() {
        [ParamArg::Type(SourceType::Named(name, arguments))] if arguments.is_empty() => name,
        [
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }),
        ] => name,
        _ => return None,
    };
    Some((pack, trait_name))
}

/// `members` in an order that resolves each after the siblings it reads
/// through `Self` (`comptime X = Self.K + 1` above `comptime K = 3`,
/// `comptime empty_of[type: AnyType] = Self.of[type=type]` above `of`), as
/// the pin reads a struct's members in any order: declaration order among
/// independent members, and for the members of a cycle.
fn read_order<'m>(members: &[&'m StructComptime]) -> Vec<&'m StructComptime> {
    let reads: Vec<HashSet<String>> = members.iter().map(|member| self_reads(member)).collect();
    let mut placed = vec![false; members.len()];
    let mut order = Vec::with_capacity(members.len());
    let ready = |index: usize, placed: &[bool]| {
        reads[index].iter().all(|name| {
            members
                .iter()
                .zip(placed)
                .enumerate()
                .all(|(other, (member, done))| other == index || *done || member.name != *name)
        })
    };
    while let Some(next) = (0..members.len())
        .find(|&index| !placed[index] && ready(index, &placed))
        .or_else(|| placed.iter().position(|done| !done))
    {
        placed[next] = true;
        order.push(members[next]);
    }
    order
}

/// The sibling members a struct `comptime` member reads through `Self`:
/// `Self.K` in its body or clauses, `Self.K` or `Self.Of[..]` in a type.
fn self_reads(member: &StructComptime) -> HashSet<String> {
    struct Reads(HashSet<String>);
    impl mojito_ast::visit::Visitor for Reads {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::Member { object, field } = &expr.kind
                && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
            {
                self.0.insert(field.clone());
            }
        }

        fn visit_type(&mut self, ty: &SourceType) {
            match ty {
                SourceType::SelfParam(name) => {
                    self.0.insert(name.trim_start_matches('*').to_string());
                }
                SourceType::Assoc { base, name, .. } if matches!(**base, SourceType::SelfType) => {
                    self.0.insert(name.clone());
                }
                _ => {}
            }
        }
    }
    let mut reads = Reads(HashSet::new());
    mojito_ast::visit::walk_struct_comptime(&mut reads, member);
    reads.0
}

/// Whether `operand` is a compile-time value of a struct type, whose
/// comparison is its operator method's.
fn struct_valued_operand(operand: &ConstraintOperand) -> bool {
    match operand {
        ConstraintOperand::Expr(expression)
        | ConstraintOperand::Value(CtValue::Expr(expression)) => expression
            .meta()
            .as_value()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Struct(..))),
        _ => false,
    }
}

/// The compile-time vector `SIMD[DType.d, w](values...)` builds: one lane per
/// value, or one value splatted across the width.
fn simd_construction(
    dtype: mojito_ast::ast::Dtype,
    width: usize,
    values: &[CtValue],
) -> Result<CtValue, TypeError> {
    let lanes =
        mojito_types::ct::simd_construction_lanes(dtype, width, values).map_err(|error| {
            TypeError::NotComptime(match error {
                SimdConstructionError::Arity => format!(
                    "SIMD[DType.{}, {width}] construction expects one lane or {width} lanes",
                    dtype.name()
                ),
                SimdConstructionError::Lane => format!(
                    "a SIMD[DType.{}, {width}] lane needs a compile-time scalar",
                    dtype.name()
                ),
            })
        })?;
    Ok(CtValue::Simd { dtype, lanes })
}

/// The type a module `comptime` bound to `value` spells, when `value` is
/// written as a type (`Int`, `SIMD[DType.uint64, 4]`, `AHasher[U256(0)]`, a
/// KGEN type): the spelling a declaration symbol reads the alias as.
fn module_alias_spelling(value: &Expr) -> Option<SourceType> {
    if super::reflection::spells_reflection_handle(value) {
        return None;
    }
    match assoc_body_source_type(value).ok()? {
        SourceType::SelfType | SourceType::SelfParam(_) | SourceType::IndexedProjection { .. } => {
            None
        }
        spelling => Some(spelling),
    }
}

/// Whether a generator's body names no parameter but its own.
/// The truth `Bool(value)` gives a closed compile-time scalar or string:
/// a nonzero number, a nonempty string. `None` for any other value.
fn ct_truth(value: &CtValue) -> Option<bool> {
    Some(match value {
        CtValue::Bool(value) => *value,
        CtValue::Int(value) | CtValue::Index(value) => *value != 0,
        CtValue::UInt(value) => *value != 0,
        CtValue::Float(bits) => f64::from_bits(*bits) != 0.0,
        CtValue::IntLiteral(value) => !value.is_zero(),
        CtValue::FloatLiteral(value) => !value.is_zero(),
        CtValue::Str(value) => !value.is_empty(),
        _ => return None,
    })
}

fn generator_is_closed(generator: &mojito_types::ct::GeneratorValue) -> bool {
    let own: Vec<&mojito_types::param_expr::ParamId> =
        generator.params.iter().map(ParamDecl::id).collect();
    let mut closed = true;
    generator.body.visit(&mut |node| {
        closed &= match node.kind() {
            mojito_types::param_expr::ParamKind::DeclRef(reference) => own.contains(&&reference.id),
            mojito_types::param_expr::ParamKind::IndexRef { .. }
            | mojito_types::param_expr::ParamKind::Hole { .. }
            | mojito_types::param_expr::ParamKind::PackQuery { .. } => false,
            _ => node.embedded_types().into_iter().all(|ty| {
                !mojito_types::types::mentions(ty, &|inner| match inner.kind() {
                    TyKind::Param { binder, .. } => !own.contains(&&binder.id),
                    TyKind::Assoc { .. } | TyKind::SelfType | TyKind::Dependent(_) => true,
                    _ => false,
                })
            }),
        };
    });
    closed
}

/// How an application leaves a parameter open: a `_` hole leaves its own
/// slot, a trailing `...` every slot the arguments before it leave.
#[derive(Clone, Copy)]
enum UnboundSlot {
    Hole,
    Rest,
}

/// The unbound slot `arg` spells (`_`, `...`, or a keyword one) and the
/// expression spelling it.
fn unbound_slot(arg: &ParamArg) -> Option<(UnboundSlot, &Expr)> {
    match arg {
        ParamArg::Named { value, .. } => unbound_slot(value),
        ParamArg::Value(value) => match &value.kind {
            ExprKind::Identifier(name) if name == "_" => Some((UnboundSlot::Hole, value)),
            ExprKind::Identifier(name) if name == "..." => Some((UnboundSlot::Rest, value)),
            _ => None,
        },
        ParamArg::Type(_) => None,
    }
}

/// A read of the generator parameter `name` at `spelling`'s location.
fn parameter_read(spelling: &Expr, name: &str) -> Expr {
    let mut read = spelling.clone();
    read.kind = ExprKind::Identifier(name.to_string());
    read
}

/// Every name `value` spells, as a value or as a type.
fn names_spelled(value: &Expr) -> HashSet<String> {
    let mut names = NamesSpelled(HashSet::new());
    mojito_ast::visit::walk_expr(&mut names, value);
    names.0
}

/// The arguments a partial application binds, by their parameters' names.
struct BoundSiblings(HashMap<String, ParamArg>);

impl BoundSiblings {
    /// Spell `param`'s declaration over the bound arguments in place of
    /// the parameters they bind.
    fn substitute(&self, param: &mut mojito_ast::ast::TypeParam) {
        use mojito_ast::visit::{walk_expr_mut, walk_type_mut};
        for ty in [&mut param.value_type, &mut param.callable_bound]
            .into_iter()
            .flatten()
        {
            walk_type_mut(&mut &*self, ty);
        }
        for value in [&mut param.origin_mutability, &mut param.default]
            .into_iter()
            .flatten()
            .chain(&mut param.constraints)
        {
            walk_expr_mut(&mut &*self, value);
        }
    }
}

impl mojito_ast::visit::MutVisitor for &BoundSiblings {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let ExprKind::Identifier(name) = &expr.kind else {
            return;
        };
        match self.0.get(name) {
            Some(ParamArg::Value(value)) => *expr = value.clone(),
            Some(ParamArg::Type(ty)) => expr.kind = ExprKind::TypeValue(ty.clone()),
            _ => {}
        }
    }

    fn visit_type_mut(&mut self, ty: &mut SourceType) {
        let SourceType::Named(name, args) = ty else {
            return;
        };
        if !args.is_empty() {
            return;
        }
        match self.0.get(name.as_str()) {
            Some(ParamArg::Type(bound)) => *ty = bound.clone(),
            Some(ParamArg::Value(value)) => {
                *ty = value
                    .type_spelling()
                    .unwrap_or_else(|| SourceType::Value(Box::new(value.clone())));
            }
            _ => {}
        }
    }
}

struct NamesSpelled(HashSet<String>);

impl mojito_ast::visit::Visitor for NamesSpelled {
    fn visit_expr(&mut self, expr: &Expr) {
        if let ExprKind::Identifier(name) = &expr.kind {
            self.0.insert(name.clone());
        }
    }

    fn visit_type(&mut self, ty: &SourceType) {
        if let SourceType::Named(name, _) = ty {
            self.0.insert(name.clone());
        }
    }
}

/// The field a whitespace-free `#kgen.param.expr<target_get_field, {},
/// "name" : !kgen.string> : T` reads, when `T` is the field's own type
/// ([`mojito_types::ct::KgenTarget::field`]).
fn target_field_read(shape: &str) -> Option<&str> {
    let (field, ty) = shape
        .strip_prefix("#kgen.param.expr<target_get_field,{},\"")?
        .split_once("\":!kgen.string>:")?;
    let expected = if mojito_types::ct::KgenTarget::is_index_field(field) {
        "index"
    } else {
        "!kgen.string"
    };
    (ty == expected && mojito_types::ct::KgenTarget::reads_field(field)).then_some(field)
}
