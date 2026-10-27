//! Symbolic-type detection and concreteness enforcement.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::{ParamError, ReflectQuery};
use mojito_types::types::FuncTy;
use mojito_types::types::TyKind;

/// The constant a parameter expression denotes under the mono environment:
/// the shared replacement, every reflection query the instance's types
/// answer, every application of a lowered function the specializer has run
/// ([`Applications`]), and every builtin application its constants answer,
/// then explicit concrete extraction. Monomorphization holds no
/// arithmetic of its own. A query with no answer at the instance
/// (`field_index["z"]()` of a struct lacking `z`) fails the instantiation,
/// as at the pin, as does a `DType` float query at a non-float dtype.
pub(super) fn eval_ct(expr: &ParamExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    eval_ct_at(expr, bindings, false)
}

/// [`eval_ct`] of a value the instance reaches: one it cannot compute (an
/// index past its list) fails the instantiation too, as at the pin. A
/// substitution still meets the reads of arms the instance never takes.
pub(super) fn eval_reached_ct(expr: &ParamExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    eval_ct_at(expr, bindings, true)
}

/// The instance's solutions as parameter-expression bindings, each binder
/// by its identity: a value binder's constant, and a type binder's closed
/// type, which a type position inside an expression (a struct member's
/// `TypeList.of[Trait=AnyType, NoneType, Self.T].values`) reads.
pub(super) fn ct_bindings(bindings: &Bindings) -> ParamBindings {
    let context = ParamContext::detached();
    let mut bound = ParamBindings::new();
    for (binder, value) in &bindings.values {
        if let Ok(value) = context.constant(value.clone()) {
            bound.bind(binder.id.clone(), value);
        }
    }
    for (binder, ty) in &bindings.types {
        if !is_symbolic(ty) {
            bound.bind_type(binder.id.clone(), ty.clone());
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
        ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
        _ => return Ok(None),
    };
    if is_symbolic(&subject) {
        return Ok(None);
    }
    let unsupported = |error: MonoError| ParamError::Unsupported(error.construct);
    let answer = match subject.kind() {
        TyKind::Struct(_, args) => {
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
            let declared = Ty::new(TyKind::Struct(instance.template.to_string(), args.clone()));
            query.answer(&declared, Some(&fields))
        }
        _ => query.answer(&subject, None),
    };
    answer.map(Some).map_err(ParamError::Reflect)
}

/// The value of the compile-time member `name` of `subject` at the
/// instance: the source struct's member, evaluated under the struct
/// instance's own bindings (`Self.n + 1` of `B[4]` to `5`); a
/// parameterized member's body also binds its own parameters to `args` by
/// position, each evaluated under the reader's `bindings` (`get[0]` of
/// `S[4, 5]` to `4`). A subject or type argument the bindings leave
/// symbolic keeps the read.
fn type_member_answer(
    subject: &ParamExpr,
    name: &str,
    args: &[ParamExpr],
    bindings: &Bindings,
) -> Result<Option<CtValue>, ParamError> {
    let subject = match subject.kind() {
        ParamKind::TypeShape(ty) => match substitute_ty(ty, bindings) {
            Ok(ty) => ty,
            Err(_) => return Ok(None),
        },
        ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
        _ => return Ok(None),
    };
    if is_symbolic(&subject) {
        return Ok(None);
    }
    let unsupported = |error: MonoError| ParamError::Unsupported(error.construct);
    let Some(instance) = struct_instance(&subject, bindings).map_err(unsupported)? else {
        return Ok(None);
    };
    if !args.is_empty() {
        let member = instance
            .shape
            .parameterized_values
            .iter()
            .find(|member| member.name == name)
            .ok_or_else(|| {
                ParamError::Unsupported(format!("`{subject}` has no compile-time member `{name}`"))
            })?;
        if member.params.len() != args.len() {
            return Err(ParamError::Unsupported(format!(
                "`{subject}.{name}` takes {} compile-time argument(s), got {}",
                member.params.len(),
                args.len()
            )));
        }
        let mut arguments = Vec::with_capacity(args.len());
        for arg in args {
            let argument = match arg.kind() {
                ParamKind::TypeShape(ty) | ParamKind::Constant(CtValue::Type(ty)) => {
                    let ty = substitute_ty(ty, bindings).map_err(unsupported)?;
                    if is_symbolic(&ty) {
                        return Ok(None);
                    }
                    TyArg::Ty(ty)
                }
                _ => TyArg::Val(eval_ct(arg, bindings).map_err(unsupported)?),
            };
            arguments.push(argument);
        }
        let mut member_bindings = instance.bindings.clone();
        bind_ty_args(&member.params, &arguments, &mut member_bindings)
            .map_err(ParamError::Unsupported)?;
        return eval_ct(&member.body, &member_bindings)
            .map(Some)
            .map_err(unsupported);
    }
    let (_, value) = instance
        .shape
        .values
        .iter()
        .find(|(member, _)| member == name)
        .ok_or_else(|| {
            ParamError::Unsupported(format!("`{subject}` has no compile-time member `{name}`"))
        })?;
    eval_ct(value, &instance.bindings)
        .map(Some)
        .map_err(unsupported)
}

pub(super) fn is_symbolic(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Infer
        | TyKind::Param { .. }
        | TyKind::Assoc { .. }
        | TyKind::Dependent(_)
        | TyKind::SelfType
        | TyKind::GenericFunc(_) => true,
        TyKind::Struct(_, args) => !ty.is_closed() && args.iter().any(arg_has_symbolic),
        TyKind::Tuple(v) | TyKind::RuntimePack(v) | TyKind::Variant(v) | TyKind::Overload(v) => {
            v.iter().any(is_symbolic)
        }
        TyKind::ComptimeList(v)
        | TyKind::ParamList(v)
        | TyKind::VariadicPack(v)
        | TyKind::PopArray { element: v, .. }
        | TyKind::Pointer { element: v, .. } => is_symbolic(v),
        TyKind::Ref(v) => is_symbolic(&v.referent),
        TyKind::Simd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        TyKind::Func(func) => {
            let FuncTy {
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } = &**func;

            params.iter().any(is_symbolic)
                || is_symbolic(ret)
                || variadic.as_ref().is_some_and(is_symbolic)
                || kw_variadic.as_ref().is_some_and(is_symbolic)
                || error.as_ref().is_some_and(is_symbolic)
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
                let regions = mojito_mir::mir::instruction_regions_mut(instruction);
                if !regions.is_empty() {
                    for region in regions {
                        erase(region, generic_regs, generic_vars);
                    }
                    return true;
                }
                match instruction {
                    MirInstr::MakeClosure { dest, .. }
                    | MirInstr::Const { dest, .. }
                    | MirInstr::CopyValue { dest, .. }
                    | MirInstr::UseVar { dest, .. }
                    | MirInstr::LoadPlace { dest, .. }
                        if generic_regs.contains(&dest.0) =>
                    {
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: Const::Int(0),
                        };
                        true
                    }
                    // The erased slot holds nothing, so nothing ends there.
                    MirInstr::DefVar { var, .. }
                    | MirInstr::DropVar { var, .. }
                    | MirInstr::ConsumeVar { var } => !generic_vars.contains(var),
                    _ => true,
                }
            });
        }
    }

    let generic_regs = function
        .reg_types
        .iter()
        .filter_map(|(reg, ty)| matches!(ty.kind(), TyKind::GenericFunc(_)).then_some(*reg))
        .collect::<HashSet<_>>();
    let generic_vars = function
        .var_tys
        .iter()
        .filter_map(|(var, ty)| matches!(ty.kind(), TyKind::GenericFunc(_)).then_some(*var))
        .collect::<HashSet<_>>();
    erase(&mut function.blocks, &generic_regs, &generic_vars);
    for reg in generic_regs {
        function.reg_types.insert(reg, Ty::int());
    }
    for var in generic_vars {
        function.var_tys.insert(var, Ty::int());
    }
    erase_generic_callable_parameters(&mut function.param_types);
}

/// A parameter typed by a generic callable receives the `Int` placeholder its
/// caller's erased storage passes
/// ([`erase_specialized_generic_callable_storage`]).
pub(super) fn erase_generic_callable_parameters(types: &mut [Ty]) {
    for ty in types {
        if matches!(ty.kind(), TyKind::GenericFunc(_)) {
            *ty = Ty::int();
        }
    }
}

/// Collect already-substituted types named only by instructions, recursing
/// into every region. These types need layouts or lifecycle declarations even
/// when no register or variable carries them.
pub(super) fn push_instruction_types(blocks: &[MirBlock], out: &mut Vec<Ty>) {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::SizeOf { ty, .. }
                | MirInstr::TypeName { ty, .. }
                | MirInstr::ConstructType { ty, .. } => out.push(ty.clone()),
                MirInstr::PointerStorageTake { element, .. }
                | MirInstr::PointerStorageDestroy { element, .. }
                | MirInstr::UninitStorageTake { element, .. }
                | MirInstr::UninitStorageDestroy { element, .. } => out.push(element.clone()),
                instruction => {
                    for region in mojito_mir::mir::instruction_regions(instruction) {
                        push_instruction_types(region, out);
                    }
                }
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
    match ty.kind() {
        TyKind::Struct(_, args) => output.extend(args.iter().filter_map(|a| {
            if let TyArg::Ty(t) = a {
                Some(t.clone())
            } else {
                None
            }
        })),
        TyKind::Tuple(v) | TyKind::RuntimePack(v) | TyKind::Variant(v) | TyKind::Overload(v) => {
            output.extend(v.iter().cloned());
        }
        TyKind::ComptimeList(v)
        | TyKind::ParamList(v)
        | TyKind::VariadicPack(v)
        | TyKind::PopArray { element: v, .. }
        | TyKind::Pointer { element: v, .. } => {
            output.push((*v).clone());
        }
        TyKind::Ref(v) => output.push(v.referent.clone()),
        _ => {}
    }
}
pub(super) fn nominal_template(name: &str) -> &str {
    name.split("$mono").next().unwrap_or(name)
}

fn eval_ct_at(expr: &ParamExpr, bindings: &Bindings, reached: bool) -> Result<CtValue, MonoError> {
    let context = ParamContext::detached();
    // A layout query the instance's oracle answers under its target, and
    // the first one it cannot.
    let mut layout_failure = None;
    let replaced = context
        .replace(expr, &ct_bindings(bindings))
        .and_then(|replaced| {
            context.answer_reflections(&replaced, &mut |subject, query| {
                reflection_answer(subject, query, bindings)
            })
        })
        .and_then(|answered| {
            context.answer_type_members(&answered, &mut |subject, name, args| {
                type_member_answer(subject, name, args, bindings)
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
                            .and_then(|ty| context.constant(CtValue::Type(ty.clone())).ok()),
                        _ => Some(arg.clone()),
                    })
                    .collect();
                Ok(args.and_then(|args| applications.answer(function, &args)))
            }),
            None => Ok(answered),
        })
        .and_then(|answered| match &bindings.layout {
            Some(oracle) => context.answer_applications(&answered, &mut |function, args| {
                if function != SIZE_OF_FUNCTION || layout_failure.is_some() {
                    return Ok(None);
                }
                match layout_answer(oracle, args, bindings) {
                    Ok(size) => Ok(size.map(CtValue::Int)),
                    Err(error) => {
                        layout_failure = Some(error);
                        Ok(None)
                    }
                }
            }),
            None => Ok(answered),
        })
        .and_then(|answered| context.answer_builtin_applications(&answered))
        .map_err(|error| MonoError {
            kind: if matches!(error, ParamError::Reflect(_) | ParamError::Constraint(_))
                || (reached && matches!(error, ParamError::Arithmetic(_)))
            {
                MonoErrorKind::Instantiation
            } else {
                MonoErrorKind::Unsupported
            },
            function: None,
            construct: error.to_string(),
        })?;
    if let Some(error) = layout_failure {
        return Err(error);
    }
    replaced.require_constant().map_err(|error| {
        let free = replaced.free_parameters();
        // A read of an erased argument fails the instance, as no later
        // substitution binds it.
        if let Some(removed) = free
            .iter()
            .find(|parameter| bindings.removed.contains(parameter))
        {
            return MonoError {
                kind: MonoErrorKind::Instantiation,
                function: None,
                construct: removed_parameter_read(removed),
            };
        }
        MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct: free.first().map_or_else(
                || error.to_string(),
                |parameter| format!("unresolved value parameter `{}`", parameter.name),
            ),
        }
    })
}

/// The size `oracle` gives the one type the layout query's `args` name,
/// once `bindings` close it; `None` while it is still symbolic.
fn layout_answer(
    oracle: &LayoutOracle,
    args: &[ParamExpr],
    bindings: &Bindings,
) -> Result<Option<i64>, MonoError> {
    let [subject] = args else {
        return Ok(None);
    };
    let ty = match subject.kind() {
        ParamKind::TypeShape(ty) => substitute_ty(ty, bindings)?,
        ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
        _ => return Ok(None),
    };
    if is_symbolic(&ty) {
        return Ok(None);
    }
    oracle.size_of(&ty).map(Some)
}
