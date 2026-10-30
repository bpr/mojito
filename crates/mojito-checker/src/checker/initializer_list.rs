//! Initializer lists: `{}`, `{a, b}`, and `{a, kw=b}` at the type their
//! context expects, upstream's initializer list emitted as the construction
//! `T(a, kw=b)` of that type; and the recording every sugar the checker spells as a
//! construction shares.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::KwArg;
use mojito_types::types::TyKind;

impl Checker {
    /// Type an initializer list against the type its context expects: the
    /// construction `expected(entries...)`, spelled under an identity derived
    /// from the brace's and checked as any construction is, so the brace
    /// keeps the type its context sees while the construction keeps its own
    /// facts. The construction is recorded for the checked arena, which hands
    /// HIR the construction in the brace's place.
    pub(super) fn infer_initializer_list(
        &self,
        expression: &Expr,
        (args, kwargs): (Vec<Expr>, Vec<KwArg>),
        expected: &Ty,
        record: bool,
    ) -> Result<Ty, TypeError> {
        let construction =
            self.initializer_list_construction(expression, args, kwargs, expected)?;
        let ty = self.infer_with_expected(&construction, expected, record)?;
        self.record_spelled_construction(expression, construction, &ty);
        Ok(ty)
    }

    /// Bind the local `comptime name = value` as a type alias of `ty`: the
    /// name denotes the type wherever a type is read, and a call of it
    /// constructs `value` ([`Self::infer_type_alias_read`]).
    pub(super) fn bind_local_type_alias(
        &mut self,
        name: &str,
        ty: Ty,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let empty = || TypeError::InvariantViolation("checker scope stack is empty".to_string());
        self.local_type_aliases
            .last_mut()
            .ok_or_else(empty)?
            .insert(name.to_string(), ty);
        self.local_type_alias_values
            .last_mut()
            .ok_or_else(empty)?
            .insert(name.to_string(), value.clone());
        Ok(())
    }

    /// Type an expression headed by a local or module type alias as the same
    /// expression over the alias's parameter expression, as upstream's name
    /// lookup reads the alias as its value. A call constructs the type: the
    /// alias's own application applied to the call's arguments (`S(2)` over
    /// `comptime S = Scalar[DType.float64]` is `Scalar[DType.float64](2)`),
    /// or, for an alias spelled otherwise (`Self.First`), the construction
    /// of the type it names. A static call, a member read, or an applied
    /// member through the alias is the type's (`Q.make()` over `comptime Q =
    /// P` is `P.make()`), as is one through a struct's `comptime` type
    /// member (`T[4].other.answer()`, `T[4].other(10)`). The checked arena
    /// hands HIR the spelling in the expression's place. `None` when no
    /// type alias heads `expression`.
    pub(super) fn infer_type_alias_read(&self, expression: &Expr) -> Option<Result<Ty, TypeError>> {
        match &expression.kind {
            // `Self(...)`: the struct's own alias, so the construction of
            // the struct over its own parameters (`G[Self.T](...)`).
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "Self" && param_args.is_empty() && self.lookup(name).is_none() => {
                let (owner, param_args) = self.self_instance_spelling()?;
                let construction = derived_construction(
                    expression,
                    ExprKind::Call {
                        name: owner,
                        param_args,
                        args: args.clone(),
                        kwargs: kwargs.clone(),
                    },
                );
                Some(self.infer_spelled(expression, construction))
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if param_args.is_empty() => self.type_alias_read(name).map(|(ty, value)| {
                self.alias_construction(expression, ty, value, args, kwargs)
                    .and_then(|construction| self.infer_spelled(expression, construction))
            }),
            // A call of a struct's type member constructs the type it names
            // (`T[4].other(10)`).
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if let Some((ty, value)) = self.type_member_callee(object, method) => Some(
                self.alias_construction(expression, &ty, &value, args, kwargs)
                    .and_then(|construction| self.infer_spelled(expression, construction)),
            ),
            _ => {
                let spelled = over_receiver(expression, |object| match &object.kind {
                    ExprKind::Identifier(name) => {
                        Some(derived_receiver(expression, self.type_alias_read(name)?.1))
                    }
                    ExprKind::Member { .. } => self
                        .type_member_spelling(object)
                        .map(|spelling| derived_receiver(expression, &spelling)),
                    _ => None,
                })?;
                Some(self.infer_spelled(expression, spelled))
            }
        }
    }

    /// Type a call of `type_of(operand)` as the construction of the type the
    /// call denotes, as upstream reads `type_of`'s result as that type
    /// (`type_of(v)(2)` over a `Float32` `v` is `SIMD[DType.float32,
    /// 1](2)`), and a receiver through it over a type parameter as that
    /// parameter's binder. The checked arena hands HIR the spelling in the
    /// expression's place. Any other receiver is the type itself
    /// ([`Self::type_of_receiver`]). `None` when no `type_of` heads
    /// `expression`.
    pub(super) fn infer_type_of_read(&self, expression: &Expr) -> Option<Result<Ty, TypeError>> {
        if let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expression.kind
            && param_args.is_empty()
            && is_type_of(callee)
        {
            return Some(
                self.denoted_type(callee, "a construction through 'type_of'")
                    .and_then(|ty| self.alias_construction(expression, &ty, callee, args, kwargs))
                    .and_then(|construction| self.infer_spelled(expression, construction)),
            );
        }
        let head = match &expression.kind {
            ExprKind::MethodCall { object, .. } | ExprKind::Member { object, .. } => object,
            ExprKind::Invoke { callee, .. } => match &callee.kind {
                ExprKind::Member { object, .. } => object,
                _ => return None,
            },
            _ => return None,
        };
        if !is_type_of(head) {
            return None;
        }
        let ty = match self.denoted_type(head, "the receiver 'type_of'") {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let receiver = derived_receiver(expression, &self.binder_spelling(&ty, head.span)?);
        Some(
            over_receiver(expression, |object| {
                std::ptr::eq(object, head.as_ref()).then(|| receiver.clone())
            })
            .ok_or_else(|| {
                TypeError::InvariantViolation("a 'type_of' receiver lost its read".to_string())
            })
            .and_then(|spelled| self.infer_spelled(expression, spelled)),
        )
    }

    /// The type `object` denotes when it is `type_of(operand)`: a receiver
    /// bound to the type value itself, on which upstream looks a member up.
    /// `None` for any other expression.
    pub(super) fn type_of_receiver(&self, object: &Expr) -> Option<Result<Ty, TypeError>> {
        is_type_of(object).then(|| self.denoted_type(object, "the receiver 'type_of'"))
    }

    /// The construction a call of a struct's `comptime` type member spells
    /// (`T.four()` over `comptime four = T[n=4]` is `T[n=4]()`), as
    /// [`Self::infer_type_alias_read`] types it. `None` for any other
    /// expression.
    pub(super) fn type_member_construction(&self, expression: &Expr) -> Option<Expr> {
        let ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } = &expression.kind
        else {
            return None;
        };
        let (ty, value) = self.type_member_callee(object, method)?;
        self.alias_construction(expression, &ty, &value, args, kwargs)
            .ok()
    }

    /// The struct the enclosing declaration's `Self` names, and the bracket
    /// arguments applying it to its own parameters (`Grid[Self.n]`), as
    /// upstream binds `Self` to the struct over its own parameters: an
    /// infer-only one by keyword (`CompilationTarget[_mlir_value=
    /// Self._mlir_value]`), since no position binds it. `None` outside a
    /// struct.
    pub(super) fn self_instance_spelling(
        &self,
    ) -> Option<(String, Vec<mojito_ast::ast::ParamArg>)> {
        let (instance, _) = mojito_types::types::nominal_parts(self.self_ty.as_ref()?)?;
        let owner = mojito_symbol::symbol::specialization_template(instance).unwrap_or(instance);
        let info = self.structs.get(owner)?;
        let param_args = info
            .source_params
            .iter()
            .map(|parameter| {
                let read =
                    mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(parameter.name.clone()));
                if parameter.infer_only {
                    mojito_ast::ast::ParamArg::Named {
                        name: parameter.name.clone(),
                        value: Box::new(read),
                    }
                } else {
                    read
                }
            })
            .collect();
        Some((owner.to_string(), param_args))
    }

    /// Record that the sugar `expression` checked as `construction` of type
    /// `ty`: the sugar answers with the construction's type, and the checked
    /// arena hands HIR the construction in the sugar's place.
    pub(super) fn record_spelled_construction(
        &self,
        expression: &Expr,
        construction: Expr,
        ty: &Ty,
    ) {
        self.record_at(expression.source_span()).ty = Some(ty.clone());
        self.record_at(expression.source_span()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SpelledConstruction {
                construction: mojito_checked::checked::SpelledConstruction(Box::new(construction)),
            },
        ));
    }

    /// The construction the checker spelled for the sugar `expression`, when
    /// it spelled one: the expression is that construction for places and
    /// borrows as for lowering.
    pub(super) fn spelled_construction(&self, expression: &Expr) -> Option<Expr> {
        match self
            .node_facts
            .borrow()
            .get(&expression.source_span())
            .and_then(|facts| facts.adjustment.as_deref())
        {
            Some(mojito_checked::checked::SemanticAdjustment::SpelledConstruction {
                construction,
            }) => Some(construction.0.as_ref().clone()),
            _ => None,
        }
    }

    /// The type the type alias `name` names, and the parameter expression
    /// it was bound to: a local alias, or else a zero-parameter module
    /// alias of a type. `None` when `name` names neither, or a binding or a
    /// type parameter shadows it.
    fn type_alias_read(&self, name: &str) -> Option<(&Ty, &Expr)> {
        if self.lookup(name).is_some() {
            return None;
        }
        let Some(scope) = self
            .local_type_aliases
            .iter()
            .rposition(|scope| scope.contains_key(name))
        else {
            return self
                .comptime_aliases
                .get(name)
                .filter(|alias| alias.decls.is_empty())
                .and_then(|alias| match &alias.body {
                    AliasBody::Type(ty) => Some((ty, &alias.value)),
                    AliasBody::Predicate(_) | AliasBody::Value(_) => None,
                });
        };
        Some((
            self.local_type_aliases[scope].get(name)?,
            self.local_type_alias_values[scope].get(name)?,
        ))
    }

    /// The spelling of the type `object` reads when it names a struct's
    /// closed `comptime` type member ([`Self::type_member_read`]): inside
    /// the struct, the member's own spelling over `Self`; through a spelled
    /// struct, the type the member resolves to there. `None` when `object`
    /// names no such member or its type has no source spelling.
    fn type_member_spelling(&self, object: &Expr) -> Option<Expr> {
        let (spelling, member) = self.type_member_read(object)?;
        if matches!(spelling, SourceType::SelfParam(_)) {
            return member?.expression_spelling(object.span);
        }
        let ty = self.ty_from_anno(&spelling).ok()?;
        match mojito_types::ct::source_type(&ty, object.span) {
            Some(spelled) => self
                .keyword_spelling(spelled)
                .expression_spelling(object.span),
            // An argument applying a function (`T[n=f()]`) has no source
            // spelling of its value: a member that reads nothing of `Self`
            // is its own spelling at any receiver.
            None => member?
                .expression_spelling(object.span)
                .filter(|spelled| !reads_self(spelled)),
        }
    }

    /// `spelling` with each bracket argument of an infer-only struct
    /// parameter (`CompilationTarget[_mlir_value=…]`) passed by keyword,
    /// since no position binds it.
    fn keyword_spelling(&self, spelling: SourceType) -> SourceType {
        let SourceType::Named(name, arguments) = spelling else {
            return spelling;
        };
        let Some(info) = self.structs.get(&name) else {
            return SourceType::Named(name, arguments);
        };
        let arguments = arguments
            .into_iter()
            .zip(info.decls.iter().map(Some).chain(std::iter::repeat(None)))
            .map(|(argument, decl)| match decl {
                Some(decl) if mojito_ast::call::CompileTimeParam::infer_only(decl) => {
                    mojito_ast::ast::ParamArg::Named {
                        name: decl.name().to_string(),
                        value: Box::new(argument),
                    }
                }
                _ => argument,
            })
            .collect();
        SourceType::Named(name, arguments)
    }

    /// The type a call of `object.member` constructs when `member` is a
    /// closed `comptime` type member of the struct `object` spells, beside
    /// that type's spelling ([`Self::type_member_spelling`]).
    fn type_member_callee(&self, object: &Expr, member: &str) -> Option<(Ty, Expr)> {
        let read = Expr::new(
            ExprKind::Member {
                object: Box::new(object.clone()),
                field: member.to_string(),
            },
            object.span,
        );
        let (spelling, _) = self.type_member_read(&read)?;
        Some((
            self.ty_from_anno(&spelling).ok()?,
            self.type_member_spelling(&read)?,
        ))
    }

    /// The type spelling of `object` when it names a struct's closed
    /// `comptime` type member (`T[4].other`, `Self.current`), beside the
    /// member's own spelling over `Self` when it has one, as upstream's name
    /// lookup reads the member as its value. `None` when `object` names no
    /// such member.
    fn type_member_read(&self, object: &Expr) -> Option<(SourceType, Option<&SourceType>)> {
        let ExprKind::Member { field, .. } = &object.kind else {
            return None;
        };
        let spelling = object.type_spelling()?;
        let owner = match &spelling {
            SourceType::SelfParam(_) => self.self_ty.clone()?,
            SourceType::Assoc { base, .. } => {
                let mut root = base.as_ref();
                while let SourceType::Assoc { base, .. } = root {
                    root = base;
                }
                if matches!(root, SourceType::Named(name, _) if self.lookup(name).is_some()) {
                    return None;
                }
                self.ty_from_anno(base).ok()?
            }
            _ => return None,
        };
        let (instance, _) = mojito_types::types::nominal_parts(&owner)?;
        let info = self
            .structs
            .get(mojito_symbol::symbol::specialization_template(instance).unwrap_or(instance))?;
        if info.methods.contains_key(field)
            || !matches!(info.associated.get(field), Some(CtValue::Type(_)))
        {
            return None;
        }
        Some((spelling, info.associated_spellings.get(field)))
    }

    /// The construction a call of a type alias of `ty`, bound to `value`,
    /// spells with the call's arguments ([`Self::infer_type_alias_read`]).
    fn alias_construction(
        &self,
        expression: &Expr,
        ty: &Ty,
        value: &Expr,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Expr, TypeError> {
        // The alias's application is read again under identities derived
        // from the call's, as a receiver through the alias is.
        let mut applied = value.clone();
        applied.source.clone_from(&expression.source);
        mojito_ast::visit::walk_expr_mut(
            &mut mojito_ast::visit::DerivedIdentities {
                parent: expression.syntax_id,
                next: 2,
            },
            &mut applied,
        );
        if let Some((name, param_args)) = self.raw_list_spelling(&applied, ty) {
            return Ok(derived_construction(
                expression,
                ExprKind::Call {
                    name,
                    param_args,
                    args: args.to_vec(),
                    kwargs: kwargs.to_vec(),
                },
            ));
        }
        let Some((callee, param_args)) = alias_application(value) else {
            return self.initializer_list_construction(
                expression,
                args.to_vec(),
                kwargs.to_vec(),
                ty,
            );
        };
        Ok(derived_construction(
            expression,
            ExprKind::Call {
                name: callee,
                param_args,
                args: args.to_vec(),
                kwargs: kwargs.to_vec(),
            },
        ))
    }

    /// Type a construction through a struct's parameterized type member
    /// (`S[Int].Of[Bool](3, 4)`, `ParameterList.of[4, 5]()`) as the
    /// construction of the type the member names, as upstream reads the
    /// member as its value: the type's spelling applied to the call's
    /// arguments. A struct over a raw parameter list, which has no source
    /// spelling, spells each parameter by keyword, a type as itself and a
    /// value read off the member's application (`ParameterList[type=Int,
    /// values=ParameterList.of[4, 5].values]`). `None` when the callee
    /// names no parameterized type member.
    pub(super) fn infer_type_member_construction(
        &self,
        expression: &Expr,
        callee: &Expr,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let ExprKind::Member { object, field } = &callee.kind else {
            return None;
        };
        let base = object.type_spelling()?;
        let SourceType::Named(owner, _) = &base else {
            return None;
        };
        let owner = self.structs.get(owner)?;
        if !owner.parameterized_associated.contains_key(field) || owner.methods.contains_key(field)
        {
            return None;
        }
        let member = SourceType::Assoc {
            base: Box::new(base),
            name: field.clone(),
            args: param_args.to_vec(),
        };
        let construction = self.ty_from_anno(&member).and_then(|ty| {
            let (name, param_args) = member_application(callee, param_args)
                .and_then(|applied| self.raw_list_spelling(&applied, &ty))
                .or_else(
                    || match mojito_types::ct::source_type(&ty, expression.span) {
                        Some(SourceType::Named(name, param_args)) => Some((name, param_args)),
                        _ => None,
                    },
                )
                .ok_or_else(|| {
                    TypeError::Unsupported(format!(
                        "a construction through '{field}': '{ty}' has no construction to spell"
                    ))
                })?;
            Ok(derived_construction(
                expression,
                ExprKind::Call {
                    name,
                    param_args,
                    args: args.to_vec(),
                    kwargs: kwargs.to_vec(),
                },
            ))
        });
        Some(construction.and_then(|construction| self.infer_spelled(expression, construction)))
    }

    /// Type `Self.<alias>(args)`, a construction through a type alias the
    /// enclosing struct declares (`Self._Mask(mlir_value=v)`), as the
    /// construction of the type the alias spells. `None` when `alias` is no
    /// such member.
    pub(super) fn infer_self_alias_construction(
        &self,
        expression: &Expr,
        alias: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let (owner, _) = mojito_types::types::nominal_parts(self.self_ty.as_ref()?)?;
        let info = self.structs.get(owner)?;
        if info.methods.contains_key(alias) {
            return None;
        }
        let SourceType::Named(name, param_args) = info.associated_spellings.get(alias)? else {
            return None;
        };
        let construction = derived_construction(
            expression,
            ExprKind::Call {
                name: name.clone(),
                param_args: param_args.clone(),
                args: args.to_vec(),
                kwargs: kwargs.to_vec(),
            },
        );
        Some(self.infer_spelled(expression, construction))
    }

    /// The keyword spelling of `ty`, a struct over a raw parameter list
    /// that the type member application `applied` names
    /// (`ParameterList.of[4, 5]`): a type parameter as its type, a value
    /// parameter read off the application. `None` for any other type.
    fn raw_list_spelling(
        &self,
        applied: &Expr,
        ty: &Ty,
    ) -> Option<(String, Vec<mojito_ast::ast::ParamArg>)> {
        use mojito_ast::ast::ParamArg;
        let TyKind::Struct(target, arguments) = ty.kind() else {
            return None;
        };
        let decls = &self.structs.get(target)?.decls;
        let raw = decls.iter().any(|decl| {
            matches!(decl, ParamDecl::Value { ty, variadic: false, .. }
                if matches!(ty.kind(), TyKind::ParamList(_)))
        });
        if !raw || decls.len() != arguments.len() {
            return None;
        }
        let spelled = decls
            .iter()
            .zip(arguments)
            .map(|(decl, argument)| {
                let value = match (decl, argument) {
                    (_, TyArg::Origin(_))
                    | (
                        ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. },
                        _,
                    ) => return None,
                    (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                        ParamArg::Type(mojito_types::ct::source_type(ty, applied.span)?)
                    }
                    _ => ParamArg::Value(Expr::new(
                        ExprKind::Member {
                            object: Box::new(applied.clone()),
                            field: decl.name().to_string(),
                        },
                        applied.span,
                    )),
                };
                Some(ParamArg::Named {
                    name: decl.name().to_string(),
                    value: Box::new(value),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some((target.clone(), spelled))
    }

    /// Type `spelled`, the expression the checker spelled for `expression`,
    /// and record it in `expression`'s place.
    pub(super) fn infer_spelled(&self, expression: &Expr, spelled: Expr) -> Result<Ty, TypeError> {
        let ty = self.infer(&spelled)?;
        self.record_spelled_construction(expression, spelled, &ty);
        Ok(ty)
    }

    /// The construction `expected(entries...)` an initializer list stands
    /// for: a type parameter's own construction, a pack element's
    /// (`Self.Ts[i]()` / `Ts[i]()` over the element's pack and index), the
    /// closed type's spelling applied to the entries, or else the
    /// construction of the type itself ([`Self::instance_construction`]).
    /// The entries keep their identities; the construction and the spelled
    /// arguments take ones derived from the brace.
    fn initializer_list_construction(
        &self,
        expression: &Expr,
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
        expected: &Ty,
    ) -> Result<Expr, TypeError> {
        let unspellable = || {
            TypeError::Unsupported(format!(
                "cannot emit initializer list for '{expected}': the type has no construction \
                 to spell"
            ))
        };
        let kind = match expected.kind() {
            TyKind::Param { binder, .. } => ExprKind::Call {
                name: binder.name.to_string(),
                param_args: Vec::new(),
                args,
                kwargs,
            },
            TyKind::Dependent(dependent)
                if let Some((list, index)) = dependent.pack_element()
                    && let Some(reference) = list.as_decl_ref() =>
            {
                let pack = reference.name.trim_start_matches('*').to_string();
                let index = pack_index_expression(index, expression.span)
                    .or_else(|| self.comptime_alias_spelling(index, expression.span))
                    .ok_or_else(unspellable)?;
                let own = self.self_decls.iter().any(|declaration| {
                    matches!(declaration, ParamDecl::Type { name, variadic: true, .. }
                        if name.trim_start_matches('*') == pack)
                });
                let callee = if own {
                    ExprKind::Member {
                        object: Box::new(Expr::new(
                            ExprKind::Identifier("Self".to_string()),
                            expression.span,
                        )),
                        field: pack,
                    }
                } else {
                    ExprKind::Identifier(pack)
                };
                ExprKind::Invoke {
                    callee: Box::new(Expr::new(callee, expression.span)),
                    param_args: vec![mojito_ast::ast::ParamArg::Value(index)],
                    args,
                    kwargs,
                }
            }
            // An origin argument has no source spelling of its own: the
            // construction binds it against the instance.
            TyKind::Struct(_, arguments)
                if arguments
                    .iter()
                    .any(|argument| matches!(argument, TyArg::Origin(_))) =>
            {
                return self
                    .instance_construction(expression, expected, args, kwargs)
                    .ok_or_else(unspellable);
            }
            _ => {
                let name = match mojito_types::ct::source_type(expected, expression.span) {
                    Some(SourceType::Named(name, param_args)) => {
                        return Ok(derived_construction(
                            expression,
                            ExprKind::Call {
                                name,
                                param_args,
                                args,
                                kwargs,
                            },
                        ));
                    }
                    Some(SourceType::Int) => "Int",
                    Some(SourceType::UInt) => "UInt",
                    Some(SourceType::Bool) => "Bool",
                    Some(SourceType::Float64) => "Float64",
                    _ => {
                        return self
                            .instance_construction(expression, expected, args, kwargs)
                            .ok_or_else(unspellable);
                    }
                };
                ExprKind::Call {
                    name: name.to_string(),
                    param_args: Vec::new(),
                    args,
                    kwargs,
                }
            }
        };
        Ok(derived_construction(expression, kind))
    }

    /// A read of the binder of `ty`, a type parameter in scope: `Self.T`
    /// for the struct's own, `T` (or the pack `Ts`) for the declaration's.
    /// `None` for any other type.
    fn binder_spelling(&self, ty: &Ty, at: mojito_common::token::Span) -> Option<Expr> {
        let TyKind::Param { binder, .. } = ty.kind() else {
            return None;
        };
        let name = binder.name.to_string();
        let binds = |declared: &ParamId| *declared == binder.id;
        if self
            .self_decls
            .iter()
            .any(|declaration| matches!(declaration, ParamDecl::Type { id, .. } if binds(id)))
        {
            return SourceType::SelfParam(name).expression_spelling(at);
        }
        self.tparams
            .iter()
            .rev()
            .find_map(|scope| {
                scope
                    .get(&name)
                    .or_else(|| scope.get(name.trim_start_matches('*')))
            })
            .filter(
                |scoped| matches!(scoped.kind(), TyKind::Param { binder, .. } if binds(&binder.id)),
            )
            .map(|_| Expr::new(ExprKind::Identifier(name), at))
    }

    /// The construction of `instance`, a type no source spelling names
    /// (`It[T]`, `Scalar[dt]`, a `TypeList` over a raw list), from `args`:
    /// the struct's call, bound to the instance it constructs, as upstream
    /// constructs the contextual type itself. `None` for a type that is no
    /// struct.
    fn instance_construction(
        &self,
        expression: &Expr,
        instance: &Ty,
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
    ) -> Option<Expr> {
        let name = match instance.kind() {
            TyKind::Struct(name, _) => name.clone(),
            TyKind::Simd { .. } => mojito_types::types::SIMD_TYPE_NAME.to_string(),
            TyKind::Dependent(_) => instance.to_string(),
            _ => return None,
        };
        let construction = derived_construction(
            expression,
            ExprKind::Call {
                name,
                param_args: Vec::new(),
                args,
                kwargs,
            },
        );
        self.spelled_instances
            .borrow_mut()
            .insert(construction.source_span(), instance.clone());
        Some(construction)
    }

    /// The name of the innermost local `comptime` binding that denotes
    /// `index` (the variable a `comptime for` over an iterator aliases to
    /// its element), spelled as a read of it.
    fn comptime_alias_spelling(
        &self,
        index: &mojito_types::param_expr::ParamExpr,
        span: mojito_common::token::Span,
    ) -> Option<Expr> {
        self.local_comptime_parameters
            .iter()
            .rev()
            .find_map(|scope| {
                scope
                    .iter()
                    .find_map(|(name, (_, expression))| (expression == index).then_some(name))
            })
            .map(|name| Expr::new(ExprKind::Identifier(name.clone()), span))
    }
}

/// A node the checker synthesizes for the sugar `expression`'s construction:
/// at `span`, in the sugar's source, under the identity `ordinal` derived
/// from the sugar's. Ordinal 0 is the construction itself.
pub(super) fn spelled_node(
    expression: &Expr,
    ordinal: u32,
    kind: ExprKind,
    span: mojito_common::token::Span,
) -> Expr {
    Expr {
        kind,
        span,
        source: expression.source.clone(),
        syntax_id: mojito_common::token::SyntaxId::derived(expression.syntax_id, ordinal),
    }
}

/// `kind` as the construction spelled for the sugar `expression` (an
/// initializer list, a scalar `range` call): its location, with every
/// synthesized node (the call and its spelled parameter arguments, never the
/// entries) numbered from the sugar's identity.
pub(super) fn derived_construction(expression: &Expr, kind: ExprKind) -> Expr {
    let mut identities = mojito_ast::visit::DerivedIdentities {
        parent: expression.syntax_id,
        next: 1,
    };
    let mut construction = spelled_node(expression, 0, kind, expression.span);
    let synthesized: Vec<&mut Expr> = match &mut construction.kind {
        ExprKind::Call { param_args, .. } | ExprKind::Invoke { param_args, .. } => param_args
            .iter_mut()
            .filter_map(|argument| match argument {
                mojito_ast::ast::ParamArg::Value(value) => Some(value),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    for value in synthesized {
        value.source.clone_from(&expression.source);
        mojito_ast::visit::walk_expr_mut(&mut identities, value);
    }
    construction
}

/// `expression` (a static call, a member read, or an applied member) over
/// the receiver `receiver` gives for its own: the same read, spelled under
/// `expression`'s identity. `None` when `expression` has no receiver or
/// `receiver` replaces none.
fn over_receiver(expression: &Expr, receiver: impl Fn(&Expr) -> Option<Box<Expr>>) -> Option<Expr> {
    let spelled = match &expression.kind {
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => ExprKind::MethodCall {
            object: receiver(object)?,
            method: method.clone(),
            args: args.clone(),
            kwargs: kwargs.clone(),
        },
        ExprKind::Member { object, field } => ExprKind::Member {
            object: receiver(object)?,
            field: field.clone(),
        },
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            let ExprKind::Member { object, field } = &callee.kind else {
                return None;
            };
            ExprKind::Invoke {
                callee: Box::new(spelled_node(
                    expression,
                    1,
                    ExprKind::Member {
                        object: receiver(object)?,
                        field: field.clone(),
                    },
                    callee.span,
                )),
                param_args: param_args.clone(),
                args: args.clone(),
                kwargs: kwargs.clone(),
            }
        }
        _ => return None,
    };
    Some(spelled_node(expression, 0, spelled, expression.span))
}

/// The type spelling `value` read as `expression`'s receiver, in its source
/// under identities derived from its own: ordinal 0 is the spelled
/// expression, 1 an applied member's callee.
fn derived_receiver(expression: &Expr, value: &Expr) -> Box<Expr> {
    let mut receiver = value.clone();
    receiver.source.clone_from(&expression.source);
    mojito_ast::visit::walk_expr_mut(
        &mut mojito_ast::visit::DerivedIdentities {
            parent: expression.syntax_id,
            next: 2,
        },
        &mut receiver,
    );
    Box::new(receiver)
}

/// The callee and bracket arguments of a type alias's parameter
/// expression that applies a name (`Scalar[DType.float64]`, `Sized[n]`) or
/// is one (`Float64`). `None` for any other spelling.
fn alias_application(value: &Expr) -> Option<(String, Vec<mojito_ast::ast::ParamArg>)> {
    match &value.kind {
        ExprKind::Identifier(name) => Some((name.clone(), Vec::new())),
        _ => value
            .bracket_application()
            .map(|(name, args)| (name.to_string(), args.into_owned())),
    }
}

/// The member application an invocation's callee and bracket list spell
/// (`ParameterList.of[4, 5]` of `ParameterList.of[4, 5]()`), as a
/// compile-time expression. `None` when a bracket argument is a keyword
/// whose value is itself named.
fn member_application(callee: &Expr, param_args: &[mojito_ast::ast::ParamArg]) -> Option<Expr> {
    use mojito_ast::ast::{ParamArg, SubscriptArg};
    let value = |argument: &ParamArg| match argument {
        ParamArg::Value(value) => Some(value.clone()),
        ParamArg::Type(ty) => Some(Expr::new(ExprKind::TypeValue(ty.clone()), callee.span)),
        ParamArg::Named { .. } => None,
    };
    let args = param_args
        .iter()
        .map(|argument| match argument {
            ParamArg::Named { name, value: named } => Some(SubscriptArg::Keyword {
                name: name.clone(),
                value: value(named)?,
            }),
            positional => value(positional).map(SubscriptArg::Index),
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Expr::new(
        ExprKind::MultiIndex {
            object: Box::new(callee.clone()),
            args,
        },
        callee.span,
    ))
}

/// The source spelling of a pack element's index that is still a parameter
/// expression: a binder in scope (`i`) or a closed integer.
fn pack_index_expression(
    index: &mojito_types::param_expr::ParamExpr,
    span: mojito_common::token::Span,
) -> Option<Expr> {
    if let Some(binder) = index.as_decl_ref() {
        return Some(Expr::new(
            ExprKind::Identifier(binder.name.to_string()),
            span,
        ));
    }
    index.as_constant()?.materialize(span)
}

/// Whether `expression` is a call of `type_of`.
fn is_type_of(expression: &Expr) -> bool {
    matches!(&expression.kind, ExprKind::Call { name, .. } if name == "type_of")
}

/// Whether `expr` spells `Self` or a read of a `Self` parameter.
fn reads_self(expr: &Expr) -> bool {
    struct ReadsSelf(bool);
    impl mojito_ast::visit::Visitor for ReadsSelf {
        fn visit_expr(&mut self, expr: &Expr) {
            if matches!(&expr.kind, ExprKind::Identifier(name) if name == "Self") {
                self.0 = true;
            }
        }

        fn visit_type(&mut self, ty: &SourceType) {
            if matches!(ty, SourceType::SelfParam(_) | SourceType::SelfType)
                || matches!(ty, SourceType::Named(name, _) if name == "Self")
            {
                self.0 = true;
            }
        }
    }
    let mut reads = ReadsSelf(false);
    mojito_ast::visit::walk_expr(&mut reads, expr);
    reads.0
}
