//! Symbolic-type detection and concreteness enforcement.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::{ParamError, ReflectQuery};

/// The constant a parameter expression denotes under the mono environment:
/// the shared replacement, every reflection query the instance's types
/// answer, every application of a lowered function the specializer has run
/// ([`Applications`]), and every builtin application its constants answer,
/// then explicit concrete extraction. Monomorphization holds no
/// arithmetic of its own. A query with no answer at the instance
/// (`field_index["z"]()` of a struct lacking `z`) fails the instantiation,
/// as at the pin, as does a `DType` float query at a non-float dtype.
pub(super) fn eval_ct(expr: &ParamExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    let context = ParamContext::detached();
    let replaced = context
        .replace(expr, &ct_bindings(bindings))
        .and_then(|replaced| {
            context.answer_reflections(&replaced, &mut |subject, query| {
                reflection_answer(subject, query, bindings)
            })
        })
        .and_then(|answered| match &bindings.applications {
            // A type binder among the arguments is the type the instance
            // binds it to.
            Some(applications) => context.answer_applications(&answered, &mut |function, args| {
                let args: Option<Vec<ParamExpr>> = args
                    .iter()
                    .map(|arg| match arg.kind() {
                        ParamKind::DeclRef(binder) => bindings
                            .types
                            .get(binder)
                            .filter(|ty| !is_symbolic(ty))
                            .and_then(|ty| {
                                context.constant(CtValue::Type(Box::new(ty.clone()))).ok()
                            }),
                        _ => Some(arg.clone()),
                    })
                    .collect();
                Ok(args.and_then(|args| applications.answer(function, &args)))
            }),
            None => Ok(answered),
        })
        .and_then(|answered| context.answer_builtin_applications(&answered))
        .map_err(|error| MonoError {
            kind: if matches!(error, ParamError::Reflect(_) | ParamError::Constraint(_)) {
                MonoErrorKind::Instantiation
            } else {
                MonoErrorKind::Unsupported
            },
            function: None,
            construct: error.to_string(),
        })?;
    // A layout application the instance's oracle answers under its target.
    if let ParamKind::Apply {
        function,
        args,
        evaluated: None,
    } = replaced.kind()
        && function == SIZE_OF_FUNCTION
        && let Some(oracle) = &bindings.layout
        && let [subject] = args.as_slice()
    {
        let ty = match subject.kind() {
            ParamKind::TypeShape(ty) => Some(substitute_ty(ty, bindings)?),
            ParamKind::Constant(CtValue::Type(ty)) => Some((**ty).clone()),
            _ => None,
        };
        if let Some(ty) = ty
            && !is_symbolic(&ty)
        {
            return oracle.size_of(&ty).map(CtValue::Int);
        }
    }
    replaced.require_constant().map_err(|error| MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: replaced.free_parameters().first().map_or_else(
            || error.to_string(),
            |parameter| format!("unresolved value parameter `{}`", parameter.name),
        ),
    })
}

/// The instance's value solutions as parameter-expression bindings, each
/// binder by its identity.
pub(super) fn ct_bindings(bindings: &Bindings) -> ParamBindings {
    let context = ParamContext::detached();
    let mut bound = ParamBindings::new();
    for (binder, value) in &bindings.values {
        if let Ok(value) = context.constant(value.clone()) {
            bound.bind(binder.id.clone(), value);
        }
    }
    bound
}

/// The answer to a reflection query over `subject` at the instance: a
/// source struct's fields at its arguments, under the policy the checker
/// applies to a closed subject. A subject the bindings leave symbolic, or a
/// struct the source does not declare, keeps the query.
fn reflection_answer(
    subject: &ParamExpr,
    query: &ReflectQuery,
    bindings: &Bindings,
) -> Result<Option<CtValue>, ParamError> {
    let subject = match subject.kind() {
        ParamKind::TypeShape(ty) => match substitute_ty(ty, bindings) {
            Ok(ty) => ty,
            Err(_) => return Ok(None),
        },
        ParamKind::Constant(CtValue::Type(ty)) => (**ty).clone(),
        _ => return Ok(None),
    };
    if is_symbolic(&subject) {
        return Ok(None);
    }
    let unsupported = |error: MonoError| ParamError::Unsupported(error.construct);
    let answer = match &subject {
        Ty::Struct(_, args) => {
            let Some(instance) = struct_instance(&subject, bindings).map_err(unsupported)? else {
                return Ok(None);
            };
            let fields = instance
                .shape
                .fields
                .iter()
                .map(|(name, ty)| {
                    substitute_ty(ty, &instance.bindings).map(|ty| (name.clone(), ty))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(unsupported)?;
            let declared = Ty::Struct(instance.template.to_string(), args.clone());
            query.answer(&declared, Some(&fields))
        }
        _ => query.answer(&subject, None),
    };
    answer.map(Some).map_err(ParamError::Reflect)
}

pub(super) fn is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer
        | Ty::Param { .. }
        | Ty::Assoc { .. }
        | Ty::Dependent(_)
        | Ty::SelfType
        | Ty::GenericFunc { .. } => true,
        Ty::Struct(_, args) => !args.is_closed() && args.iter().any(arg_has_symbolic),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            v.iter().any(is_symbolic)
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            is_symbolic(v)
        }
        Ty::Ref(v) => is_symbolic(&v.referent),
        Ty::Simd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(is_symbolic)
                || is_symbolic(ret)
                || variadic.as_deref().is_some_and(is_symbolic)
                || kw_variadic.as_deref().is_some_and(is_symbolic)
                || error.as_deref().is_some_and(is_symbolic)
        }
        _ => false,
    }
}
pub(super) fn arg_has_symbolic(arg: &TyArg) -> bool {
    match arg {
        TyArg::Ty(ty) => is_symbolic(ty),
        TyArg::Val(CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)) => true,
        TyArg::Val(_) | TyArg::Origin(_) => false,
    }
}
pub(super) fn function_types(function: &MirFunction) -> impl Iterator<Item = &Ty> {
    function
        .param_types
        .iter()
        .chain(function.ret_ty.iter())
        .chain(function.error_ty.iter())
        .chain(function.var_tys.values())
        .chain(function.reg_types.values())
}

/// Dependent callable values are compile-time carriers once every indirect use
/// has become a direct specialized call. Their storage and the parameters
/// that receive them become an `Int` placeholder: a value is the constant `0`,
/// which a call still passes to an instance that folded the callable into its
/// identity, so neither verification nor backend lowering sees a fictitious
/// runtime ABI.
pub(super) fn erase_specialized_generic_callable_storage(function: &mut MirFunction) {
    pub(super) fn erase(
        blocks: &mut [MirBlock],
        generic_regs: &HashSet<u32>,
        generic_vars: &HashSet<u32>,
    ) {
        for block in blocks {
            block.instrs.retain_mut(|instruction| {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    erase(body, generic_regs, generic_vars);
                    if let Some((_, blocks)) = handler {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    if let Some(blocks) = orelse {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    if let Some(blocks) = finalbody {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    return true;
                }
                match instruction {
                    MirInstr::MakeClosure { dest, .. }
                    | MirInstr::Const { dest, .. }
                    | MirInstr::CopyValue { dest, .. }
                    | MirInstr::UseVar { dest, .. }
                        if generic_regs.contains(&dest.0) =>
                    {
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: Const::Int(0),
                        };
                        true
                    }
                    MirInstr::DefVar { var, .. } => !generic_vars.contains(var),
                    _ => true,
                }
            });
        }
    }

    let generic_regs = function
        .reg_types
        .iter()
        .filter_map(|(reg, ty)| matches!(ty, Ty::GenericFunc { .. }).then_some(*reg))
        .collect::<HashSet<_>>();
    let generic_vars = function
        .var_tys
        .iter()
        .filter_map(|(var, ty)| matches!(ty, Ty::GenericFunc { .. }).then_some(*var))
        .collect::<HashSet<_>>();
    erase(&mut function.blocks, &generic_regs, &generic_vars);
    for reg in generic_regs {
        function.reg_types.insert(reg, Ty::Int);
    }
    for var in generic_vars {
        function.var_tys.insert(var, Ty::Int);
    }
    erase_generic_callable_parameters(&mut function.param_types);
}

/// A parameter typed by a generic callable receives the `Int` placeholder its
/// caller's erased storage passes
/// ([`erase_specialized_generic_callable_storage`]).
pub(super) fn erase_generic_callable_parameters(types: &mut [Ty]) {
    for ty in types {
        if matches!(ty, Ty::GenericFunc { .. }) {
            *ty = Ty::Int;
        }
    }
}

/// Collect already-substituted types named only by instructions, recursing
/// into `try` regions. These types need layouts or lifecycle declarations even
/// when no register or variable carries them.
pub(super) fn push_instruction_types(blocks: &[MirBlock], out: &mut Vec<Ty>) {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    push_instruction_types(body, out);
                    if let Some((_, blocks)) = handler {
                        push_instruction_types(blocks, out);
                    }
                    if let Some(blocks) = orelse {
                        push_instruction_types(blocks, out);
                    }
                    if let Some(blocks) = finalbody {
                        push_instruction_types(blocks, out);
                    }
                }
                MirInstr::SizeOf { ty, .. } | MirInstr::TypeName { ty, .. } => out.push(ty.clone()),
                MirInstr::PointerStorageTake { element, .. }
                | MirInstr::PointerStorageDestroy { element, .. }
                | MirInstr::UninitStorageTake { element, .. }
                | MirInstr::UninitStorageDestroy { element, .. } => out.push(element.clone()),
                _ => {}
            }
        }
    }
}
/// Reject an instance the verifier's concrete mode refuses, naming the
/// template it came from. Materialization asks per instance, ahead of the
/// whole-program verification, so the failure names its instance.
pub(super) fn ensure_concrete_function(
    template: &str,
    name: &str,
    function: &MirFunction,
) -> Result<(), MonoError> {
    let findings = mojito_mir::mir::verify::concrete_function_findings(name, function);
    if findings.is_empty() {
        Ok(())
    } else {
        Err(MonoError {
            kind: MonoErrorKind::Unsupported,
            function: Some(template.to_string()),
            construct: format!("{} after monomorphization", findings.join("; ")),
        })
    }
}
pub(super) fn collect_nested_types(ty: &Ty, output: &mut Vec<Ty>) {
    match ty {
        Ty::Struct(_, args) => output.extend(args.iter().filter_map(|a| {
            if let TyArg::Ty(t) = a {
                Some(t.clone())
            } else {
                None
            }
        })),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            output.extend(v.iter().cloned());
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            output.push((**v).clone());
        }
        Ty::Ref(v) => output.push((*v.referent).clone()),
        _ => {}
    }
}
pub(super) fn nominal_template(name: &str) -> &str {
    name.split("$mono").next().unwrap_or(name)
}
