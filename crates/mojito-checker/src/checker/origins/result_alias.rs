//! The call-result aliasing rule: a call assigned straight back over its
//! destination must not borrow an owned interior of that destination through
//! one of its direct arguments.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Reject `D = f(args)` when a direct argument of the outermost call — the
    /// receiver included — borrows an owned interior of `D` (`s = takes(s.rstrip())`,
    /// whose view carries `s.<bytes>`), as current Mojo does. An argument
    /// borrowing `D` itself (`StringSpan(s)`), a moved argument, a read of a
    /// loan-free register-passable value, and the arguments of nested calls
    /// do not alias.
    pub(in crate::checker) fn check_result_aliases_destination(
        &self,
        destination: &mojito_types::origin::OriginPlace,
        value: &Expr,
    ) -> Result<(), TypeError> {
        self.check_result_aliases(destination, value, true)
    }

    /// Reject `C[i] = f(args)` through a nominal `__setitem__` when a direct
    /// argument's value carries an owned interior of the container `C`
    /// (`xs[0] = keep(xs[1].rstrip())`), as current Mojo does. Unlike a whole
    /// assignment, an argument that merely reads an element place
    /// (`xs[0] = xs[1].copy()`) does not alias.
    pub(in crate::checker) fn check_result_carries_container_interior(
        &self,
        container: &mojito_types::origin::OriginPlace,
        value: &Expr,
    ) -> Result<(), TypeError> {
        self.check_result_aliases(container, value, false)
    }

    /// Reject a `mut self` call whose read argument carries a borrow of the
    /// receiver's storage (`s += s.rstrip()`, whose view carries `s.<bytes>`),
    /// as current Mojo does. Place arguments are judged by the within-call
    /// exclusivity check instead.
    pub(in crate::checker) fn check_mutable_receiver_carried_aliases(
        &self,
        receiver: &Expr,
        method: &str,
        parameter_names: &[String],
        slots: &[mojito_ast::call::ArgSlot],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        use mojito_ast::call::ArgSlot;
        let Ok(receiver) = self.origin_place(receiver) else {
            return Ok(());
        };
        slots
            .iter()
            .enumerate()
            .find_map(|(index, slot)| {
                let argument = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return None,
                };
                if matches!(argument.kind, ExprKind::Transfer(_)) {
                    return None;
                }
                if !self
                    .carried_argument_origins(argument)
                    .iter()
                    .any(|origin| origin_overlaps_place(origin, &receiver))
                {
                    return None;
                }
                // An argument that is itself a call borrowing the receiver's
                // interior through its own argument (`xs[0] = keep(xs[0].rstrip())`)
                // is reported at that inner call, as upstream reports it.
                Some(
                    self.check_result_aliases(&receiver, argument, false)
                        .err()
                        .unwrap_or_else(|| TypeError::AliasingMutableReceiverArgument {
                            parameter: parameter_names
                                .get(index)
                                .cloned()
                                .unwrap_or_else(|| format!("arg{index}")),
                            callee: method.to_string(),
                        }),
                )
            })
            .map_or(Ok(()), Err)
    }

    /// The origins an argument's value carries. A view returned by a method
    /// on an element place (`xs[1].rstrip()`) borrows the owned interior below
    /// that element.
    pub(in crate::checker) fn carried_argument_origins(
        &self,
        expression: &Expr,
    ) -> Vec<mojito_types::origin::Origin> {
        let origins = self.aggregate_origins(expression);
        if origins.is_empty()
            && let ExprKind::MethodCall { object, .. } = &expression.kind
            && matches!(object.kind, ExprKind::Index { .. })
            && matches!(
                self.operation_adjustments
                    .borrow()
                    .get(&expression.source_span()),
                Some(mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. })
            )
            && let Ok(element) = self.origin_place(object)
        {
            return self.project_view_result_interior(
                expression,
                vec![mojito_types::origin::Origin::Place(element)],
            );
        }
        origins
    }

    fn check_result_aliases(
        &self,
        destination: &mojito_types::origin::OriginPlace,
        value: &Expr,
        place_arguments_alias: bool,
    ) -> Result<(), TypeError> {
        let mut call = value;
        while let ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } = &call.kind {
            call = inner;
        }
        let Some((callee, initializer, arguments)) = self.result_call_arguments(call) else {
            return Ok(());
        };
        arguments
            .into_iter()
            .find(|argument| {
                self.argument_aliases_interior(destination, argument, place_arguments_alias)
            })
            .map_or(Ok(()), |argument| {
                Err(TypeError::AliasingResultArgument {
                    parameter: argument.parameter,
                    callee,
                    initializer,
                })
            })
    }

    /// The callee spelling, whether it is an initializer, and the direct
    /// arguments in source order, for a call the rule judges.
    fn result_call_arguments<'a>(
        &self,
        call: &'a Expr,
    ) -> Option<(String, bool, Vec<AliasCandidate<'a>>)> {
        let span = call.source_span();
        match &call.kind {
            // The stringify intrinsic: upstream's `String(*args)` initializer.
            ExprKind::Call { name, args, .. }
                if !args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                Some((
                    "String".to_string(),
                    true,
                    args.iter()
                        .map(|expression| AliasCandidate {
                            parameter: "args".to_string(),
                            expression,
                            convention: None,
                            parameter_ty: None,
                        })
                        .collect(),
                ))
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => Some((
                name.rsplit('$').next().unwrap_or(name).to_string(),
                self.structs.contains_key(name),
                self.contract_arguments(&span, args, kwargs)?,
            )),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let (receiver_convention, receiver_elided) = self
                    .selected_calls
                    .borrow()
                    .get(&span)
                    .map(|contract| (contract.receiver_convention, contract.receiver_elided))?;
                let mut arguments = Vec::new();
                if !receiver_elided {
                    arguments.push(AliasCandidate {
                        parameter: "self".to_string(),
                        expression: object,
                        convention: receiver_convention,
                        parameter_ty: None,
                    });
                }
                arguments.extend(self.contract_arguments(&span, args, kwargs)?);
                Some((method.clone(), false, arguments))
            }
            _ => None,
        }
    }

    /// The selected contract's source arguments at `span`, named by the
    /// callee's recorded parameter names (`arg<i>` when none were recorded).
    fn contract_arguments<'a>(
        &self,
        span: &SourceSpan,
        args: &'a [Expr],
        kwargs: &'a [mojito_ast::ast::KwArg],
    ) -> Option<Vec<AliasCandidate<'a>>> {
        use mojito_checked::checked::CheckedCallArgumentSource;
        let parameters = self.call_parameters.borrow();
        let parameters = parameters.get(span);
        let selected = self.selected_calls.borrow();
        let Some(contract) = selected.get(span) else {
            // A free call records no contract: positional arguments bind the
            // leading parameters and keywords bind by name.
            let parameters = parameters?;
            return Some(
                args.iter()
                    .zip(parameters)
                    .map(|(expression, parameter)| (parameter, expression))
                    .chain(kwargs.iter().filter_map(|keyword| {
                        parameters
                            .iter()
                            .find(|parameter| parameter.name == keyword.name)
                            .map(|parameter| (parameter, &keyword.value))
                    }))
                    .map(|(parameter, expression)| AliasCandidate {
                        parameter: parameter.name.clone(),
                        expression,
                        convention: parameter.convention,
                        parameter_ty: Some(parameter.ty.clone()),
                    })
                    .collect(),
            );
        };
        let mut arguments: Vec<_> = contract
            .arguments
            .iter()
            .enumerate()
            .filter_map(|(index, argument)| {
                let (order, expression) = match argument.source {
                    CheckedCallArgumentSource::Positional(position) => {
                        (position, args.get(position)?)
                    }
                    CheckedCallArgumentSource::Keyword(position) => {
                        (args.len() + position, &kwargs.get(position)?.value)
                    }
                    CheckedCallArgumentSource::Default => return None,
                };
                let parameter = parameters
                    .and_then(|parameters| parameters.get(index))
                    .map_or_else(|| format!("arg{index}"), |parameter| parameter.name.clone());
                Some((
                    order,
                    AliasCandidate {
                        parameter,
                        expression,
                        convention: argument.convention,
                        parameter_ty: Some(argument.parameter_ty.clone()),
                    },
                ))
            })
            .collect();
        arguments.sort_by_key(|(order, _)| *order);
        Some(
            arguments
                .into_iter()
                .map(|(_, argument)| argument)
                .collect(),
        )
    }

    /// Whether the argument borrows an owned interior of `destination`: through
    /// the origins its value carries, or (when `place_arguments_alias`) as a
    /// place under that interior.
    fn argument_aliases_interior(
        &self,
        destination: &mojito_types::origin::OriginPlace,
        argument: &AliasCandidate<'_>,
        place_arguments_alias: bool,
    ) -> bool {
        match argument.convention {
            Some(ArgConvention::Var | ArgConvention::Deinit | ArgConvention::Out) => return false,
            Some(ArgConvention::Mut | ArgConvention::Ref) => {}
            Some(ArgConvention::Imm) | None => {
                let ty = argument.parameter_ty.clone().or_else(|| {
                    self.expression_types
                        .borrow()
                        .get(&argument.expression.source_span())
                        .cloned()
                });
                if ty.is_some_and(|ty| self.is_loan_free_register_passable(&ty)) {
                    return false;
                }
            }
        }
        let mut origins = self.carried_argument_origins(argument.expression);
        if place_arguments_alias
            && crate::checker::places::is_place_expr(argument.expression)
            && let Ok(reference) = self.reference_actual(argument.expression)
        {
            origins.push(reference.origin);
        }
        origins
            .iter()
            .any(|origin| origin_in_owned_interior(origin, destination))
    }

    /// A read of this type copies a value that borrows nothing.
    fn is_loan_free_register_passable(&self, ty: &Ty) -> bool {
        self.is_trivial_register_passable(ty) && !self.type_carries_loans(ty)
    }
}

/// One direct argument of an assigned call, with the parameter it binds.
struct AliasCandidate<'a> {
    parameter: String,
    expression: &'a Expr,
    convention: Option<ArgConvention>,
    parameter_ty: Option<Ty>,
}

/// Whether `origin` lies under `destination` and crosses an owned-interior
/// segment below it.
fn origin_in_owned_interior(
    origin: &mojito_types::origin::Origin,
    destination: &mojito_types::origin::OriginPlace,
) -> bool {
    use mojito_types::origin::{Origin, OriginSeg};
    match origin {
        Origin::Place(place) => {
            place.root == destination.root
                && place.path.starts_with(&destination.path)
                && place.path[destination.path.len()..]
                    .iter()
                    .any(|segment| matches!(segment, OriginSeg::Interior(_)))
        }
        Origin::Union(members) => members
            .iter()
            .any(|member| origin_in_owned_interior(member, destination)),
        _ => false,
    }
}

/// Whether `origin` names storage overlapping `place` (either is a prefix of
/// the other).
fn origin_overlaps_place(
    origin: &mojito_types::origin::Origin,
    place: &mojito_types::origin::OriginPlace,
) -> bool {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Place(borrowed) => {
            let (shorter, longer) = if borrowed.path.len() <= place.path.len() {
                (&borrowed.path, &place.path)
            } else {
                (&place.path, &borrowed.path)
            };
            borrowed.root == place.root && longer.starts_with(shorter)
        }
        Origin::Union(members) => members
            .iter()
            .any(|member| origin_overlaps_place(member, place)),
        _ => false,
    }
}
