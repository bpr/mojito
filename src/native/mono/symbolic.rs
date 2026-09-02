//! Symbolic-type detection and concreteness enforcement.

use super::*;

pub(super) fn eval_ct(expr: &CtExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    use CtExpr::*;
    let int = |value: CtValue| match value {
        CtValue::Int(v) => Ok(v),
        _ => Err(MonoError {
            function: None,
            construct: "dependent expression requires an Int value".to_string(),
        }),
    };
    Ok(match expr {
        Value(CtValue::Param(name)) | Param(name) => bindings
            .values
            .get(name)
            .cloned()
            .ok_or_else(|| MonoError {
                function: None,
                construct: format!("unresolved value parameter `{name}`"),
            })?,
        Value(value) => value.clone(),
        Neg(v) => CtValue::Int(int(eval_ct(v, bindings)?)?.wrapping_neg()),
        Add(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_add(int(eval_ct(b, bindings)?)?))
        }
        Sub(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_sub(int(eval_ct(b, bindings)?)?))
        }
        Mul(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_mul(int(eval_ct(b, bindings)?)?))
        }
        FloorDiv(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.div_euclid(int(eval_ct(b, bindings)?)?))
        }
        Mod(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.rem_euclid(int(eval_ct(b, bindings)?)?))
        }
        Pow(a, b) => CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_pow(
            u32::try_from(int(eval_ct(b, bindings)?)?).map_err(|_| MonoError {
                function: None,
                construct: "dependent exponent is out of range".to_string(),
            })?,
        )),
    })
}

pub(super) fn is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer
        | Ty::Param { .. }
        | Ty::Assoc { .. }
        | Ty::Dependent(_)
        | Ty::SelfType
        | Ty::GenericFunc { .. } => true,
        Ty::Struct(_, args) => args.iter().any(arg_has_symbolic),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            v.iter().any(is_symbolic)
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            is_symbolic(v)
        }
        Ty::Ref(v) => is_symbolic(&v.referent),
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
        TyArg::Val(CtValue::Param(_)) => true,
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
/// has become a direct specialized call. Remove their now-dead MIR plumbing so
/// neither verification nor backend lowering sees a fictitious runtime ABI.
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
                    | MirInstr::UseVar { dest, .. } => !generic_regs.contains(&dest.0),
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
                MirInstr::SizeOf { ty, .. } => out.push(ty.clone()),
                MirInstr::PointerStorageTake { element, .. }
                | MirInstr::PointerStorageDestroy { element, .. }
                | MirInstr::UninitStorageTake { element, .. }
                | MirInstr::UninitStorageDestroy { element, .. } => out.push(element.clone()),
                _ => {}
            }
        }
    }
}
pub(super) fn ensure_concrete_function(
    name: &str,
    function: &MirFunction,
) -> Result<(), MonoError> {
    if let Some(ty) = function_types(function).find(|ty| is_symbolic(ty)) {
        Err(MonoError {
            function: Some(name.to_string()),
            construct: format!("symbolic type `{ty}` remains after monomorphization"),
        })
    } else {
        Ok(())
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
            output.extend(v.iter().cloned())
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            output.push((**v).clone())
        }
        Ty::Ref(v) => output.push((*v.referent).clone()),
        _ => {}
    }
}
pub(super) fn nominal_template(name: &str) -> &str {
    name.split("$mono").next().unwrap_or(name)
}
