//! Type/field equivalence, constant-register scans, and callable
//! canonicalization.

use super::*;

/// Field-list equivalence for name-colliding struct instances: strict
/// structural equality except that pointer types collapse (one opaque
/// target word, drop-inert), recursing through nested aggregate shapes.
pub(super) fn fields_equivalent(a: &[(String, Ty)], b: &[(String, Ty)]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((a_name, a_ty), (b_name, b_ty))| a_name == b_name && ty_equivalent(a_ty, b_ty))
}

pub(super) fn ty_equivalent(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Pointer { .. }, Ty::Pointer { .. }) => true,
        (Ty::Struct(a_name, a_args), Ty::Struct(b_name, b_args)) => {
            a_name == b_name
                && a_args.len() == b_args.len()
                && a_args.iter().zip(b_args).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => ty_equivalent(a, b),
                    _ => a == b,
                })
        }
        (Ty::Tuple(a), Ty::Tuple(b)) | (Ty::RuntimePack(a), Ty::RuntimePack(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| ty_equivalent(a, b))
        }
        _ => a == b,
    }
}

/// An unresolved method-call shell for a VM by-name dunder dispatch
/// (`call_dunder`): the `MethodCall` rewrite arm resolves and retargets it
/// through the shared resolver like any source-level method call.
/// The compile-time value of a register defined by a `Const` in `function`,
/// when that constant has a `CtValue` form — the resolver for value-parameter
/// arguments spelled as materialized literal registers.
pub(super) fn const_reg_value(function: &MirFunction, reg: Reg) -> Option<CtValue> {
    const_reg_value_inner(function, reg, &mut HashSet::new())
}

pub(super) fn function_constant_values(function: &MirFunction) -> HashMap<u32, CtValue> {
    function
        .reg_types
        .keys()
        .filter_map(|reg| const_reg_value(function, Reg(*reg)).map(|value| (*reg, value)))
        .collect()
}

/// Resolve statically named callable values through the MIR's register/variable
/// plumbing. Generic lifted bodies are specialized at their indirect call site;
/// the boolean records whether direct-call rewriting may erase the environment.
pub(super) fn function_callable_targets(function: &MirFunction) -> HashMap<u32, (String, bool)> {
    pub(super) fn visit(
        blocks: &[MirBlock],
        registers: &mut HashMap<u32, (String, bool)>,
        variables: &mut HashMap<u32, (String, bool)>,
    ) -> bool {
        let mut changed = false;
        for block in blocks {
            for instruction in &block.instrs {
                let resolved = match instruction {
                    MirInstr::MakeClosure {
                        dest,
                        function,
                        captures,
                    } => Some((dest.0, (function.clone(), captures.is_empty()))),
                    MirInstr::Const {
                        dest,
                        k: Const::Function(function),
                    } => Some((dest.0, (function.clone(), true))),
                    MirInstr::CopyValue { dest, value } => registers
                        .get(&value.0)
                        .cloned()
                        .map(|value| (dest.0, value)),
                    MirInstr::UseVar { dest, var, .. } => {
                        variables.get(var).cloned().map(|value| (dest.0, value))
                    }
                    _ => None,
                };
                if let Some((dest, value)) = resolved
                    && registers.get(&dest) != Some(&value)
                {
                    registers.insert(dest, value);
                    changed = true;
                }
                if let MirInstr::DefVar { var, src, .. } = instruction
                    && let Some(value) = registers.get(&src.0).cloned()
                    && variables.get(var) != Some(&value)
                {
                    variables.insert(*var, value);
                    changed = true;
                }
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    changed |= visit(body, registers, variables);
                    if let Some((_, blocks)) = handler {
                        changed |= visit(blocks, registers, variables);
                    }
                    if let Some(blocks) = orelse {
                        changed |= visit(blocks, registers, variables);
                    }
                    if let Some(blocks) = finalbody {
                        changed |= visit(blocks, registers, variables);
                    }
                }
            }
        }
        changed
    }

    let mut registers = HashMap::new();
    let mut variables = HashMap::new();
    while visit(&function.blocks, &mut registers, &mut variables) {}
    registers
}

pub(super) fn const_reg_value_inner(
    function: &MirFunction,
    reg: Reg,
    visiting: &mut HashSet<u32>,
) -> Option<CtValue> {
    if !visiting.insert(reg.0) {
        return None;
    }
    for block in &function.blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::Const { dest, k } if *dest == reg => {
                    return match k {
                        Const::Int(value) => Some(CtValue::Int(*value)),
                        Const::IntLiteral(literal) => literal.to_i64().map(CtValue::Int),
                        Const::Bool(value) => Some(CtValue::Bool(*value)),
                        Const::Function(function) => Some(CtValue::Str(function.clone())),
                        _ => None,
                    };
                }
                MirInstr::MaterializeLiteral { dest, value, .. } if *dest == reg => {
                    return const_reg_value_inner(function, *value, visiting);
                }
                _ => {}
            }
        }
    }
    None
}

pub(super) fn dunder_method_call(
    dest: Reg,
    recv: Reg,
    method: &str,
    resolved: Option<String>,
    args: Vec<Reg>,
) -> MirInstr {
    MirInstr::MethodCall {
        dest,
        recv,
        method: method.to_string(),
        resolved,
        raises: None,
        reference_result: None,
        result_adapter: None,
        args,
        kwargs: Vec::new(),
        recv_place: None,
        recv_writes: false,
        arg_places: Vec::new(),
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
    }
}

/// How many leading method `param_decls` restate the owner struct's own
/// parameters: `__init__` declarations prepend them (`src/mir.rs`), and the
/// owner-bound instance identity already carries their solutions.
pub(super) fn owner_covered_prefix(
    struct_params: &[ParamDecl],
    method_params: &[ParamDecl],
) -> usize {
    if struct_params.is_empty() || method_params.len() < struct_params.len() {
        return 0;
    }
    if struct_params
        .iter()
        .zip(method_params)
        .all(|(s, m)| s.name() == m.name())
    {
        struct_params.len()
    } else {
        0
    }
}

/// Structural type equality that ignores `ref` and pointer origin components
/// (which erase from the runtime ABI and vary per call site), while still
/// requiring mutability agreement.
pub(super) fn ty_equal_modulo_origins(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Ref(a), Ty::Ref(b)) => {
            a.mutability == b.mutability && ty_equal_modulo_origins(&a.referent, &b.referent)
        }
        (Ty::Pointer { element: a, .. }, Ty::Pointer { element: b, .. }) => {
            ty_equal_modulo_origins(a, b)
        }
        (Ty::Struct(a_name, a_args), Ty::Struct(b_name, b_args)) => {
            a_name == b_name
                && a_args.len() == b_args.len()
                && a_args.iter().zip(b_args).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => ty_equal_modulo_origins(a, b),
                    (TyArg::Origin(_), TyArg::Origin(_)) => true,
                    _ => a == b,
                })
        }
        (Ty::Tuple(a), Ty::Tuple(b))
        | (Ty::RuntimePack(a), Ty::RuntimePack(b))
        | (Ty::Variant(a), Ty::Variant(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| ty_equal_modulo_origins(a, b))
        }
        (Ty::ComptimeList(a), Ty::ComptimeList(b)) | (Ty::VariadicPack(a), Ty::VariadicPack(b)) => {
            ty_equal_modulo_origins(a, b)
        }
        // Callable environments (`thin` vs `capturing[origin@N]`) and
        // parameter-name/convention spellings erase from the runtime ABI:
        // one two-word value shape serves every `def(...)` contract with the
        // same parameter/return/raising structure.
        (
            Ty::Func {
                params: a_params,
                ret: a_ret,
                required: a_required,
                variadic: a_variadic,
                kw_variadic: a_kw_variadic,
                positional_only: a_positional_only,
                keyword_only: a_keyword_only,
                raises: a_raises,
                error: a_error,
                ..
            },
            Ty::Func {
                params: b_params,
                ret: b_ret,
                required: b_required,
                variadic: b_variadic,
                kw_variadic: b_kw_variadic,
                positional_only: b_positional_only,
                keyword_only: b_keyword_only,
                raises: b_raises,
                error: b_error,
                ..
            },
        ) => {
            let option_eq = |a: &Option<Box<Ty>>, b: &Option<Box<Ty>>| match (a, b) {
                (Some(a), Some(b)) => ty_equal_modulo_origins(a, b),
                (None, None) => true,
                _ => false,
            };
            a_raises == b_raises
                && a_required == b_required
                && a_positional_only == b_positional_only
                && a_keyword_only == b_keyword_only
                && a_params.len() == b_params.len()
                && a_params
                    .iter()
                    .zip(b_params)
                    .all(|(a, b)| ty_equal_modulo_origins(a, b))
                && ty_equal_modulo_origins(a_ret, b_ret)
                && option_eq(a_variadic, b_variadic)
                && option_eq(a_kw_variadic, b_kw_variadic)
                && option_eq(a_error, b_error)
        }
        _ => a == b,
    }
}

/// Erase the callable-environment spelling from every `Ty::Func` in `ty`,
/// recursively. Environments (`thin` vs `capturing[...]`) are semantic
/// origin facts with no runtime ABI: instance identity and binding solutions
/// must not split on them (`capturing[_]` vs `capturing[origin@N]` is the
/// same closure value).
pub(super) fn canonicalize_callable(ty: &Ty) -> Ty {
    let mut canonical = ty.clone();
    erase_callable_environments(&mut canonical);
    canonical
}

pub(super) fn erase_callable_environments(ty: &mut Ty) {
    match ty {
        Ty::Func {
            environment,
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            *environment = mojito_types::origin::CallableEnvironment::Default;
            for param in params {
                erase_callable_environments(param);
            }
            erase_callable_environments(ret);
            if let Some(variadic) = variadic {
                erase_callable_environments(variadic);
            }
            if let Some(kw_variadic) = kw_variadic {
                erase_callable_environments(kw_variadic);
            }
            if let Some(error) = error {
                erase_callable_environments(error);
            }
        }
        Ty::Struct(_, args) => {
            for arg in args {
                if let TyArg::Ty(ty) = arg {
                    erase_callable_environments(ty);
                }
            }
        }
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            for element in elements {
                erase_callable_environments(element);
            }
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) => {
            erase_callable_environments(element);
        }
        Ty::Pointer { element, .. } => erase_callable_environments(element),
        Ty::Ref(reference) => erase_callable_environments(&mut reference.referent),
        _ => {}
    }
}

/// The referent behind any number of reference layers — the VM dereferences
/// `Value::Ref` operands before nominal dispatch.
pub(super) fn peel_refs(ty: &Ty) -> &Ty {
    let mut ty = ty;
    while let Ty::Ref(reference) = ty {
        ty = &reference.referent;
    }
    ty
}

pub(super) fn reg_ty<'a>(
    function: &'a MirFunction,
    reg: Reg,
    owner: &str,
) -> Result<&'a Ty, MonoError> {
    function.reg_types.get(&reg.0).ok_or_else(|| MonoError {
        function: Some(owner.to_string()),
        construct: format!("register r{} lacks a concrete type", reg.0),
    })
}
