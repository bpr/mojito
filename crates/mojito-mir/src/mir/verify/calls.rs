//! Block, direct-call, function, and runtime-pack ABI verification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};

pub(super) fn verify_block(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    block: &MirBlock,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let mut asserted = None;
    for (index, instruction) in block.instrs.iter().enumerate() {
        verify_instruction(
            name,
            function,
            declarations,
            instruction,
            asserted.as_ref().unwrap_or(context),
            errors,
        );
        if asserted.is_none()
            && !context.asserted
            && matches!(instruction, MirInstr::ComptimeAssert { .. })
        {
            asserted = Some(RegionContext {
                asserted: true,
                ..context.clone()
            });
        }
        verify_in_place_binding(name, &block.instrs[..index], instruction, errors);
        if let Some(raises) = instruction.raise_edge() {
            verify_error_region(name, block, index, raises, errors);
        }
    }
    verify_terminator(name, function, &block.term, context, errors);
}

pub(super) fn verify_call_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::Call {
            func: FuncRef(callee),
            args,
            kwargs,
            capture_accesses,
            param_arg_regs,
            instantiated_args,
            receiver,
            spread,
            ..
        } => {
            verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
            if let Some(position) = spread {
                verify_pack_spread(
                    prefix,
                    cx.function,
                    *position,
                    args,
                    declared(cx.declarations, callee),
                    errors,
                );
            }
            if let Some(declaration) = declared(cx.declarations, callee) {
                verify_collector_passed(prefix, declaration, *spread, kwargs, errors);
                let parameters = crate::mir::call_operand_parameters(
                    callee,
                    declaration,
                    args.len(),
                    *spread,
                    kwargs,
                );
                verify_direct_call(cx, declaration, args, kwargs, parameters.as_deref(), errors);
                let owner = receiver
                    .as_ref()
                    .and_then(|receiver| owner_arguments(cx.declarations, receiver));
                verify_param_arguments(
                    prefix,
                    cx.function,
                    &declaration.param_decls,
                    param_arg_regs,
                    instantiated_args,
                    owner
                        .as_ref()
                        .map(|(decls, arguments)| (decls.as_ref(), arguments.as_ref())),
                    errors,
                );
            }
        }
        MirInstr::MethodCall {
            dest,
            method,
            recv,
            receiver_convention,
            resolved: Some(callee),
            reference_result,
            result_adapter,
            args,
            kwargs,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread,
            recv_writes,
            refined_bound,
            ..
        } => {
            verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
            // A `def(...)` bound's `__call__` requirement call binds the
            // bound's signature on a read receiver, as a declared trait's
            // requirement call binds its requirement, and a receiver passed
            // by address is an immutable reference, as upstream's
            // `lit.call` takes a `!lit.ref` of a `read self`; a
            // register-passable one is passed by value.
            let callable_bound =
                super::super::bound_call_contract(instruction, &cx.function.reg_types).is_some();
            if callable_bound
                && let Some(contract) = cx
                    .reg_ty(*recv)
                    .map(super::super::receiver_value_ty)
                    .and_then(mojito_types::types::callable_contract_ty)
            {
                if cx
                    .reg_ty(*recv)
                    .and_then(crate::mir::addressed_receiver)
                    .is_some_and(|reference| {
                        reference.mutability != mojito_types::origin::Mutability::Immutable
                    })
                {
                    errors.push(format!(
                        "{prefix}: a `def(...)` bound's `__call__` receiver is a mutable reference"
                    ));
                }
                verify_bound_call(
                    cx,
                    contract,
                    instruction,
                    callee,
                    *recv_writes || receiver_convention.is_some(),
                    errors,
                );
            }
            if let Some(position) = spread {
                verify_pack_spread(
                    prefix,
                    cx.function,
                    *position,
                    args,
                    declared(cx.declarations, callee),
                    errors,
                );
            }
            let abstract_value_next = mojito_symbol::symbol::is_trait_dispatch_symbol(callee)
                && method == "__next__"
                && reference_result.is_none();
            if let Some(reference) = reference_result {
                let expected = Ty::new(TyKind::Ref((**reference).clone()));
                if cx.function.reg_types.get(&dest.0) != Some(&expected) {
                    errors.push(format!(
                        "{prefix}: method-call reference ABI {expected} does not match its destination type"
                    ));
                }
            }
            // The adapter copies the element out of the reference a concrete
            // `__next__` returns, as it does for the abstract dispatch.
            let adapted_reference_next = method == "__next__"
                && reference_result.is_none()
                && declared(cx.declarations, callee)
                    .is_some_and(|declaration| declaration.returns_reference);
            match result_adapter.as_deref().map(|adapter| adapter.kind) {
                Some(mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference) => {
                    if !abstract_value_next && !adapted_reference_next {
                        errors.push(format!(
                            "{prefix}: copy-reference result adapter is not attached to a value-yielding __next__ call"
                        ));
                    }
                }
                None if abstract_value_next => errors.push(format!(
                    "{prefix}: abstract value-returning __next__ call lacks its copy-reference adapter"
                )),
                None => {}
            }
            if let Some(declaration) = declared(cx.declarations, callee) {
                verify_collector_passed(prefix, declaration, *spread, kwargs, errors);
                // The call takes ownership of its receiver, by `var` or
                // `deinit`, exactly as the callee's receiver does, as
                // upstream's call reads the convention off its callee's type.
                if declaration.has_receiver
                    && owned_receiver(*receiver_convention)
                        != owned_receiver(declaration.receiver_convention)
                {
                    errors.push(format!(
                        "{prefix}: method-call receiver convention does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                let parameters =
                    crate::mir::method_operand_parameters(declaration, args.len(), *spread, kwargs);
                verify_direct_call(cx, declaration, args, kwargs, parameters.as_deref(), errors);
                verify_by_address_receiver(cx, declaration, *recv, errors);
                let result_abi_matches =
                    match reference_result {
                        Some(reference) => {
                            declaration.returns_reference
                                && types_compatible(&reference.referent, &declaration.ret_ty)
                        }
                        None => {
                            declaration.returns_reference == result_adapter.is_some()
                                && cx.function.reg_types.get(&dest.0).is_some_and(|result| {
                                    types_compatible(result, &declaration.ret_ty)
                                })
                        }
                    };
                if !result_abi_matches {
                    errors.push(format!(
                        "{prefix}: method-call result ABI does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                if declaration.param_decls != **param_decls {
                    errors.push(format!(
                        "{prefix}: method-call compile-time parameter metadata does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                let owner = cx
                    .reg_ty(*recv)
                    .and_then(|receiver| owner_arguments(cx.declarations, receiver));
                verify_param_arguments(
                    prefix,
                    cx.function,
                    &declaration.param_decls,
                    param_arg_regs,
                    instantiated_args,
                    owner
                        .as_ref()
                        .map(|(decls, arguments)| (decls.as_ref(), arguments.as_ref())),
                    errors,
                );
            } else if !callable_bound
                && spread.is_none()
                && let Some(requirement) = cx.declarations.call_requirement(
                    callee,
                    cx.reg_ty(*recv),
                    refined_bound.as_ref().map(|bound| bound.traits.as_slice()),
                )
            {
                let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
                let operands =
                    crate::mir::requirement_operand_parameters(requirement, args.len(), &keywords)
                        .map(|parameters| {
                            args.iter()
                                .map(|argument| (*argument, None))
                                .chain(
                                    kwargs
                                        .iter()
                                        .map(|(name, argument)| (*argument, Some(name.as_str()))),
                                )
                                .zip(parameters)
                                .map(
                                    |((register, keyword), parameter)| crate::mir::BoundOperand {
                                        register,
                                        keyword,
                                        parameter,
                                    },
                                )
                                .collect::<Vec<_>>()
                        });
                // A call that writes its receiver but records no convention
                // for it leaves the receiver to its place, as lowering does.
                let receiver = (!(*recv_writes && receiver_convention.is_none()))
                    .then_some((*recv, *receiver_convention));
                verify_requirement_call(cx, requirement, receiver, operands.as_deref(), errors);
            }
        }
        // A spread names the collector of the method the checker selected;
        // a dispatch-resolved call has none to check it against.
        MirInstr::MethodCall {
            resolved: None,
            spread: Some(position),
            ..
        } => errors.push(format!(
            "{prefix}: unresolved method call spreads argument {position}"
        )),
        MirInstr::CallIndirect { .. } => verify_indirect_call(cx, instruction, errors),
        _ => {}
    }
}

/// The receiver's parameter declarations beside its arguments (a struct's
/// own, a `SIMD`'s as `SIMD`'s), for a call whose callee's declarations
/// name them (`Self.dtype`); a reference receiver answers for its referent.
fn owner_arguments<'a>(
    declarations: &'a MirDeclarations,
    receiver: &'a Ty,
) -> Option<OwnerArguments<'a>> {
    let receiver = match receiver.kind() {
        TyKind::Ref(reference) => &reference.referent,
        _ => receiver,
    };
    let (name, arguments) = mojito_types::types::nominal_parts(receiver)?;
    let owner = declarations.structure(name)?;
    let arguments = std::borrow::Cow::Owned(owner.view_arguments(&arguments).into_owned());
    Some((owner.argument_view(), arguments))
}

/// The operand a call passes its callee's positional collector as: the
/// pack the call builds for it (`total(1, 2)`, a `tuple.make` of the
/// overflow), or the caller's collector spread whole (`show(*args)`,
/// `Sink().take(*args)`). The position names an argument whose register
/// holds a homogeneous collector's `VariadicList`, a `VariadicPack` over a
/// pack still a parameter, or the tuple of a bound pack's elements, and a
/// declared callee collects a positional pack. `print` is undeclared and
/// collects.
fn verify_pack_spread(
    prefix: &str,
    function: &MirFunction,
    position: usize,
    args: &[Reg],
    declaration: Option<&MirFunctionDeclaration>,
    errors: &mut Vec<String>,
) {
    let Some(argument) = args.get(position) else {
        errors.push(format!(
            "{prefix}: call spreads argument {position} but passes {} arguments",
            args.len()
        ));
        return;
    };
    match function.reg_types.get(&argument.0).map(Ty::kind) {
        Some(TyKind::VariadicPack(_) | TyKind::Tuple(_) | TyKind::RuntimePack(_)) => {}
        Some(found)
            if mojito_types::types::variadic_list_element(found).is_some()
                || mojito_types::types::variadic_pack_owned(found).is_some() => {}
        Some(found) => errors.push(format!(
            "{prefix}: spread argument {position} has type {found}, not a pack collector"
        )),
        None => errors.push(format!("{prefix}: spread argument {position} has no type")),
    }
    if let Some(declaration) = declaration
        && declaration.variadic.is_none()
    {
        errors.push(format!(
            "{prefix}: a pack is spread into '{}', which collects no positional pack",
            declaration.lowered_name
        ));
    }
}

/// A call of a declared callee that collects a positional pack passes it
/// as one operand ([`verify_pack_spread`]), and one that collects keywords
/// passes its keyword collector as its `**` operand, as upstream's call
/// passes the pack and the dictionary it builds; no backend collects the
/// overflow itself.
fn verify_collector_passed(
    prefix: &str,
    declaration: &MirFunctionDeclaration,
    spread: Option<usize>,
    kwargs: &[(String, Reg)],
    errors: &mut Vec<String>,
) {
    if declaration.variadic.is_some() && spread.is_none() {
        errors.push(format!(
            "{prefix}: a call of '{}' passes no positional collector",
            declaration.lowered_name
        ));
    }
    if declaration.kw_variadic.is_some()
        && !kwargs
            .iter()
            .any(|(name, _)| name == mojito_ast::ast::FORWARDED_KWARGS_NAME)
    {
        errors.push(format!(
            "{prefix}: a call of '{}' passes no keyword collector",
            declaration.lowered_name
        ));
    }
}

/// Argument-type and by-address checks against a declaration, as
/// upstream's `lit.call` verifier checks every operand against its
/// callee's signature.
///
/// `parameters` maps each operand, positional then keyword, to the regular
/// parameter it binds by `declaration` (a direct `call`, a resolved
/// `call.method`, or a resolved indirect `__call__` target), the spread
/// pack binding the positional collector; `None` when the operands do not
/// bind by it, which is an error. Every operand that binds a regular
/// parameter has that parameter's type, a borrowed one (any convention but
/// `var`, `deinit`, or `out`) compared by its referent. Every operand of a
/// parameter `by_address_parameter` takes by address, a `mut`/`ref` one or
/// a `read` one of a type not proven register-passable, must be a
/// reference, as upstream's `lit.call` takes a `!lit.ref` wherever the
/// signature does, however the operand was spelled at the call site. An
/// elaborated instance's declaration names its substituted types, so a
/// `read` its instance proves register-passable takes the value.
pub(super) fn verify_direct_call(
    cx: &InstrCx<'_>,
    declaration: &MirFunctionDeclaration,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    parameters: Option<&[Option<usize>]>,
    errors: &mut Vec<String>,
) {
    let Some(parameters) = parameters else {
        errors.push(format!(
            "{}: the operands of '{}' do not bind its declaration",
            cx.prefix, declaration.lowered_name
        ));
        return;
    };
    let operands: Vec<_> = args
        .iter()
        .map(|argument| (*argument, None))
        .chain(
            kwargs
                .iter()
                .map(|(name, argument)| (*argument, Some(name.as_str()))),
        )
        .zip(parameters)
        .map(
            |((register, keyword), parameter)| crate::mir::BoundOperand {
                register,
                keyword,
                parameter: *parameter,
            },
        )
        .collect();
    verify_by_address_operands(cx, declaration, &operands, errors);
    let param_types = owner_instance_param_types(cx, declaration, &operands);
    verify_operand_types(
        cx,
        &declaration.lowered_name,
        param_types.as_deref().unwrap_or(&declaration.param_types),
        &declaration.param_conventions,
        &operands,
        errors,
    );
}

/// Require every operand that binds a regular parameter to have that
/// parameter's type in `param_types`, a borrowed one (any convention in
/// `conventions` but `var`, `deinit`, or `out`, at a non-reference type)
/// compared by its referent, since a parameter received by address takes
/// a reference to the caller's storage (`Flatten::address_operand`).
///
/// `callee` names the callee in the message, `operands` lists each
/// operand, positional then keyword.
pub(super) fn verify_operand_types(
    cx: &InstrCx<'_>,
    callee: &str,
    param_types: &[Ty],
    conventions: &[Option<mojito_ast::ast::ArgConvention>],
    operands: &[crate::mir::BoundOperand<'_>],
    errors: &mut Vec<String>,
) {
    for (index, operand) in operands.iter().enumerate() {
        let Some((parameter, expected)) = operand
            .parameter
            .and_then(|parameter| Some((parameter, param_types.get(parameter)?)))
        else {
            continue;
        };
        let borrowed = !matches!(
            conventions.get(parameter).copied().flatten(),
            Some(
                mojito_ast::ast::ArgConvention::Var
                    | mojito_ast::ast::ArgConvention::Deinit
                    | mojito_ast::ast::ArgConvention::Out
            )
        );
        let found = cx.reg_ty(operand.register).map(|found| {
            crate::mir::addressed_operand(found, expected)
                .filter(|_| borrowed)
                .map_or(found, |reference| &reference.referent)
        });
        if let Some(found) = found
            && !types_compatible(found, expected)
        {
            let operand = operand.keyword.map_or_else(
                || format!("argument {index}"),
                |name| format!("keyword argument '{name}'"),
            );
            errors.push(format!(
                "{}: {operand} of '{callee}' has type {found}, declared {expected}",
                cx.prefix
            ));
        }
    }
}

/// Require a reference at every operand of a call of `declaration` whose
/// parameter `by_address_parameter` takes by address (`verify_direct_call`).
///
/// `operands` lists each operand, positional then keyword. Each such
/// operand must be `ref<P>` against its parameter `P`
/// ([`crate::mir::addressed_operand`]), so a parameter declared as a
/// reference type takes a reference to a reference.
pub(super) fn verify_by_address_operands(
    cx: &InstrCx<'_>,
    declaration: &MirFunctionDeclaration,
    operands: &[crate::mir::BoundOperand<'_>],
    errors: &mut Vec<String>,
) {
    for (index, operand) in operands.iter().enumerate() {
        if let Some(parameter) = operand.parameter
            && crate::mir::by_address_parameter(declaration, cx.declarations, parameter).is_some()
            && let Some(expected) = declaration.param_types.get(parameter)
            && let Some(found) = cx.reg_ty(operand.register)
            && crate::mir::addressed_operand(found, expected).is_none()
        {
            let operand = operand.keyword.map_or_else(
                || format!("argument {index}"),
                |name| format!("keyword argument '{name}'"),
            );
            errors.push(format!(
                "{}: {operand} of '{}' is received by address but passes a {found} value",
                cx.prefix, declaration.lowered_name
            ));
        }
    }
}

/// Require a reference wherever a call through a trait bound binds its
/// receiver or a regular operand by address, as upstream's `lit.call` of
/// `@Trait::method` is verified against the requirement's signature.
///
/// `requirement` is the one the call binds
/// ([`MirDeclarations::call_requirement`]). `receiver` is the receiver
/// beside the convention the call records for it (a requirement records
/// none), `None` when the call leaves it to its place. `operands` lists
/// each regular operand, positional then keyword, with the parameter it
/// binds, `None` when they do not bind the requirement's shape.
pub(super) fn verify_requirement_call(
    cx: &InstrCx<'_>,
    requirement: &crate::mir::TraitRequirement,
    receiver: Option<(Reg, Option<mojito_ast::ast::ArgConvention>)>,
    operands: Option<&[crate::mir::BoundOperand<'_>]>,
    errors: &mut Vec<String>,
) {
    let dispatch = &requirement.dispatch;
    if let Some((receiver, convention)) = receiver
        && let Some(found) = cx.reg_ty(receiver)
        && crate::mir::addressed_receiver(found).is_none()
        && crate::mir::requirement_by_address_receiver(convention, found, cx.declarations).is_some()
    {
        errors.push(format!(
            "{}: receiver of '{dispatch}' is received by address but passes a {found} value",
            cx.prefix
        ));
    }
    let Some(operands) = operands else {
        errors.push(format!(
            "{}: the operands of '{dispatch}' do not bind its requirement",
            cx.prefix
        ));
        return;
    };
    for (index, operand) in operands.iter().enumerate() {
        if let Some(parameter) = operand.parameter
            && let Some(expected) = requirement.param_types.get(parameter)
            && let Some(found) = cx.reg_ty(operand.register)
            && crate::mir::addressed_operand(found, expected).is_none()
            && crate::mir::requirement_by_address_parameter(
                requirement,
                parameter,
                found,
                cx.declarations,
            )
            .is_some()
        {
            let operand = operand.keyword.map_or_else(
                || format!("argument {index}"),
                |name| format!("keyword argument '{name}'"),
            );
            errors.push(format!(
                "{}: {operand} of '{dispatch}' is received by address but passes a {found} value",
                cx.prefix
            ));
        }
    }
}

/// Require a reference at the receiver of a resolved method call, or of a
/// checked subscript accessor, whose declaration takes it by address
/// (`mir::by_address_receiver`: a `mut`/`ref` receiver, or a `read` one of
/// a type not proven register-passable), as upstream's `lit.call` takes a
/// `!lit.ref` wherever the callee's signature does, the receiver included.
pub(super) fn verify_by_address_receiver(
    cx: &InstrCx<'_>,
    declaration: &MirFunctionDeclaration,
    receiver: Reg,
    errors: &mut Vec<String>,
) {
    if !declaration.has_receiver {
        return;
    }
    if let Some(found) = cx.reg_ty(receiver)
        && crate::mir::addressed_receiver(found).is_none()
        && crate::mir::by_address_receiver(declaration, found, cx.declarations).is_some()
    {
        errors.push(format!(
            "{}: receiver of '{}' is received by address but passes a {found} value",
            cx.prefix, declaration.lowered_name
        ));
    }
}

pub(super) fn subscript_arg_regs(argument: &crate::mir::MirSubscriptArg, out: &mut Vec<Reg>) {
    match argument {
        crate::mir::MirSubscriptArg::Index(register) => out.push(*register),
    }
}

pub(super) fn subscript_argument_ty(
    function: &MirFunction,
    argument: &crate::mir::MirSubscriptArg,
) -> Option<Ty> {
    match argument {
        crate::mir::MirSubscriptArg::Index(register) => {
            function.reg_types.get(&register.0).cloned()
        }
    }
}

pub(super) fn effective_call_convention_matches(
    declared: Option<mojito_ast::ast::ArgConvention>,
    effective: Option<mojito_ast::ast::ArgConvention>,
) -> bool {
    declared == effective
        || matches!(
            (declared, effective),
            (
                Some(mojito_ast::ast::ArgConvention::Ref),
                Some(mojito_ast::ast::ArgConvention::Imm)
            )
        )
}

/// The whole declared list of the generic callable `ty`, which a call
/// through it carries.
pub(super) fn generic_callable_decls(ty: &Ty) -> Option<&[mojito_types::types::ParamDecl]> {
    match ty.kind() {
        TyKind::GenericFunc(generic) => Some(&generic.decls),
        TyKind::Param {
            callable_bound: Some(bound),
            ..
        } => generic_callable_decls(bound),
        _ => None,
    }
}

/// Ensure each symbolic dependent index is owned by an explicit enclosing
/// value-parameter binder, and that every parameter expression a type carries
/// may cross into MIR at all. This permits generic MIR to remain symbolic
/// while rejecting a misspelled or escaped index name, an out-of-range or
/// mistyped signature slot, and an unknown or unbound parameter before
/// wildcard compatibility could hide it. The binders in `enclosing`, which
/// the declaration owning the body declares, bind too.
pub(super) fn validate_dependent_bindings_in(
    ty: &Ty,
    enclosing: &HashSet<String>,
) -> Result<(), String> {
    /// `frames` holds the value-binder types of each enclosing generic
    /// signature, innermost last; a type parameter's slot holds `None`.
    fn check_expr(
        expr: &mojito_types::param_expr::ParamExpr,
        frames: &[Vec<Option<Ty>>],
    ) -> Result<(), String> {
        use mojito_types::param_expr::{MetaTy, ParamKind};
        // A tabulation's element names its index as the slot of a binder of
        // its own.
        if let ParamKind::ListTabulate { count, element } = expr.kind() {
            check_expr(count, frames)?;
            let mut inner = frames.to_vec();
            inner.push(vec![Some(Ty::int())]);
            return check_expr(element, &inner);
        }
        let finding = match expr.kind() {
            ParamKind::Hole { .. } => {
                Some("an unknown or unbound parameter cannot cross into MIR".to_string())
            }
            ParamKind::IndexRef { depth, index } => {
                let slot = frames
                    .len()
                    .checked_sub(1 + *depth as usize)
                    .and_then(|frame| frames[frame].get(*index as usize));
                match slot {
                    None => Some(format!(
                        "signature slot {depth}.{index} names no enclosing binder"
                    )),
                    Some(None) => Some(format!(
                        "signature slot {depth}.{index} names a type parameter"
                    )),
                    Some(Some(declared)) if MetaTy::value(declared.clone()) != *expr.meta() => {
                        Some(format!(
                            "signature slot {depth}.{index} is declared `{declared}`, not `{}`",
                            expr.meta()
                        ))
                    }
                    Some(Some(_)) => None,
                }
            }
            _ => None,
        };
        match finding {
            Some(finding) => Err(finding),
            None => expr
                .operands()
                .into_iter()
                .try_for_each(|operand| check_expr(operand, frames)),
        }
    }

    fn walk(
        ty: &Ty,
        bound: &HashSet<String>,
        frames: &mut Vec<Vec<Option<Ty>>>,
    ) -> Result<(), String> {
        match ty.kind() {
            TyKind::Dependent(dependent) => {
                let mut referenced = HashSet::new();
                dependent.expr().referenced_parameters(&mut referenced);
                let mut unbound: Vec<_> = referenced.difference(bound).cloned().collect();
                unbound.sort();
                if !unbound.is_empty() {
                    return Err(format!(
                        "dependent index references unbound parameter(s): {}",
                        unbound.join(", ")
                    ));
                }
                check_expr(dependent.expr(), frames)?;
                for element in dependent
                    .selection()
                    .map_or(&[][..], |(elements, _)| elements)
                {
                    walk(element, bound, frames)?;
                }
            }
            TyKind::GenericFunc(generic) => {
                let GenericFuncTy {
                    decls,
                    func:
                        FuncTy {
                            params,
                            ret,
                            variadic,
                            kw_variadic,
                            error,
                            ..
                        },
                } = &**generic;

                let mut signature_scope = bound.clone();
                frames.push(
                    decls
                        .iter()
                        .map(|declaration| match declaration {
                            ParamDecl::Value { ty, .. } => Some(ty.clone()),
                            ParamDecl::Type { .. } | ParamDecl::Origin { .. } => None,
                        })
                        .collect(),
                );
                let checked = (|| -> Result<(), String> {
                    for declaration in decls {
                        match declaration {
                            ParamDecl::Type {
                                callable_bound,
                                default,
                                ..
                            } => {
                                if let Some(callable) = callable_bound {
                                    walk(callable, &signature_scope, frames)?;
                                }
                                if let Some(default) = default {
                                    walk(default, &signature_scope, frames)?;
                                }
                            }
                            ParamDecl::Value { name, ty, .. } => {
                                walk(ty, &signature_scope, frames)?;
                                signature_scope.insert(name.trim_start_matches('*').to_string());
                            }
                            ParamDecl::Origin { .. } => {}
                        }
                    }
                    for parameter in params {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    walk(ret, &signature_scope, frames)?;
                    if let Some(parameter) = variadic {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    if let Some(parameter) = kw_variadic {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    if let Some(error) = error {
                        walk(error, &signature_scope, frames)?;
                    }
                    Ok(())
                })();
                frames.pop();
                checked?;
            }
            TyKind::Func(func) => {
                let FuncTy {
                    params,
                    ret,
                    variadic,
                    kw_variadic,
                    error,
                    ..
                } = &**func;

                for parameter in params {
                    walk(parameter, bound, frames)?;
                }
                walk(ret, bound, frames)?;
                if let Some(parameter) = variadic {
                    walk(parameter, bound, frames)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, bound, frames)?;
                }
                if let Some(error) = error {
                    walk(error, bound, frames)?;
                }
            }
            TyKind::Param {
                callable_bound: Some(callable),
                ..
            } => walk(callable, bound, frames)?,
            struct_ty if let Some((_, arguments)) = struct_ty.as_non_simd_struct() => {
                for argument in arguments {
                    match argument {
                        TyArg::Ty(ty) => walk(ty, bound, frames)?,
                        TyArg::Val(mojito_types::ct::CtValue::Expr(expr)) => {
                            check_expr(expr, frames)?;
                        }
                        TyArg::Val(_) | TyArg::Origin(_) => {}
                    }
                }
            }
            TyKind::ComptimeList(element) | TyKind::VariadicPack(element) => {
                walk(element, bound, frames)?;
            }
            pointer if let Some((element, _)) = pointer.pointer_layout() => {
                walk(element, bound, frames)?;
            }
            TyKind::Tuple(elements)
            | TyKind::RuntimePack(elements)
            | TyKind::Variant(elements)
            | TyKind::Overload(elements) => {
                for element in elements {
                    walk(element, bound, frames)?;
                }
            }
            TyKind::Assoc { base, .. } => walk(base, bound, frames)?,
            TyKind::Ref(reference) => walk(&reference.referent, bound, frames)?,
            // A symbolic lane or width of a vector or its `!kgen.simd`
            // storage is a parameter expression like a struct's value
            // argument; `verify/scope.rs` checks its binders and kind, and
            // the concrete mode rejects it.
            kind if let Some((dtype, width)) = kind
                .as_simd()
                .or_else(|| mojito_types::types::kgen_simd_slots(kind)) =>
            {
                if let mojito_types::types::SimdDtype::Expr(expr) = dtype {
                    check_expr(&expr, frames)?;
                }
                if let mojito_types::types::SimdWidth::Expr(expr) = width {
                    check_expr(&expr, frames)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(ty, enclosing, &mut Vec::new())
}

/// The binders the declaration owning function `name` declares: its own and,
/// for a method, its struct's.
fn enclosing_binders(name: &str, declarations: &MirDeclarations) -> HashSet<String> {
    let own = declarations
        .function(name)
        .into_iter()
        .flat_map(mojito_checked::declaration::FunctionDeclaration::argument_binders);
    let owner = name
        .split_once('.')
        .and_then(|(owner, _)| declarations.structure(owner))
        .into_iter()
        .flat_map(mojito_checked::declaration::StructDeclaration::argument_binders);
    own.chain(owner)
        .map(|declaration| declaration.name().trim_start_matches('*').to_string())
        .collect()
}

/// The storage a `MakeRef` handle designates. A capability-typed root already
/// contains a runtime frame/slot handle, so `MakeRef` forwards that handle and
/// extends its projection instead of borrowing the capability slot itself.
/// The whole of a pointer-typed root is the pointer, a value, as upstream's
/// `!lit.ref` of a pointer local addresses the local: it designates the
/// pointee only read through the slot (`through` naming the root) of a
/// local, never of a `mut`/`ref` parameter of `function`, whose handle
/// addresses the caller's pointer. An ordinary root, including a struct
/// field whose value happens to be a reference, is borrowed as storage and
/// therefore retains `place.ty`.
pub(super) fn make_ref_target<'p>(
    place: &'p MirPlace,
    function: &MirFunction,
) -> Option<(&'p Ty, Option<ReferencePermission>)> {
    let root_capability = place.root_ty.as_ref().and_then(reference_capability);
    let pointer_value = pointer_root_is_value(place, function);
    match (root_capability, place.is_whole_root()) {
        (Some(_), true) if pointer_value => place.ty.as_ref().map(|target| (target, None)),
        (Some(capability), true) => Some((capability.target, Some(capability.permission))),
        // A projection through a pointer the place loads on the way
        // (`elements[i][]`) addresses with that pointer's capability.
        (Some(capability), false) => {
            let permission = place
                .proj
                .iter()
                .skip(1)
                .zip(&place.projection_tys)
                .filter(|(projection, base)| {
                    matches!(projection, Proj::Index(_)) && base.is_pointer_layout()
                })
                .filter_map(|(_, base)| reference_capability(base))
                .next_back()
                .map_or(capability.permission, |inner| inner.permission);
            place.ty.as_ref().map(|target| (target, Some(permission)))
        }
        (None, _) => place.ty.as_ref().map(|target| (target, None)),
    }
}

/// Whether `place`'s pointer-typed root is the pointer itself rather than
/// its pointee: true unless the place reads a local through its own slot
/// (`through` naming the root). A `mut`/`ref` parameter's handle addresses
/// the caller's pointer, so through itself it is still the pointer.
pub(super) fn pointer_root_is_value(place: &MirPlace, function: &MirFunction) -> bool {
    let reference_parameter = function
        .ref_params
        .get(place.root as usize)
        .copied()
        .unwrap_or(false);
    (place.through != Some(place.root) || reference_parameter)
        && place
            .root_ty
            .as_ref()
            .is_some_and(|ty| ty.is_pointer_layout() || matches!(ty.kind(), TyKind::LitRef { .. }))
}

pub(super) fn verify_function(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    if function.var_names.len() != function.n_vars {
        errors.push(format!(
            "MIR function '{name}' has {} variable names for {} slots",
            function.var_names.len(),
            function.n_vars
        ));
    }
    for (&slot, ty) in &function.var_tys {
        let typed = matches!(ty.kind(), TyKind::Ref(_));
        if typed != function.binds_reference(slot) {
            errors.push(format!(
                "MIR function '{name}' slot $v{slot} `{}` {} a reference but is typed `{ty}`",
                function
                    .var_names
                    .get(slot as usize)
                    .map_or("?", String::as_str),
                if typed { "does not bind" } else { "binds" }
            ));
        }
    }
    if let Some(slot) = function
        .ref_bindings
        .iter()
        .find(|slot| !function.var_tys.contains_key(slot))
    {
        errors.push(format!(
            "MIR function '{name}' slot $v{slot} binds a reference but has no type"
        ));
    }
    match function.error_slot {
        None if function.raises => {
            errors.push(format!(
                "MIR function '{name}' raises but has no error slot"
            ));
        }
        Some(_) if !function.raises => {
            errors.push(format!(
                "MIR function '{name}' has an error slot but does not raise"
            ));
        }
        Some(slot) if slot as usize >= function.n_vars || (slot as usize) < function.n_params => {
            errors.push(format!(
                "MIR function '{name}' error slot $v{slot} is no local slot"
            ));
        }
        _ => {}
    }
    for (index, ty) in function.param_types.iter().enumerate() {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' parameter slot {index} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (slot, ty) in &function.var_tys {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' variable slot {slot} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (register, ty) in &function.reg_types {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' register r{register} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (description, ty) in [
        ("return", function.ret_ty.as_ref()),
        ("error", function.error_ty.as_ref()),
    ] {
        if ty.is_some_and(contains_runtime_pack) {
            errors.push(format!(
                "MIR function '{name}' {description} contract retains ABI-only RuntimePack type"
            ));
        }
    }
    verify_scope(name, function, declarations, errors);
    verify_slot_ends(name, function, declarations, errors);
    verify_register_destroys(name, function, declarations, errors);
    let context = RegionContext {
        regions: vec![RegionKind::Function],
        protected: false,
        asserted: false,
    };
    verify_block(
        name,
        function,
        declarations,
        &function.body,
        &context,
        errors,
    );
}

/// `RuntimePack[T0, ...]` distinguishes a specialized heterogeneous `*args`
/// declaration from an ordinary homogeneous `*args: Tuple[...]`. It may occur
/// only as the top-level positional-variadic ABI type. Once arguments have
/// been matched, the body sees an ordinary native `Tuple[T0, ...]` slot.
///
/// Only the program's own declarations are checked: an imported package's
/// were verified when the package was written.
pub(super) fn verify_runtime_pack_abi(declarations: &MirDeclarations, errors: &mut Vec<String>) {
    for declaration in &declarations.structs {
        for (field, ty) in &declaration.fields {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR struct '{}.{}' contains ABI-only RuntimePack type {ty}",
                    declaration.name, field
                ));
            }
        }
    }
    for declaration in &declarations.functions {
        let name = &declaration.lowered_name;
        let parameter_count = declaration.param_types.len();
        for (description, found) in [
            ("names", declaration.param_names.len()),
            ("defaults", declaration.defaults.len()),
            ("required mask", declaration.required.len()),
            ("conventions", declaration.param_conventions.len()),
            ("reference mask", declaration.ref_params.len()),
        ] {
            if found != parameter_count {
                errors.push(format!(
                    "MIR declaration '{name}' has {found} parameter {description}, expected {parameter_count}"
                ));
            }
        }
        if !declaration.has_receiver && declaration.receiver_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a receiver convention but no receiver"
            ));
        }
        if declaration.variadic.is_none() && declaration.variadic_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a positional variadic convention but no positional variadic parameter"
            ));
        }
        if declaration.kw_variadic.is_none() && declaration.kw_variadic_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a keyword variadic convention but no keyword variadic parameter"
            ));
        }
        for (index, convention) in declaration.param_conventions.iter().enumerate() {
            let expected_reference = matches!(
                convention,
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
            );
            if declaration.ref_params.get(index).copied() != Some(expected_reference) {
                errors.push(format!(
                    "MIR declaration '{name}' parameter {index} convention/reference mask disagree"
                ));
            }
        }
        for (index, ty) in declaration.param_types.iter().enumerate() {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR declaration '{name}' regular parameter {index} contains ABI-only RuntimePack type {ty}"
                ));
            }
        }
        if let Some(variadic) = &declaration.variadic {
            match variadic.kind() {
                TyKind::RuntimePack(elements) => {
                    if elements.iter().any(contains_runtime_pack) {
                        errors.push(format!(
                            "MIR declaration '{name}' has a nested RuntimePack variadic ABI"
                        ));
                    }
                }
                _ if contains_runtime_pack(variadic) => errors.push(format!(
                    "MIR declaration '{name}' embeds RuntimePack below its variadic ABI root"
                )),
                _ => {}
            }
        }
        for (description, ty) in [
            ("keyword variadic", declaration.kw_variadic.as_ref()),
            ("return", Some(&declaration.ret_ty)),
            ("error", declaration.error_ty.as_ref()),
        ] {
            if ty.is_some_and(contains_runtime_pack) {
                errors.push(format!(
                    "MIR declaration '{name}' {description} type contains ABI-only RuntimePack"
                ));
            }
        }
    }
}

/// Verify a `def(...)` bound's `__call__` requirement call: it names the
/// bound's requirement, reads its receiver, and binds the bound's
/// signature. A generic bound's call carries the bound's compile-time
/// parameter declarations with the arguments it applies, and binds the
/// signature they instantiate, else the symbolic one under the bound's
/// binders; a monomorphic bound's call carries none.
fn verify_bound_call(
    cx: &InstrCx<'_>,
    contract: &Ty,
    instruction: &MirInstr,
    callee: &str,
    owned_receiver: bool,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    let MirInstr::MethodCall {
        dest,
        raises,
        args,
        kwargs,
        spread,
        param_arg_regs,
        param_decls,
        instantiated_args,
        ..
    } = instruction
    else {
        return;
    };
    let expected = mojito_symbol::symbol::callable_requirement_target(contract);
    if expected.as_deref() != Some(callee) {
        errors.push(format!(
            "{prefix}: bound call target '{callee}' does not match callable contract '{}'",
            expected.unwrap_or_default()
        ));
    }
    if owned_receiver {
        errors.push(format!(
            "{prefix}: a `def(...)` bound's `__call__` takes its receiver `read`"
        ));
    }
    let mut instantiated = None;
    match generic_callable_decls(contract) {
        Some(decls) => {
            if decls != param_decls.as_slice() {
                errors.push(format!(
                    "{prefix}: a generic `def[...]` bound's call does not carry the bound's compile-time parameters"
                ));
            }
            verify_param_arguments(
                prefix,
                cx.function,
                decls,
                param_arg_regs,
                instantiated_args,
                None,
                errors,
            );
            if instantiated_args.is_empty() {
                errors.push(format!(
                    "{prefix}: a call through a generic `def[...]` bound carries no instantiation"
                ));
            } else {
                match instantiate_generic_callable_contract(contract, instantiated_args) {
                    Ok(signature) => instantiated = Some(signature),
                    Err(reason) => errors.push(format!(
                        "{prefix}: invalid generic callable instantiation: {reason}"
                    )),
                }
            }
        }
        None if !param_decls.is_empty()
            || !param_arg_regs.is_empty()
            || !instantiated_args.is_empty() =>
        {
            errors.push(format!(
                "{prefix}: a monomorphic `def(...)` bound's call carries compile-time parameter metadata"
            ));
        }
        None => {}
    }
    verify_callable_contract_call(
        cx,
        instantiated.as_ref().unwrap_or(contract),
        raises.as_ref().map(|raises| &raises.ty),
        *dest,
        args,
        kwargs,
        *spread,
        errors,
    );
}

fn verify_indirect_call(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let MirInstr::CallIndirect {
        dest,
        callee,
        resolved,
        raises,
        args,
        kwargs,
        spread,
        capture_accesses,
        param_arg_regs,
        param_decls,
        instantiated_contract,
        instantiated_args,
        ..
    } = instruction
    else {
        return;
    };
    verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
    // A bound's callee, its value or the address its requirement call
    // passes, is called through the bound's `__call__`.
    let callee_ty = cx.reg_ty(*callee).map(|ty| match ty.kind() {
        TyKind::Ref(reference) => &reference.referent,
        _ => ty,
    });
    if let Some(TyKind::Param {
        callable_bound: Some(bound),
        ..
    }) = callee_ty.map(Ty::kind)
        && matches!(bound.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
    {
        errors.push(format!(
            "{prefix}: a call through a `def(...)` bound is its `__call__` requirement call, not an indirect call"
        ));
    }
    let stored_contract = cx
        .reg_ty(*callee)
        .and_then(mojito_types::types::callable_contract_ty);
    let mut verified_instantiation = None;
    let contract = match stored_contract {
        Some(symbolic) if matches!(symbolic.kind(), TyKind::GenericFunc(_)) => {
            if let Some(found) = instantiated_contract {
                match instantiate_generic_callable_contract(symbolic, instantiated_args) {
                    Ok(expected) => {
                        if **found != expected {
                            errors.push(format!(
                                        "{prefix}: checker-instantiated callable contract does not match its retained generic arguments"
                                    ));
                        }
                        verified_instantiation = Some(expected);
                    }
                    Err(reason) => errors.push(format!(
                        "{prefix}: invalid generic callable instantiation: {reason}"
                    )),
                }
                verified_instantiation.as_ref()
            } else {
                // A call through a generic callable binds its parameters
                // as Mojo's `#kgen.bind_params` does: the call carries
                // its instantiation.
                errors.push(format!(
                    "{prefix}: a call through a generic callable carries no instantiation"
                ));
                if let Err(reason) = validate_dependent_bindings_in(
                    symbolic,
                    &enclosing_binders(cx.name, cx.declarations),
                ) {
                    errors.push(format!(
                        "{prefix}: invalid symbolic generic callable contract: {reason}"
                    ));
                }
                Some(symbolic)
            }
        }
        Some(contract) => {
            if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                errors.push(format!(
                    "{prefix}: nongeneric indirect call carries generic instantiation metadata"
                ));
            }
            Some(contract)
        }
        None => {
            if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                errors.push(format!(
                    "{prefix}: non-callable indirect operand carries generic instantiation metadata"
                ));
            }
            None
        }
    };
    if let Some(contract) = contract {
        verify_callable_contract_call(
            cx,
            contract,
            raises.as_ref().map(|raises| &raises.ty),
            *dest,
            args,
            kwargs,
            *spread,
            errors,
        );
    }
    let checked_decls = cx.reg_ty(*callee).and_then(generic_callable_decls);
    if let Some(checked_decls) = checked_decls {
        if checked_decls != param_decls.as_slice() {
            errors.push(format!(
                        "{prefix}: indirect-call compile-time parameter metadata does not match its callable contract"
                    ));
        }
        verify_param_arguments(
            prefix,
            cx.function,
            checked_decls,
            param_arg_regs,
            instantiated_args,
            None,
            errors,
        );
    } else if !param_decls.is_empty() {
        errors.push(format!(
            "{prefix}: nongeneric indirect call carries compile-time parameter metadata"
        ));
    }
    // A call of a struct value, or through a reference to one, is its
    // `__call__` method call, as upstream's `CallNode` resolves a callee
    // that is no function.
    let referent = std::iter::successors(cx.reg_ty(*callee), |ty| match ty.kind() {
        TyKind::Ref(reference) => Some(&reference.referent),
        _ => None,
    })
    .last();
    if let Some((name, _)) = referent.and_then(|ty| ty.kind().as_non_simd_struct()) {
        errors.push(format!(
            "{prefix}: a call of the struct value '{name}' is its `__call__` method call, not an indirect call"
        ));
        return;
    }
    if let Some(target) = resolved {
        if let Some(expected) = cx
            .reg_ty(*callee)
            .and_then(mojito_symbol::symbol::callable_contract_target)
            && target != &expected
        {
            errors.push(format!(
                "{prefix}: indirect-call target '{target}' does not match callable contract '{expected}'"
            ));
        }
        let is_call_target =
            mojito_symbol::symbol::split_method_symbol(target).is_some_and(|(_, method)| {
                method == "__call__" || mojito_symbol::symbol::is_overload_of(method, "__call__")
            });
        if !is_call_target {
            errors.push(format!(
                "{prefix}: indirect-call target '{target}' is not a __call__ method"
            ));
        }
        if let Some(declaration) = declared(cx.declarations, target) {
            let parameters =
                crate::mir::method_operand_parameters(declaration, args.len(), *spread, kwargs);
            verify_direct_call(cx, declaration, args, kwargs, parameters.as_deref(), errors);
        }
    }
}

/// An `in_place` binding is its producer's result storage: its source must be
/// a result of the nearest preceding call-family instruction, past only the
/// drops elaboration splices after the binding's producer.
fn verify_in_place_binding(
    name: &str,
    preceding: &[MirInstr],
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let MirInstr::DefVar {
        src,
        in_place: true,
        ..
    } = instruction
    else {
        return;
    };
    let producer = preceding.iter().rev().find(|instruction| {
        !matches!(
            instruction,
            MirInstr::Drop { .. } | MirInstr::DropVar { .. } | MirInstr::DropPlace { .. }
        )
    });
    let produced = producer.is_some_and(|producer| {
        matches!(
            producer,
            MirInstr::Call { .. }
                | MirInstr::CallIndirect { .. }
                | MirInstr::MethodCall { .. }
                | MirInstr::UnOp { .. }
                | MirInstr::BinOp { .. }
        ) && {
            let mut results = Vec::new();
            instruction_result_regs(producer, &mut results);
            results.contains(src)
        }
    });
    if !produced {
        errors.push(format!(
            "MIR function '{name}': in-place binding of r{} does not follow the call producing it",
            src.0
        ));
    }
}

/// A raising instruction is followed, in its block, by its error region,
/// as upstream's `addErrorRegions` follows each throwing call: an `if` on
/// its flag, whose `then` region the instruction enters having written its
/// error into its error slot. Lowering raises from there; inlining and its
/// folds may make that raise a break of the inlined body, or the handler
/// of a `try` they elide, may fold the region into an `if` deciding which
/// value a result of the instruction has, and may fold an inlined body's
/// region into the block's terminator, which carries the flag out to the
/// error region after the body. Nothing between the instruction and the
/// first `if` on its flag, or the terminator carrying it, reads its
/// results, which the raise path leaves undefined.
fn verify_error_region(
    name: &str,
    block: &MirBlock,
    index: usize,
    raises: &crate::mir::Raises,
    errors: &mut Vec<String>,
) {
    let prefix = format!("{name}: instruction {index}");
    let on_flag = |instruction: &MirInstr| matches!(instruction, MirInstr::If { cond, .. } if *cond == raises.flag);
    let rest = &block.instrs[index + 1..];
    let carried = || {
        crate::mir::terminator_regs(&block.term)
            .contains(&raises.flag)
            .then_some(rest.len())
    };
    let Some(region) = rest.iter().position(on_flag).or_else(carried) else {
        errors.push(format!(
            "{prefix}: raising instruction has no error region on its flag %r{}",
            raises.flag.0
        ));
        return;
    };
    let mut results = Vec::new();
    instruction_result_regs(&block.instrs[index], &mut results);
    let mut reads = Vec::new();
    for between in &rest[..region] {
        reads.clear();
        instruction_operand_regs(between, &mut reads);
        if let Some(read) = reads.iter().find(|read| results.contains(read)) {
            errors.push(format!(
                "{prefix}: %r{} is read before the error region decides it is defined",
                read.0
            ));
        }
    }
}

/// The convention by which a receiver of `convention` is taken ownership
/// of: `var` or `deinit`, else none.
fn owned_receiver(
    convention: Option<mojito_ast::ast::ArgConvention>,
) -> Option<mojito_ast::ast::ArgConvention> {
    convention.filter(|convention| {
        matches!(
            convention,
            mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Deinit
        )
    })
}

/// A receiver's struct binders, as an instance binds them by position,
/// beside its arguments ([`owner_arguments`]).
type OwnerArguments<'a> = (
    std::borrow::Cow<'a, [mojito_types::types::ParamDecl]>,
    std::borrow::Cow<'a, [TyArg]>,
);

/// The parameter types of `declaration`, a member of a struct declaring
/// parameters, at the owner instance one of its `Self`-typed operands
/// carries, as upstream checks a call against its generator applied to the
/// call's parameters: a `Counter[length]` member called on a
/// `Counter[length + 1]` declares `Counter[length + 1]` there. `None` when
/// no operand binds the owner, or the instance leaves a binder open.
fn owner_instance_param_types(
    cx: &InstrCx<'_>,
    declaration: &MirFunctionDeclaration,
    operands: &[crate::mir::BoundOperand<'_>],
) -> Option<Vec<Ty>> {
    let (owner, _) = mojito_symbol::symbol::split_method_symbol(&declaration.lowered_name)?;
    let decls = &cx.declarations.structure(owner)?.param_decls;
    let own = ParamDecl::self_arguments(decls);
    let instance = operands.iter().find_map(|operand| {
        let expected = declaration.param_types.get(operand.parameter?)?;
        let (_, arguments) = expected.kind().as_non_simd_struct()?;
        let found = cx.reg_ty(operand.register)?;
        let (found_name, found) = crate::mir::addressed_operand(found, expected)
            .map_or(found, |reference| &reference.referent)
            .kind()
            .as_non_simd_struct()?;
        (found_name == owner && *arguments == own && *found != own).then_some(found)
    })?;
    mojito_types::types::struct_instance_types(decls, instance, &declaration.param_types)
}
