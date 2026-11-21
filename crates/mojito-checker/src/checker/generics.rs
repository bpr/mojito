//! Generic parameter classification, solving, substitution, and bound checks.

use super::constraints::LACKING_EVIDENCE;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
pub(super) use mojito_symbol::symbol::materialized_instantiation_argument;
use mojito_types::origin::{RefSig, SigOrigin};
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};
pub use mojito_types::types::{map_tyargs, substitute};

pub(super) fn unify(
    pattern: &Ty,
    actual: &Ty,
    subst: &mut TySubst,
    deduce: &dyn Fn(&Ty) -> Ty,
) -> Result<(), TypeError> {
    match pattern.kind() {
        TyKind::Param { binder, .. } => {
            // An argument binds the type it materializes to, as the pin's
            // deduction materializes a `@__nonmaterializable` struct.
            let solved = deduce(actual);
            match subst.get(&binder.id) {
                None => {
                    subst.insert(binder.id.clone(), solved);
                }
                Some(existing) if *existing == solved => {}
                // A literal argument against an already-bound parameter
                // (`Pair[Float64](0.5, 1)`) materializes to the binding, as
                // it does against a concrete parameter type.
                Some(existing)
                    if actual.kind().is_numeric_literal() && coerces(actual, existing) => {}
                Some(existing) => {
                    return Err(TypeError::TypeMismatch {
                        expected: existing.to_string(),
                        found: solved.to_string(),
                        context: format!("type parameter '{}'", binder.name),
                    });
                }
            }
            Ok(())
        }
        // Recurse into a parameterized struct pattern to solve nested type
        // parameters (`Pair[T]` against `Pair[Int]` solves `T = Int`). Value
        // arguments contribute no type solution. A structural mismatch is left
        // for the caller's coercion check to report.
        struct_ty if let Some((pn, pargs)) = struct_ty.as_non_simd_struct() => {
            // A spread of a pack against a closed element list is a whole
            // argument list, which `solve_value_args` binds: its one slot is
            // no element pattern. Against another spread it forwards that
            // pack, which the slot below binds.
            if mojito_types::types::pack_spread_argument(pargs).is_some()
                && !actual
                    .kind()
                    .as_non_simd_struct()
                    .is_some_and(|(_, aargs)| {
                        mojito_types::types::pack_spread_argument(aargs).is_some()
                    })
            {
                return Ok(());
            }
            if let Some((an, aargs)) = actual.kind().as_non_simd_struct()
                && pn == an
                && pargs.len() == aargs.len()
            {
                for (p, a) in pargs.iter().zip(aargs) {
                    if let (TyArg::Ty(p), TyArg::Ty(a)) = (p, a) {
                        unify(p, a, subst, deduce)?;
                    }
                }
            } else if is_span_struct_name(pn)
                && let Some(element_pattern) = pargs.iter().find_map(|argument| match argument {
                    TyArg::Ty(element) => Some(element),
                    _ => None,
                })
                && let Some(element) = list_element(actual)
            {
                // The checked analogue of `Span`'s `@implicit` constructor from
                // a `List`: a `List[X]` argument solves a `Span[T, _]`
                // parameter's element (`join[T](elems: Span[T, _])` called with
                // a `List[String]`); the argument then converts as a call
                // argument does.
                unify(element_pattern, element, subst, deduce)?;
            }
            Ok(())
        }
        TyKind::Variant(pattern_alternatives) => {
            if let TyKind::Variant(actual_alternatives) = actual.kind()
                && pattern_alternatives.len() == actual_alternatives.len()
            {
                for (pattern, actual) in pattern_alternatives.iter().zip(actual_alternatives) {
                    unify(pattern, actual, subst, deduce)?;
                }
            }
            Ok(())
        }
        TyKind::Func(func) => {
            let FuncTy {
                environment: pattern_environment,
                params: pattern_params,
                ret: pattern_ret,
                error: pattern_error,
                ..
            } = &**func;

            if let Some(FuncTy {
                environment: actual_environment,
                params: actual_params,
                ret: actual_ret,
                error: actual_error,
                ..
            }) = actual.as_func()
            {
                if !callable_environment_coerces(actual_environment, pattern_environment) {
                    return Ok(());
                }
                for (pattern, actual) in pattern_params.iter().zip(actual_params) {
                    unify(pattern, actual, subst, deduce)?;
                }
                unify(pattern_ret, actual_ret, subst, deduce)?;
                if let (Some(pattern), Some(actual)) = (pattern_error, actual_error) {
                    unify(pattern, actual, subst, deduce)?;
                } else if let Some(pattern) = pattern_error {
                    unify(pattern, &Ty::never(), subst, deduce)?;
                }
            }
            Ok(())
        }
        // A raw parameter list solves through its element
        // (`KGENParamList[type]` against `KGENParamList[Int]`).
        TyKind::ParamList(pattern_element) => match actual.kind() {
            TyKind::ParamList(element) => unify(pattern_element, element, subst, deduce),
            _ => Ok(()),
        },
        // A reference pattern solves through its referent; the actual may be
        // the bare referent type (reference reads type as the referent).
        TyKind::Ref(pattern_reference) => match actual.kind() {
            TyKind::Ref(actual_reference) => unify(
                &pattern_reference.referent,
                &actual_reference.referent,
                subst,
                deduce,
            ),
            _ => unify(&pattern_reference.referent, actual, subst, deduce),
        },
        // A pointer pattern solves through its pointee (`Pointer[Self.T,
        // Self.origin]` against `Pointer[Int, …]` solves `T = Int`); the
        // provenance is bound separately (constructor origin binding).
        pointer if let Some(pattern_element) = pointer.pointer_element() => {
            if let Some(element) = actual.pointer_element() {
                unify(pattern_element, element, subst, deduce)?;
            }
            Ok(())
        }
        // So do its raw `!kgen.pointer` and `!lit.ref`.
        TyKind::KgenPointer {
            element: pattern_element,
            ..
        } => match actual.kind() {
            TyKind::KgenPointer { element, .. } => unify(pattern_element, element, subst, deduce),
            _ => Ok(()),
        },
        TyKind::LitRef {
            element: pattern_element,
            ..
        } => match actual.kind() {
            TyKind::LitRef { element, .. } => unify(pattern_element, element, subst, deduce),
            _ => Ok(()),
        },
        TyKind::LitRefPack {
            elements: pattern_elements,
            ..
        } => match actual.kind() {
            TyKind::LitRefPack { elements, .. } => pattern_elements
                .iter()
                .zip(elements)
                .try_for_each(|(pattern, element)| unify(pattern, element, subst, deduce)),
            _ => Ok(()),
        },
        // A projection solves through its base against the same projection
        // of another base (`I.Element` against `T.IteratorOwnedType.Element`
        // solves `I = T.IteratorOwnedType`), as the pin's deduction matches
        // an associated-type access structurally.
        TyKind::Assoc {
            base: pattern_base,
            name: pattern_name,
            args: pattern_args,
        } => {
            if let TyKind::Assoc { base, name, args } = actual.kind()
                && name == pattern_name
                && args.len() == pattern_args.len()
            {
                unify(pattern_base, base, subst, deduce)?;
                for (pattern, actual) in pattern_args.iter().zip(args) {
                    if let (TyArg::Ty(pattern), TyArg::Ty(actual)) = (pattern, actual) {
                        unify(pattern, actual, subst, deduce)?;
                    }
                }
            }
            Ok(())
        }
        // A non-parameter pattern contributes no solution; coercion is checked
        // separately by the caller.
        _ => Ok(()),
    }
}

/// Substitute a member template type at a receiver's type arguments: type
/// parameters via [`substitute`], value parameters via the receiver's
/// `TyArg::Val` bindings — `other: Self` on an `Array[Int, 3]` receiver
/// becomes `Array[Int, 3]`, not `Array[Int, length]`.
pub(super) fn substitute_at(ty: &Ty, info: &StructInfo, targs: &[TyArg]) -> Ty {
    substitute_at_with(ty, info, targs, None)
}

/// A member's `*args` collector element at a receiver's type arguments:
/// a bare struct pack (`*b: *Self.Ts`) closed by the receiver is the runtime
/// pack of its elements, as a method-level pack solved from the call is. A
/// caller's pack bound whole (`V[*Ts]`) is the struct's pack there, so it
/// carries the struct pack's declared bounds, which a spread of the caller's
/// collector must match exactly.
pub(super) fn substitute_variadic_at(ty: &Ty, info: &StructInfo, targs: &[TyArg]) -> Ty {
    let TyKind::Param { binder, .. } = ty.kind() else {
        return substitute_at(ty, info, targs);
    };
    let Some(declared) = info.params.iter().find_map(|decl| match decl {
        ParamDecl::Type {
            id,
            bounds,
            variadic: true,
            ..
        } if *id == binder.id => Some(bounds),
        _ => None,
    }) else {
        return substitute_at(ty, info, targs);
    };
    let retyped = |element: &mut Ty| {
        if matches!(element.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')) {
            element.update(|kind| {
                if let TyKind::Param { bounds, .. } = kind {
                    bounds.clone_from(declared);
                }
            });
        }
    };
    if let Some((_, mut elements)) = bound_pack_elements(&info.decls(), &info.view_arguments(targs))
    {
        if let [element] = elements.as_mut_slice() {
            retyped(element);
        }
        TyKind::RuntimePack(elements).into()
    } else {
        let mut element = substitute_at(ty, info, targs);
        retyped(&mut element);
        element
    }
}

/// Expand every spread of a variadic struct's pack in a member template type
/// (`Tuple[*Self.Ts]`, `other: Self`) to the elements a closed application
/// binds it to, whether the application spells the pack element by element
/// (`Tuple[Int, Bool]`) or as one bound list (a resolved `Pair[Int, Bool]`).
/// A type over a pack still open, or of a struct with no pack, is returned as
/// it is.
pub(super) fn expand_bound_pack(ty: &Ty, decls: &[ParamDecl], targs: &[TyArg]) -> Ty {
    match bound_pack_elements(decls, targs) {
        Some((pack, elements)) => mojito_types::types::expand_pack_spread(ty, &pack, &elements),
        None => ty.clone(),
    }
}

/// `ty` with every spread of a pack the callable itself declares expanded to
/// the elements this use solved it to.
///
/// `other: Tuple[*OtherTs]` at `OtherTs = [Int, Bool]` is `Tuple[Int,
/// Bool]`. A pack still open, or forwarded from the caller's, keeps its
/// spread.
pub(super) fn expand_solved_packs(ty: &Ty, decls: &[ParamDecl], tyargs: &[TyArg]) -> Ty {
    ParamDecl::aligned_pairs(decls, tyargs)
        .filter_map(|(decl, argument)| match (decl, argument) {
            (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(values))) => values
                .iter()
                .map(|value| match value {
                    CtValue::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|elements| (decl.spelling(), elements)),
            _ => None,
        })
        .fold(ty.clone(), |ty, (pack, elements)| {
            mojito_types::types::expand_pack_spread(&ty, pack, &elements)
        })
}

/// The pack a closed application of a variadic struct binds, with its
/// element types, in either spelling — the pack's own argument beside the
/// struct's other parameters (`Bad[Int, Bool]` of `Bad[T, *Ts]`), or the
/// sole pack's elements spelled positionally; `None` while the pack is open
/// (a forwarded spread) or the struct has no pack.
pub(super) fn bound_pack_elements(
    decls: &[ParamDecl],
    targs: &[TyArg],
) -> Option<(String, Vec<Ty>)> {
    let solved = (decls.len() == targs.len())
        .then(|| {
            ParamDecl::aligned_pairs(decls, targs).find_map(|(decl, argument)| {
                match (decl, argument) {
                    (
                        ParamDecl::Type { variadic: true, .. },
                        TyArg::Val(binding @ CtValue::Tuple(_)),
                    ) => Some((decl.spelling().to_string(), binding.clone())),
                    _ => None,
                }
            })
        })
        .flatten();
    let (pack, binding) = match solved {
        Some(solved) => solved,
        None => {
            positional_pack_binding(decls, targs).and_then(|(pack, binding)| match binding {
                TyArg::Val(value) => Some((pack, value)),
                _ => None,
            })?
        }
    };
    let CtValue::Tuple(types) = binding else {
        return None;
    };
    types
        .iter()
        .map(|value| match value {
            CtValue::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|elements| (pack, elements))
}

/// The value-parameter bindings a resolved application implies: each
/// `ParamDecl::Value` paired with its solved `TyArg::Val` argument, and each
/// type pack with its element list, which a dependent `Ts[i]` reads.
pub(super) fn solved_value_bindings(
    decls: &[ParamDecl],
    tyargs: &[TyArg],
) -> HashMap<String, CtValue> {
    // A type pack's list binder is keyed as the pack, `*Ts`, whose
    // visible binder's `TypeList` binds nothing a type reads.
    ParamDecl::aligned_pairs(decls, tyargs)
        .filter(|(decl, _)| !decl.is_type_pack())
        .filter_map(|(decl, argument)| match (decl, argument) {
            (
                ParamDecl::Value { .. } | ParamDecl::Type { variadic: true, .. },
                TyArg::Val(value),
            ) => Some((decl.binder().name.to_string(), value.clone())),
            _ => None,
        })
        .collect()
}

/// Solve value parameters from a pattern/actual type pair, the value-argument
/// counterpart of [`unify`]: a pattern's symbolic `TyArg::Val(CtValue::Expr)`
/// binds to the actual's value argument in the same slot (`Array[T, length]`
/// against `Array[Int, 3]` solves `length = 3`). First solution wins; a
/// structural mismatch contributes nothing, exactly like `unify`.
pub(super) fn solve_value_args(pattern: &Ty, actual: &Ty, out: &mut HashMap<String, CtValue>) {
    match (pattern.kind(), actual.kind()) {
        // A symbolic lane binds to the actual's own dtype (`Scalar[dt]`
        // against `Scalar[DType.int32]` solves `dt`), and a numeric literal
        // binds it at the literal's default type (`kind(3)` solves
        // `DType.int`, `kind(2.5)` `DType.float64`), as upstream materializes
        // it. A `Bool` is no SIMD but converts into `Scalar[DType.bool]`, so
        // it binds `DType.bool`. The width slot holds a `SIMDLength`, so it
        // is a struct value argument: a `SIMDLength` binder (`SIMD[dt, w]`
        // with `w: SIMDLength`) binds to the actual's length, `SIMDLength(4)`
        // for a four-lane vector, and an `Int` binder is solved from the
        // construction that converted the actual's width, `SIMDLength(w)`
        // against `SIMDLength(n)`, as at the pin. A known width, or a length
        // no conversion spells (`m`, `m + 1`, an alias), solves no `Int`
        // binder.
        // `!kgen.simd` storage solves its slots as the vector it stores, but
        // only from a slot a binder spells (`!kgen.simd<width._mlir_value,
        // dtype._mlir_value>` in a body generic over them), as the pin's
        // unifier does: a concrete or literal slot binds nothing, so
        // `SIMD(mlir_value=v._mlir_value)` over a concrete `v` infers no
        // `dtype` or `length`. A template's selection is the one every
        // instance runs; no instance re-solves its storage patterns.
        (TyKind::KgenSimd { .. }, TyKind::KgenSimd { .. }) => {
            let slots = |ty: &Ty| {
                mojito_types::types::storage_simd(ty)
                    .and_then(|vector| mojito_types::types::simd_slots(vector.kind()))
            };
            if let (Some((dtype, width)), Some((actual_dtype, actual_width))) =
                (slots(pattern), slots(actual))
            {
                // Only a slot that is a binder itself binds: a width an
                // `Int` binder converts to (`SIMD[dt, n]` over `n: Int`)
                // spells no `SIMDLength` the pin's unifier can read back.
                let dtype_binder = matches!(&actual_dtype,
                    SimdDtype::Expr(slot) if slot.as_decl_ref().is_some());
                let width_binder = matches!(&actual_width,
                    SimdWidth::Expr(slot) if slot.as_decl_ref().is_some());
                solve_simd_slot_values(
                    &dtype,
                    &width,
                    dtype_binder.then_some(actual_dtype),
                    width_binder.then_some(actual_width),
                    out,
                );
            }
        }
        // A vector of addresses solves its width as an `int` vector's
        // storage of that width would.
        (
            TyKind::KgenAddressSimd { width },
            TyKind::KgenAddressSimd {
                width: actual_width,
            },
        ) => {
            let storage = |width: &SimdWidth| -> Ty {
                TyKind::KgenSimd {
                    dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int),
                    width: width.clone(),
                }
                .into()
            };
            solve_value_args(&storage(width), &storage(actual_width), out);
        }
        // A literal's value binds an `IntLiteral[v]` or `FloatLiteral[v]`
        // pattern's binder, as a literal's text binds a `StringLiteral[v]`
        // one's through the struct arm below.
        (pattern_kind, actual_kind)
            if let Some((pattern, actual)) = pattern_kind
                .int_literal_slot()
                .zip(actual_kind.int_literal_slot())
                .or_else(|| {
                    pattern_kind
                        .float_literal_slot()
                        .zip(actual_kind.float_literal_slot())
                }) =>
        {
            if let CtValue::Expr(expr) = pattern {
                if let Some(reference) = expr.as_decl_ref() {
                    out.entry(reference.name.to_string())
                        .or_insert_with(|| actual.clone());
                } else if let CtValue::Expr(actual) = actual {
                    solve_applied_args(expr, actual, out);
                }
            }
        }
        (pattern_kind, actual_kind)
            if let Some(pattern) = pattern_kind.float_literal_slot()
                && actual_kind.is_int_literal() =>
        {
            if let CtValue::Expr(expr) = pattern
                && let Some(reference) = expr.as_decl_ref()
                && let Some(value) = mojito_types::types::int_literal_as_float(actual)
            {
                out.entry(reference.name.to_string()).or_insert(value);
            }
        }
        (kind, _) if let Some((dtype, width)) = kind.as_simd() => {
            let actual_slots = match actual.kind() {
                kind if kind.is_bool() => {
                    Some((SimdDtype::Known(Dtype::Bool), SimdWidth::Known(1)))
                }
                _ => simd_slots(actual),
            };
            if let Some((actual_dtype, actual_width)) = actual_slots {
                solve_simd_slot_values(&dtype, &width, Some(actual_dtype), Some(actual_width), out);
            }
        }
        // A spread of a pack that is still a parameter (`Tuple[*Self.Ts]`
        // against `Tuple[Int, Bool]`, the fieldwise constructor of a variadic
        // struct) binds the pack to the actual's whole element list, whether
        // the actual spells it element by element or as one bound pack. A
        // pack forwarded from another still-open pack solves nothing here.
        (TyKind::Struct(pn, pargs), TyKind::Struct(an, aargs))
            if let Some(TyKind::Param { binder, .. }) =
                (mojito_types::types::pack_spread_argument(pargs)).map(Ty::kind)
                && mojito_types::types::pack_spread_argument(aargs).is_none()
                && (pn == an
                    || (pn == mojito_types::types::TUPLE_TYPE_NAME
                        && mojito_types::types::tuple_elements(actual).is_some())) =>
        {
            let element_types =
                |types: Vec<Ty>| CtValue::Tuple(types.into_iter().map(CtValue::Type).collect());
            // A public Tuple actual may already be spelled by its concrete
            // specialization symbol (a later discovery round); its retained
            // element types are the list either way.
            let elements = match aargs.as_slice() {
                [TyArg::Val(value @ CtValue::Tuple(_))] => Some(value.clone()),
                _ if pn == mojito_types::types::TUPLE_TYPE_NAME => {
                    mojito_types::types::tuple_elements(actual)
                        .map(|types| element_types(types.into_iter().cloned().collect()))
                }
                _ => aargs
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => Some(ty.clone()),
                        TyArg::Val(_) | TyArg::Origin(_) => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(element_types),
            };
            if let Some(elements) = elements {
                out.entry(binder.name.trim_start_matches('*').to_string())
                    .or_insert(elements);
            }
        }
        (TyKind::Struct(pn, pargs), TyKind::Struct(an, aargs))
            if pn == an && pargs.len() == aargs.len() =>
        {
            for (p, a) in pargs.iter().zip(aargs) {
                match (p, a) {
                    // A direct reference binds. Any other residual (`n + 1`
                    // against `4`) is an equation this solver leaves alone:
                    // the substituted signature compares canonically later.
                    (TyArg::Val(CtValue::Expr(expr)), TyArg::Val(value)) => {
                        if let Some(reference) = expr.as_decl_ref() {
                            out.entry(reference.name.to_string())
                                .or_insert_with(|| value.clone());
                        } else if let CtValue::Expr(actual) = value {
                            solve_applied_args(expr, actual, out);
                        }
                    }
                    (TyArg::Ty(p), TyArg::Ty(a)) => solve_value_args(p, a, out),
                    _ => {}
                }
            }
        }
        // A pointer's address space is a value argument of its own: a
        // binder slot (`Pointer[T, _, address_space=a]`) binds to the
        // actual's space, and the pointees solve as types do.
        (
            TyKind::Pointer {
                element: pattern_element,
                address_space: pattern_space,
                ..
            }
            | TyKind::KgenPointer {
                element: pattern_element,
                address_space: pattern_space,
            }
            | TyKind::LitRef {
                element: pattern_element,
                address_space: pattern_space,
                ..
            },
            TyKind::Pointer {
                element,
                address_space,
                ..
            }
            | TyKind::KgenPointer {
                element,
                address_space,
            }
            | TyKind::LitRef {
                element,
                address_space,
                ..
            },
        ) if std::mem::discriminant(pattern.kind()) == std::mem::discriminant(actual.kind()) => {
            if let Some(reference) = pattern_space.expr().and_then(ParamExpr::as_decl_ref) {
                out.entry(reference.name.to_string())
                    .or_insert_with(|| address_space.value());
            }
            solve_value_args(pattern_element, element, out);
        }
        (TyKind::Ref(pattern_reference), TyKind::Ref(actual_reference)) => {
            if let Some(reference) = pattern_reference
                .address_space
                .expr()
                .and_then(ParamExpr::as_decl_ref)
            {
                out.entry(reference.name.to_string())
                    .or_insert_with(|| actual_reference.address_space.value());
            }
            solve_value_args(&pattern_reference.referent, &actual_reference.referent, out);
        }
        (TyKind::Ref(pattern_reference), _) => {
            solve_value_args(&pattern_reference.referent, actual, out);
        }
        _ => {}
    }
}

/// Replace every `TyKind::SelfType` in `ty` with `replacement` (the conforming
/// struct type, or a bounded `T`). Recurses into struct/function types.
pub(super) fn substitute_self(ty: &Ty, replacement: &Ty) -> Ty {
    match ty.kind() {
        TyKind::SelfType => replacement.clone(),
        TyKind::Param {
            binder,
            bounds,
            callable_bound,
        } => TyKind::Param {
            binder: binder.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| substitute_self(bound, replacement)),
        }
        .into(),
        struct_ty if let Some((name, args)) = struct_ty.as_non_simd_struct() => {
            ty.reusing(TyKind::Struct(
                name.clone(),
                map_tyargs(args, |t| substitute_self(t, replacement)),
            ))
        }
        TyKind::Dependent(dependent) => dependent.map_types(|ty| substitute_self(ty, replacement)),
        TyKind::ComptimeList(elem) => {
            TyKind::ComptimeList(substitute_self(elem, replacement)).into()
        }
        TyKind::ParamList(elem) => TyKind::ParamList(substitute_self(elem, replacement)).into(),
        TyKind::Tuple(elems) => TyKind::Tuple(
            elems
                .iter()
                .map(|t| substitute_self(t, replacement))
                .collect(),
        )
        .into(),
        TyKind::RuntimePack(elems) => TyKind::RuntimePack(
            elems
                .iter()
                .map(|t| substitute_self(t, replacement))
                .collect(),
        )
        .into(),
        TyKind::VariadicPack(element) => {
            TyKind::VariadicPack(substitute_self(element, replacement)).into()
        }
        TyKind::Variant(alternatives) => TyKind::Variant(
            alternatives
                .iter()
                .map(|ty| substitute_self(ty, replacement))
                .collect(),
        )
        .into(),
        TyKind::Pointer {
            element,
            origin,
            address_space,
        } => Ty::pointer(
            substitute_self(element, replacement),
            origin.clone(),
            address_space.clone(),
        ),
        TyKind::KgenPointer {
            element,
            address_space,
        } => Ty::kgen_pointer(substitute_self(element, replacement), address_space.clone()),
        TyKind::LitRef {
            element,
            origin,
            address_space,
        } => TyKind::LitRef {
            element: substitute_self(element, replacement),
            origin: origin.clone(),
            address_space: address_space.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements
                .iter()
                .map(|element| substitute_self(element, replacement))
                .collect(),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Assoc { base, name, args } => TyKind::Assoc {
            base: substitute_self(base, replacement),
            name: name.clone(),
            args: map_tyargs(args, |t| substitute_self(t, replacement)),
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
                params: params
                    .iter()
                    .map(|p| substitute_self(p, replacement))
                    .collect(),
                names: names.clone(),
                ret: substitute_self(ret, replacement),
                required: required.clone(),
                variadic: variadic.as_ref().map(|v| substitute_self(v, replacement)),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|v| substitute_self(v, replacement)),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| substitute_self(error, replacement)),
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
                .map(|candidate| substitute_self(candidate, replacement))
                .collect(),
        )
        .into(),
        _ => ty.clone(),
    }
}

/// What a function value reads: a binding in scope (`twice`), or a static
/// method reached through its struct (`S.st`, `P[Int].st`), upstream's
/// overload set for that attribute of the struct type, read at the
/// instance's `receiver` arguments (none for a non-parametric struct).
#[derive(Clone, Copy)]
pub(super) enum CallableValueSource<'n> {
    Binding(&'n str),
    Static {
        owner: &'n str,
        method: &'n str,
        receiver: &'n [TyArg],
    },
}

/// The bindings concrete resolution of a parameterized associated type
/// substitutes into its symbolic template: type parameters by name (the
/// enclosing struct's and the member's), value parameters by binder identity
/// (`ids`) or else by name, and origin parameters by `OriginParamId`.
pub(super) struct AssocBindings {
    pub types: TySubst,
    pub values: HashMap<String, CtValue>,
    pub ids: mojito_types::param_expr::ParamBindings,
    pub origins: HashMap<u32, mojito_types::origin::Origin>,
}

/// Substitute a parameterized associated type's template with concrete
/// arguments. Origin parameters (`Origin::Param`) are replaced first, while
/// the template names only its own: an origin id is a slot number, so a type
/// argument's binder of the same number (an enclosing clone's) must not be
/// reached. Types are then substituted with the ordinary type substitution,
/// and a last pass replaces symbolic value parameters (`CtValue::Expr`),
/// which the type-only pass carries through.
pub(super) fn substitute_assoc(ty: &Ty, bindings: &AssocBindings) -> Ty {
    let bound = mojito_types::types::bind_origin_params(ty, &bindings.origins);
    let mut values = bindings.ids.clone();
    values.bind_named_values(&ParamContext::detached(), &bindings.values);
    substitute_values(&substitute(&bound, &bindings.types), &values)
}

/// `ty` with every struct origin tail naming an explicitly bound origin
/// binder rebound to that origin, and every pointer nested in it too, a
/// parametric binder's (`Origin[mut=mut]`) at the mutability
/// `binder_mutability` gives the bound origin, as upstream infers the
/// binder's `mut` with the origin its bracket binds.
pub(super) fn bind_struct_origins(
    ty: &Ty,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
    binder_mutability: super::origins::BinderMutability<'_>,
) -> Ty {
    super::origins::bind_pointer_origins(
        &super::origins::substitute_struct_origins(ty, tails),
        tails,
        Some(binder_mutability),
    )
}

fn substitute_values(ty: &Ty, bindings: &mojito_types::param_expr::ParamBindings) -> Ty {
    let recur = |t: &Ty| substitute_values(t, bindings);
    let context = ParamContext::detached();
    let space = |slot: &mojito_types::types::AddressSpaceSlot| {
        slot.map_or_keep(|space| context.replace(space, bindings))
    };
    let map_args = |args: &[TyArg]| -> Vec<TyArg> {
        args.iter()
            .map(|argument| match argument {
                TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                TyArg::Val(value) => TyArg::Val(
                    mojito_types::types::replace_value_parameters(&context, value, bindings)
                        .unwrap_or_else(|_| value.clone()),
                ),
                TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
            })
            .collect()
    };
    match ty.kind() {
        struct_ty if let Some((name, args)) = struct_ty.as_non_simd_struct() => {
            TyKind::Struct(name.clone(), map_args(args)).into()
        }
        TyKind::Assoc { base, name, args } => TyKind::Assoc {
            base: recur(base),
            name: name.clone(),
            args: map_args(args),
        }
        .into(),
        TyKind::Pointer {
            element,
            origin,
            address_space,
        } => Ty::pointer(recur(element), origin.clone(), space(address_space)),
        TyKind::KgenPointer {
            element,
            address_space,
        } => Ty::kgen_pointer(recur(element), space(address_space)),
        TyKind::LitRef {
            element,
            origin,
            address_space,
        } => TyKind::LitRef {
            element: recur(element),
            origin: origin.clone(),
            address_space: space(address_space),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements.iter().map(recur).collect(),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recur(&reference.referent);
            reference.address_space = space(&reference.address_space);
            TyKind::Ref(reference).into()
        }
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(recur).collect()).into()
        }
        TyKind::VariadicPack(element) => TyKind::VariadicPack(recur(element)).into(),
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(recur).collect()).into()
        }
        TyKind::ComptimeList(element) => TyKind::ComptimeList(recur(element)).into(),
        TyKind::ParamList(element) => TyKind::ParamList(recur(element)).into(),
        // A dependent type keeps what its bound references closed (`Ts[idx]`
        // at `idx = 0` is `Ts[0]` while `Ts` stays open).
        TyKind::Dependent(_) => {
            let resolved = mojito_types::types::replace_parameters(&context, ty, bindings, 0)
                .unwrap_or_else(|_| ty.clone());
            match resolved.kind() {
                TyKind::Dependent(dependent) => dependent.map_types(recur),
                _ => recur(&resolved),
            }
        }
        // A raw origin's symbolic mutability reads the bound `Bool`
        // (`_lit_origin_type_of_mut[m]` is `!lit.origin<m>`).
        TyKind::LitOrigin(mutability) => TyKind::LitOrigin(
            mutability
                .try_map(|mutability| context.replace(mutability, bindings))
                .unwrap_or_else(|_| mutability.clone()),
        )
        .into(),
        // A callable contract closes over the parameters its signature
        // names, its own binders excepted (`def(Scalar[Self.dtype])`).
        TyKind::Func(_) | TyKind::GenericFunc(_) => {
            mojito_types::types::replace_parameters(&context, ty, bindings, 0)
                .unwrap_or_else(|_| ty.clone())
        }
        // A symbolic lane dtype or width closes the same way and
        // re-canonicalizes (`Scalar[Self.dt]` at `dt = DType.int` is `Int`).
        // A symbolic literal value closes the same way.
        kind if (kind.is_simd()
            || kind.string_literal_value().is_some()
            || kind.is_numeric_literal()
            || matches!(kind, TyKind::KgenSimd { .. } | TyKind::PopArray { .. }))
            && mojito_types::types::is_symbolic(ty) =>
        {
            mojito_types::types::replace_parameters(&context, ty, bindings, 0)
                .unwrap_or_else(|_| ty.clone())
        }
        _ => ty.clone(),
    }
}

/// The struct's type pack lists a receiver binds to a caller's pack still
/// open (`VariadicPack[..., *Ts]`), each bound to that pack's own list, so a
/// member type computed from the list (`variadic_ptr_map` of `Self.Ts`)
/// names the caller's pack.
fn forwarded_pack_lists(
    decls: &[ParamDecl],
    targs: &[TyArg],
) -> mojito_types::param_expr::ParamBindings {
    let context = ParamContext::detached();
    let mut bindings = mojito_types::param_expr::ParamBindings::new();
    for (decl, argument) in ParamDecl::aligned_pairs(decls, targs) {
        if let (ParamDecl::Type { variadic: true, .. }, TyArg::Ty(spread)) = (decl, argument)
            && let Some(TyKind::Param { binder, .. }) =
                mojito_types::types::pack_spread(std::slice::from_ref(spread)).map(Ty::kind)
        {
            bindings.bind(
                decl.id().clone(),
                context.decl_ref(
                    binder.id.clone(),
                    &binder.name,
                    mojito_types::param_expr::MetaTy::type_list(),
                ),
            );
        }
    }
    bindings
}

/// Solve value parameters from the arguments of one function's application
/// (`h(n)` against `h(2)` solves `n = 2`, `G[n].f()` against `G[2].f()` the
/// same through the instance), as the pin unifies a call in the parameter
/// domain argument by argument. A struct value a builtin `__init__` folds
/// to is matched by the construction it folded from, as the pin infers
/// through the call and not through the fields. Two canonical operator
/// nodes unify the same way, node by node (`w * 2` against `n * 2` solves
/// `w = n`); the solver never inverts an operation, so `w * 2` against `4`
/// solves nothing. First solution wins; any other pair of expressions
/// contributes nothing.
fn solve_applied_args(pattern: &ParamExpr, actual: &ParamExpr, out: &mut HashMap<String, CtValue>) {
    let construction = |expr: &ParamExpr| match expr.kind() {
        ParamKind::StructValue { spelling, .. } => spelling.0.clone(),
        _ => expr.clone(),
    };
    let (pattern, actual) = (construction(pattern), construction(actual));
    if matches!(
        (pattern.kind(), actual.kind()),
        (ParamKind::Op { .. }, ParamKind::Op { .. })
    ) {
        let mut solved = HashMap::new();
        if unify_operator_pattern(&pattern, &actual, &mut solved) {
            for (name, value) in solved {
                out.entry(name).or_insert(value);
            }
        }
        return;
    }
    let (
        ParamKind::Apply { function, args, .. },
        ParamKind::Apply {
            function: applied,
            args: actuals,
            ..
        },
    ) = (pattern.kind(), actual.kind())
    else {
        return;
    };
    if function != applied || args.len() != actuals.len() {
        return;
    }
    for (arg, actual) in args.iter().zip(actuals) {
        match (arg.kind(), actual.kind()) {
            (ParamKind::DeclRef(reference), _) => {
                out.entry(reference.name.to_string()).or_insert_with(|| {
                    actual
                        .as_constant()
                        .cloned()
                        .unwrap_or_else(|| CtValue::Expr(actual.clone()))
                });
            }
            (
                ParamKind::TypeShape(pattern),
                ParamKind::TypeShape(actual) | ParamKind::Constant(CtValue::Type(actual)),
            ) => solve_value_args(pattern, actual, out),
            _ => solve_applied_args(arg, actual, out),
        }
    }
}

/// Unify a canonical operator pattern with an actual node by node: the same
/// operator and arity, a binder operand binding its actual (consistently,
/// when it occurs twice), a nested operator unifying recursively, and every
/// other operand equal to its actual. A mismatch anywhere fails the whole
/// pattern, which then binds nothing.
fn unify_operator_pattern(
    pattern: &ParamExpr,
    actual: &ParamExpr,
    solved: &mut HashMap<String, CtValue>,
) -> bool {
    if let Some(reference) = pattern.as_decl_ref() {
        let value = actual
            .as_constant()
            .cloned()
            .unwrap_or_else(|| CtValue::Expr(actual.clone()));
        return match solved.entry(reference.name.to_string()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(value);
                true
            }
            std::collections::hash_map::Entry::Occupied(slot) => *slot.get() == value,
        };
    }
    match (pattern.kind(), actual.kind()) {
        (
            ParamKind::Op { op, operands },
            ParamKind::Op {
                op: actual_op,
                operands: actuals,
            },
        ) => {
            op == actual_op
                && operands.len() == actuals.len()
                && operands
                    .iter()
                    .zip(actuals)
                    .all(|(operand, actual)| unify_operator_pattern(operand, actual, solved))
        }
        _ => pattern == actual,
    }
}

/// Solve a SIMD pattern's `dtype` and `length` binders from the actual's
/// slots; a slot given as `None` binds nothing.
fn solve_simd_slot_values(
    dtype: &SimdDtype,
    width: &SimdWidth,
    actual_dtype: Option<SimdDtype>,
    actual_width: Option<SimdWidth>,
    out: &mut HashMap<String, CtValue>,
) {
    if let SimdDtype::Expr(lane) = dtype
        && let Some(reference) = lane.as_decl_ref()
        && let Some(actual_dtype) = actual_dtype
    {
        let value = match actual_dtype {
            SimdDtype::Known(dtype) => mojito_types::types::dtype_value(dtype),
            SimdDtype::Expr(expr) => expr.require_constant().unwrap_or(CtValue::Expr(expr)),
        };
        out.entry(reference.name.to_string()).or_insert(value);
    }
    let (SimdWidth::Expr(length), Some(actual_width)) = (width, actual_width) else {
        return;
    };
    if let Some(reference) = length.as_decl_ref() {
        let value = match actual_width {
            SimdWidth::Known(count) => mojito_types::types::simd_length_value(count),
            SimdWidth::Expr(actual) => actual
                .as_constant()
                .cloned()
                .unwrap_or(CtValue::Expr(actual)),
        };
        out.entry(reference.name.to_string()).or_insert(value);
    } else if let SimdWidth::Expr(actual) = &actual_width {
        solve_applied_args(length, actual, out);
    }
}

/// Callable specialization and method-generic instantiation moved from `checker.rs`.
/// The mutability a reference through the origin binder `decl`
/// ([`ParamDecl::Origin`], numbered `id`) carries: its fixed `mut`, else
/// the binder's own symbolic mutability.
pub(super) const fn origin_binder_mutability(
    decl: &ParamDecl,
    id: mojito_types::origin::OriginParamId,
) -> mojito_types::origin::Mutability {
    use mojito_types::origin::Mutability;
    match decl {
        ParamDecl::Origin {
            mutability: OriginMutability::Fixed(true),
            ..
        } => Mutability::Mutable,
        ParamDecl::Origin {
            mutability: OriginMutability::Fixed(false),
            ..
        } => Mutability::Immutable,
        _ => Mutability::Param(id),
    }
}

impl Checker {
    /// The origin binder `name` names in the declared list `params`,
    /// classified by its annotation's resolved type as the check classifies
    /// the list ([`Self::origin_binder_at`]): its [`ParamDecl::Origin`] and
    /// the [`OriginParamId`](mojito_types::origin::OriginParamId) its
    /// position in `params` is. An `OriginSet` binder is one too.
    pub(super) fn origin_binder(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        name: &str,
    ) -> Option<(ParamDecl, mojito_types::origin::OriginParamId)> {
        params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.name == name)
            .find_map(|(index, _)| {
                self.origin_binder_at(params, index).map(|decl| {
                    (
                        decl,
                        mojito_types::origin::OriginParamId(
                            u32::try_from(index).unwrap_or(u32::MAX),
                        ),
                    )
                })
            })
    }

    /// The [`ParamDecl::Origin`] `params[index]` declares, `None` for any
    /// other binder: its annotation resolves with its siblings in scope, so
    /// an `Origin[mut=m]` names its list's `m` wherever the list is read.
    pub(super) fn origin_binder_at(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        index: usize,
    ) -> Option<ParamDecl> {
        let param = params.get(index)?;
        self.push_param_scope(&[]);
        let unresolved: Vec<&str> = params
            .iter()
            .filter(|sibling| sibling.name != param.name && !sibling.name.starts_with('*'))
            .map(|sibling| sibling.name.as_str())
            .filter(|name| self.value_parameter_in_scope(name).is_none())
            .collect();
        if let Some(scope) = self.param_scopes.borrow_mut().values.last_mut() {
            for name in unresolved {
                scope.insert(
                    name.to_string(),
                    ParamContext::detached().decl_ref(
                        ParamRef::unbound(name).id,
                        name,
                        MetaTy::Value(Ty::bool()),
                    ),
                );
            }
        }
        let decl = self
            .origin_binder_decl(param, index, ParamId::new("$origin_binder", index))
            .ok()
            .flatten();
        self.pop_param_scope();
        // Leave no dead value level behind: a caller writing into the
        // innermost value scope writes into its own.
        let mut scopes = self.param_scopes.borrow_mut();
        let depth = scopes.types.len();
        scopes.values.truncate(depth);
        scopes.packs.truncate(depth);
        decl
    }

    /// Whether `params[index]` is origin metadata, which instances and calls
    /// infer from places rather than bind by position: an origin binder
    /// ([`Self::origin_binder_at`]), or the value binder a sibling origin
    /// binder names as its mutability (`m: Bool, //, o: Origin[mut=m]`), as
    /// [`ParamDecl::is_argument_binder`] reads a classified list.
    pub(super) fn is_origin_metadata(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        index: usize,
    ) -> bool {
        let Some(param) = params.get(index) else {
            return false;
        };
        self.origin_binder_at(params, index).is_some()
            || (0..params.len()).filter(|peer| *peer != index).any(|peer| {
                matches!(
                    self.origin_binder_at(params, peer),
                    Some(ParamDecl::Origin {
                        mutability: OriginMutability::Binder(binder),
                        ..
                    }) if *binder.name == *param.name
                )
            })
    }

    /// Whether any binder of `params` is origin metadata
    /// ([`Self::is_origin_metadata`]).
    pub(super) fn declares_origin_metadata(&self, params: &[mojito_ast::ast::TypeParam]) -> bool {
        (0..params.len()).any(|index| self.is_origin_metadata(params, index))
    }

    /// [`substitute_at`] for a member of the instance `targs` names (a
    /// receiver's type, or a static call's explicit `View[o]`): the
    /// instance's origin tail binds the struct's origin binders in pointer
    /// provenances too, so `get` declared `-> Pointer[T, Self.o]` on a
    /// `View[origin_of(x)]` names `origin_of(x)`, as upstream substitutes
    /// every parameter of the instance type into the member's signature. A
    /// slot holding an `ImmOrigin(...)` cast binds immutably, as the cast's
    /// `mut=False` does upstream.
    pub(super) fn substitute_at_instance(&self, ty: &Ty, info: &StructInfo, targs: &[TyArg]) -> Ty {
        substitute_at_with(
            ty,
            info,
            targs,
            Some(&|_, origin| self.bound_origin_mutability(origin)),
        )
    }

    /// The mutability a parametric origin binder (`Origin[mut=mut]`) takes
    /// when bound to `origin`: an `ImmOrigin(...)` cast's `False`, a
    /// place's owner's, a caller's own binder's declared one (itself, when
    /// that is parametric too), or its pack origin's.
    pub(super) fn bound_origin_mutability(
        &self,
        origin: &mojito_types::origin::Origin,
    ) -> Option<mojito_types::origin::Mutability> {
        use mojito_types::origin::{Mutability, Origin};
        match origin {
            Origin::MutCast(..) => origin.static_mutability().map(Mutability::of),
            Origin::Place(place) => Some(self.owner_reference_mutability(place.root)),
            // A collector's pack origin is `Origin[mut=elt_is_mutable]`, which
            // a `read` collector, the only `VariadicPack` one, binds `False`.
            Origin::Param(id) if id.is_pack_binder() => Some(Mutability::Immutable),
            Origin::Param(id) => self
                .origin_binder_at(&self.enclosing_type_params, id.0 as usize)
                .filter(|decl| !decl.is_origin_set())
                .map(|decl| origin_binder_mutability(&decl, *id)),
            _ => None,
        }
    }

    /// `ty` with each origin argument cast to the mutability its base
    /// already has folded to the base ([`Self::fold_origin_mut_cast`]), as
    /// upstream's cast of an `Origin[mut=m]` value to `m` is that value: a
    /// struct type converts between the two slots only then.
    pub(super) fn fold_origin_mut_casts(&self, ty: &Ty) -> Ty {
        mojito_types::types::map_origins(ty, &|origin| self.fold_origin_mut_cast(origin))
    }

    /// The origin and `mut=` arguments among `bound`, a bracket's arguments
    /// bound against `decls`, the whole declared list of `name`, as
    /// upstream's binder binds them: each in its binder's declared slot,
    /// read back from `explicit` when it resolved them already. Each origin
    /// argument converts to its slot's `Origin[mut=…]` type, and each `mut=`
    /// argument is a `Bool` its origin's capability allows. A `use_` of
    /// [`BracketUse::Type`] keeps an origin's casts in the type it names,
    /// and binds a signature's origin naming places only the body resolves
    /// unbound. An `OriginSet` is inferred, never supplied.
    pub(super) fn bound_bracket_origins(
        &self,
        name: &str,
        decls: &[ParamDecl],
        bound: &[Vec<(usize, &mojito_ast::ast::ParamArg)>],
        (explicit, use_): (&ExplicitTypes, BracketUse),
    ) -> Result<BracketOrigins, TypeError> {
        let mut origins = Vec::new();
        let mut mutabilities = Vec::new();
        for (index, (decl, arguments)) in decls.iter().zip(bound).enumerate() {
            if decl.is_argument_binder(decls) {
                continue;
            }
            // A `_` leaves its binder to the call's arguments.
            let arguments = arguments
                .iter()
                .filter(|(_, argument)| mojito_ast::call::unbound_marker(argument).is_none());
            for &(position, argument) in arguments {
                if decl.is_origin_mutability_binder(decls) {
                    mutabilities.push((index, decl, position, argument));
                    continue;
                }
                let ParamDecl::Origin {
                    origin: id,
                    mutability,
                    ..
                } = decl
                else {
                    continue;
                };
                if *mutability == mojito_types::types::OriginMutability::Set {
                    return Err(TypeError::Unsupported(format!(
                        "semantic parameter '{}' is inferred and cannot be supplied explicitly",
                        decl.name()
                    )));
                }
                if let Some(resolved) = explicit.origin(index, position) {
                    origins.push(resolved.clone());
                    continue;
                }
                origins.push(self.bound_bracket_origin(
                    (name, decls),
                    (index, decl, *id),
                    (position, argument),
                    use_,
                )?);
            }
        }
        let mutabilities = mutabilities
            .into_iter()
            .map(|(index, decl, position, argument)| {
                if let Some(resolved) = explicit.mutability(index, position) {
                    return Ok(resolved.clone());
                }
                self.bound_bracket_mutability(
                    name,
                    decls,
                    (index, decl),
                    (position, argument),
                    (&mut origins, use_),
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(BracketOrigins {
            origins,
            mutabilities,
        })
    }

    /// `callable` with the origins its bracket bound substituted into its
    /// signature, as upstream substitutes a bound declared list: each
    /// parameter, collector, and result type naming a bound origin binder
    /// (`Span[Int, o]`, `Pointer[Pointer[Int, o], …]`), and each `ref`
    /// contract over one.
    pub(super) fn bind_bracket_origins(&self, callable: Ty, origins: &[BoundOrigin]) -> Ty {
        if origins.is_empty() || callable.callable().is_none() {
            return callable;
        }
        let tails = origins
            .iter()
            .map(|bound| (bound.id, bound.origin.clone()))
            .collect();
        bind_callable_origins(
            bind_callable_ref_origins(callable, &tails),
            &tails,
            &|_, origin| self.bound_origin_mutability(origin),
        )
    }

    /// Materialize the monomorphic checked view of an explicitly specialized
    /// generic function value: its bracket `arguments` bound against its
    /// whole declared list, and what they bound substituted into its
    /// signature. Generic execution remains type-erased; only its callable
    /// contract is instantiated here.
    pub(super) fn instantiate_generic_callable_value(
        &self,
        name: &str,
        callable: Ty,
        arguments: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        self.bind_generic_callable_value(name, callable, arguments)
            .map(|bound| (bound.contract, bound.view))
    }

    /// [`Self::instantiate_generic_callable_value`] with the bracket's
    /// arguments over the callee's whole declared list too, and the origin
    /// binders it left open, which the contract still names.
    fn bind_generic_callable_value(
        &self,
        name: &str,
        callable: Ty,
        arguments: &[mojito_ast::ast::ParamArg],
    ) -> Result<BoundCallableValue, TypeError> {
        let TyKind::GenericFunc(generic) = callable.kind() else {
            return Ok(BoundCallableValue {
                contract: callable,
                view: Vec::new(),
                declared: Vec::new(),
                open: Vec::new(),
            });
        };
        let BoundParameters {
            subst,
            arguments: tyargs,
            origins,
            ..
        } = self.resolve_use_params(name, &generic.decls, arguments, &[], &[])?;
        let open = generic
            .decls
            .iter()
            .filter(|decl| {
                matches!(decl, ParamDecl::Origin { origin, .. }
                    if !origins.iter().any(|bound| bound.id == *origin))
            })
            .cloned()
            .collect();
        let declared = self.declared_symbol_arguments(&generic.decls, &tyargs, &origins);
        let bound = self.bind_bracket_origins(callable.clone(), &origins);
        let Some(GenericFuncTy { decls, func }) = bound.as_generic_func() else {
            return Ok(BoundCallableValue {
                contract: bound,
                view: tyargs,
                declared,
                open,
            });
        };
        let decls = ParamDecl::argument_view(decls);
        let values = DependentValues::own(&self.param_context, &decls, &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = expand_solved_packs(ty, &decls, &tyargs);
            let substituted = self.resolve_assoc_ty(&substitute(&expanded, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let contract = TyKind::Func(Box::new(func.try_map_types(resolve)?));
        Ok(BoundCallableValue {
            contract: contract.into(),
            view: tyargs,
            declared,
            open,
        })
    }

    /// A function value's bracket arguments `view` over the argument view
    /// of `decls`, its callee's whole declared list, as that whole list, as
    /// upstream's symbol reference binds a generator's declared list: each
    /// origin binder holds the origin the bracket bound (`origins`) or
    /// stays unbound for each call to infer, and each `mut=` binder the
    /// mutability its origin decides.
    fn declared_symbol_arguments(
        &self,
        decls: &[ParamDecl],
        view: &[TyArg],
        origins: &[BoundOrigin],
    ) -> Vec<TyArg> {
        if !decls.iter().any(ParamDecl::is_origin) {
            return view.to_vec();
        }
        let placed = decls.iter().filter_map(|decl| match decl {
            ParamDecl::Origin { origin: id, .. } => Some(
                origins
                    .iter()
                    .find(|bound| bound.id == *id)
                    .map_or(mojito_types::origin::Origin::Unbound, |bound| {
                        bound.origin.clone()
                    }),
            ),
            _ => None,
        });
        self.place_struct_origins(decls, view.iter().cloned(), placed, &[])
    }

    pub(super) fn specialize_callable_value_candidate(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        callable: Ty,
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        // The pinned compiler accepts explicit Origin specialization of a
        // stateless nested function as a value but rejects materializing one
        // that has a capture environment.
        if !arguments.is_empty()
            && (super::is_closure_storage(&callable)
                || callable
                    .callable()
                    .is_some_and(|FuncTy { environment, .. }| environment.captures().is_some()))
        {
            return Err(TypeError::Unsupported(format!(
                "cannot materialize an explicit Origin specialization of '{name}': it has a \
                 capture environment; call it directly or use a capture-free function"
            )));
        }
        match callable.kind() {
            // A bare reference or an empty bracket binds none of the
            // declared parameters, defaulted ones included, so the function
            // stays parametric; only inferred origins are left to bind.
            TyKind::GenericFunc(generic)
                if arguments.is_empty() && ParamDecl::first_unbound(&generic.decls).is_some() =>
            {
                Ok((callable, Vec::new()))
            }
            // A bracket leaving origin binders open (`first[_, 6]`) binds a
            // generator over them, as upstream's partial binding is: each
            // call of the value infers them.
            TyKind::GenericFunc(_) => {
                let BoundCallableValue {
                    contract,
                    declared: arguments,
                    open,
                    ..
                } = self.bind_generic_callable_value(name, callable, arguments)?;
                let contract = match contract.into_kind() {
                    TyKind::Func(func) if !open.is_empty() => {
                        TyKind::GenericFunc(Box::new(GenericFuncTy {
                            decls: open,
                            func: *func,
                        }))
                    }
                    other => other,
                };
                Ok((contract.into(), arguments))
            }
            TyKind::Func(_) if arguments.is_empty() => Ok((callable, Vec::new())),
            TyKind::Func(_) => Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 0,
                got: arguments.len(),
            }),
            other => Err(TypeError::NotCallable {
                name: name.to_string(),
                ty: other.to_string(),
            }),
        }
    }

    /// The function a bracket after a name binds and the operands it binds,
    /// when the name denotes a declared function or a compile-time callable:
    /// upstream's `SubscriptNode` over an overload set or a callable
    /// parameter, whose operands are parameter bindings (`twice[4]`,
    /// `pair[a=3, b=4]`, `total[]`). `None` for any other base, a runtime
    /// local holding a function value included: upstream subscripts that.
    pub(super) fn bracket_callable(
        &self,
        expr: &Expr,
    ) -> Option<(String, Vec<mojito_ast::ast::ParamArg>)> {
        let (name, args) = expr.bracket_application()?;
        self.binds_bracket_parameters(name)
            .then(|| (name.to_string(), args.into_owned()))
    }

    /// The name a bracket applies and the operands it spells, when the
    /// bracket is a parameter application rather than a runtime subscript:
    /// upstream's classification of a `SubscriptNode`'s base. A base with no
    /// value binding (a type, alias, trait, or builtin head) or a callable
    /// [`Self::bracket_callable`] binds is applied; a runtime value is
    /// subscripted.
    pub(super) fn parameter_application<'e>(
        &self,
        expr: &'e Expr,
    ) -> Option<(&'e str, std::borrow::Cow<'e, [mojito_ast::ast::ParamArg]>)> {
        let (name, args) = expr.bracket_application()?;
        (self.lookup(name).is_none() || self.binds_bracket_parameters(name)).then_some((name, args))
    }

    /// Type a bracket over a function, which binds its parameters, or
    /// `None` for any other base. A bracket over a function's name binds
    /// its parameters in place (`twice[4]`), and one over a static read
    /// through its struct binds the static's (`S.gst[10]`).
    pub(super) fn infer_bracket_callable(&self, expr: &Expr) -> Option<Result<Ty, TypeError>> {
        if let Some((name, args)) = self.bracket_callable(expr) {
            return Some(self.bind_bracket_callable(expr, &name, &args, None, true));
        }
        self.static_method_bracket(expr)
            .map(|bracket| self.infer_static_method_bracket(expr, &bracket))
    }

    /// Bind the bracket `expr` over the function `name` to `args` in place,
    /// as upstream binds a `SubscriptNode` over an overload set: the
    /// specialization `expected` selects, recorded at the bracket itself.
    pub(super) fn bind_bracket_callable(
        &self,
        expr: &Expr,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
        expected: Option<&Ty>,
        record: bool,
    ) -> Result<Ty, TypeError> {
        self.infer_specialized_callable_value(
            expr.source_span(),
            CallableValueSource::Binding(name),
            args,
            expected,
            record,
        )?
        .ok_or_else(|| super::inference::type_read_as_value(name))
    }

    /// Whether `value` reads a function its bracket specialized
    /// (`borrow[origin_of(v)]`, `twice[4]`) as a value.
    pub(super) fn specializes_callable_value(&self, value: &Expr) -> bool {
        self.bracket_callable(value).is_some()
            && self.has_fact(&value.source_span(), |facts| {
                facts.overload_target.is_some()
            })
    }

    pub(super) fn infer_specialized_callable_value(
        &self,
        span: SourceSpan,
        source: CallableValueSource<'_>,
        arguments: &[mojito_ast::ast::ParamArg],
        expected: Option<&Ty>,
        record: bool,
    ) -> Result<Option<Ty>, TypeError> {
        let Some((selected, instance_arguments, target)) =
            self.specialized_callable_value(&span, source, arguments, expected, record)?
        else {
            return Ok(None);
        };
        if record {
            // A generic function's value names its instance at the
            // arguments the bracket bound, as upstream's symbol reference
            // names a specialization; MIR carries them to the elaborator.
            if !instance_arguments.is_empty() {
                self.record_at(span.clone()).generic_instantiation =
                    Some(mojito_checked::checked::GenericInstantiation {
                        callee: target.clone(),
                        parameter_names: Vec::new(),
                        parameter_types: Vec::new(),
                        variadic: None,
                        arguments: instance_arguments,
                    });
            }
            self.record_at(span.clone()).overload_target = Some(target);
            self.record_at(span).ty = Some(selected.clone());
        }
        Ok(Some(selected))
    }

    /// The function value `source` reads at the bracket `arguments`: its
    /// type, the instance arguments the bracket bound, and the symbol it
    /// lowers to, the candidate `expected` selects from an overload set.
    pub(super) fn specialized_callable_value(
        &self,
        span: &SourceSpan,
        source: CallableValueSource<'_>,
        arguments: &[mojito_ast::ast::ParamArg],
        expected: Option<&Ty>,
        record: bool,
    ) -> Result<Option<(Ty, Vec<TyArg>, String)>, TypeError> {
        let Some(CallableValueCandidates {
            name,
            callable,
            targets,
        }) = self.callable_value_candidates(span, source, record)?
        else {
            return Ok(None);
        };
        let name = name.as_str();
        let ((selected, instance_arguments), target) = if let TyKind::Overload(candidates) =
            callable.kind()
        {
            let expected = expected.ok_or_else(|| TypeError::BadCall {
                func: name.to_string(),
                reason: "an overloaded function value requires a contextual callable type"
                    .to_string(),
            })?;
            // Each candidate is specialized inside a probe; the one that
            // fits is specialized again below, which alone keeps its facts.
            let mut matches = candidates
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    self.probe(|| {
                        self.specialize_callable_value_candidate(name, arguments, candidate.clone())
                            .ok()
                            .filter(|(specialized, _)| self.value_coerces(specialized, expected))
                    })
                    .and_then(|_| targets.get(index).cloned().flatten())
                    .map(|target| (index, target))
                })
                .collect::<Vec<_>>();
            let (index, target) = match matches.len() {
                0 => {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: format!("specialization of overload({name})"),
                        context: "overloaded callable value".to_string(),
                    });
                }
                1 => matches.pop().expect("one callable-value candidate"),
                _ => {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: format!(
                            "multiple specialized overloads fit expected type '{expected}'"
                        ),
                    });
                }
            };
            let specialized = self.specialize_callable_value_candidate(
                name,
                arguments,
                candidates[index].clone(),
            )?;
            (specialized, target)
        } else {
            let specialized =
                self.specialize_callable_value_candidate(name, arguments, callable)?;
            let target = targets
                .into_iter()
                .next()
                .flatten()
                .unwrap_or_else(|| name.to_string());
            (specialized, target)
        };
        Ok(Some((selected, instance_arguments, target)))
    }

    /// Record a generic-struct application reached as a constructor target or
    /// method-call receiver.
    pub(super) fn record_struct_instantiation(
        &self,
        template: &str,
        arguments: &[TyArg],
        source: Option<&str>,
    ) {
        // Unstamped bundled code is checked for every program, and a
        // synthesized node has no source at all: neither is user-reachable
        // code, so an instance seen only there is not a fact of this program.
        if source.is_none() || super::overload_support::is_bundled_module_source(source) {
            return;
        }
        let Some(arguments) = self.instance_arguments(template, arguments) else {
            return;
        };
        let mut recorded = self.struct_instantiations.borrow_mut();
        if !recorded
            .iter()
            .any(|existing| existing.template == template && existing.arguments == arguments)
        {
            recorded.push(mojito_checked::checked::StructInstantiation {
                template: template.to_string(),
                arguments,
            });
        }
    }

    /// The materialized declaration-order arguments of a closed instance of
    /// an ordinary generic struct, which key a call's instantiation record,
    /// or `None` when the application keys none. Only a struct whose
    /// parameters are all plain type or value parameters qualifies: packs,
    /// callable-bounded parameters, and origin binders key none, as does a
    /// `StringLiteral` or an open value argument.
    pub(super) fn instance_arguments(
        &self,
        template: &str,
        arguments: &[TyArg],
    ) -> Option<Vec<TyArg>> {
        let info = self.structs.get(template)?;
        let bakeable = !info.decls().is_empty()
            && info.decls().iter().all(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: false,
                        callable_bound: None,
                        ..
                    } | ParamDecl::Value {
                        variadic: false,
                        ..
                    }
                )
            })
            && info.decls().len() == arguments.len()
            && !info
                .params
                .iter()
                .any(|decl| decl.is_origin() || decl.is_origin_mutability_binder(&info.params))
            && arguments.iter().all(|argument| match argument {
                TyArg::Ty(ty) => !mojito_types::types::contains_string_literal(ty),
                TyArg::Val(value) => mojito_symbol::symbol::specialization_value_is_closed(value),
                TyArg::Origin(_) => false,
            });
        bakeable.then(|| {
            arguments
                .iter()
                .map(materialized_instantiation_argument)
                .collect()
        })
    }

    /// The type an argument solves its parameter's binders from, before
    /// they are solved. A list, set, or dictionary display against a
    /// collection whose element (key, value) is one of `decls`' open binders
    /// takes its element type from its own elements, as an annotation hole
    /// (`List[_]`) does, so `f(["a"])` solves `K` in `List[K]`; `explicit`
    /// substitutes the binders the brackets already bound first. The solve
    /// is shallow, and anything else is inferred bare.
    pub(super) fn solving_argument_ty(
        &self,
        expression: &Expr,
        pattern: &Ty,
        decls: &[ParamDecl],
        explicit: &TySubst,
    ) -> Result<Ty, TypeError> {
        let holes = match &expression.kind {
            ExprKind::ListLit(values) if !values.is_empty() => 1,
            ExprKind::BraceLit(entries)
                if entries.first().is_some_and(|(_, value)| value.is_some()) =>
            {
                2
            }
            ExprKind::BraceLit(entries) if !entries.is_empty() => 1,
            _ => return self.infer(expression),
        };
        let pattern = substitute(pattern, explicit);
        let shaped = match holes {
            2 => dict_elements(&pattern).is_some(),
            _ => match &expression.kind {
                ExprKind::ListLit(_) => list_element(&pattern).is_some(),
                _ => set_element(&pattern).is_some(),
            },
        };
        let Some((name, arguments)) = pattern.kind().as_non_simd_struct() else {
            return self.infer(expression);
        };
        if !shaped {
            return self.infer(expression);
        }
        let open = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Param { binder, .. }
                if decls.iter().any(|decl| matches!(decl,
                    ParamDecl::Type { id, variadic: false, .. } if *id == binder.id)))
        };
        let holed = Ty::new(TyKind::Struct(
            name.clone(),
            arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| match argument {
                    TyArg::Ty(ty) if index < holes && open(ty) => TyArg::Ty(Ty::infer()),
                    argument => argument.clone(),
                })
                .collect(),
        ));
        if holed == pattern {
            return self.infer(expression);
        }
        // A display the hole rule cannot type is left to the solved
        // parameter's check, which reports it.
        self.infer_with_expected(expression, &holed, false)
            .or_else(|_| self.infer(expression))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn instantiate_method_generics(
        &self,
        name: &str,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<MethodInstantiation, TypeError> {
        // The bracket binds the method's whole declared list, as a free
        // call's does, and the origins it bound are substituted into the
        // signature; the call's one solve then binds the origin binders it
        // left open from the arguments' places, beside its type and value
        // binders.
        let explicit = self.explicit_type_args(name, &signature.declared, param_args)?;
        let bracket_origins: HashMap<_, _> = explicit
            .origins()
            .iter()
            .map(|bound| (bound.id, bound.origin.clone()))
            .collect();
        let binder_mutability =
            |_, origin: &mojito_types::origin::Origin| self.bound_origin_mutability(origin);
        let bind = |ty: &Ty| bind_struct_origins(ty, &bracket_origins, &binder_mutability);
        let bracket_params: Vec<Ty> = params.iter().map(bind).collect();
        let params = bracket_params.as_slice();
        let bracket_variadic = variadic.map(bind);
        let variadic = bracket_variadic.as_ref();
        let kw_variadic = kw_variadic.map(bind);
        let kw_variadic = kw_variadic.as_ref();
        if signature.declared.is_empty() {
            let BoundParameters { attributes, .. } = self.resolve_explicit_use_params(
                name,
                &signature.declared,
                (param_args, &explicit),
                &[],
                &[],
            )?;
            return Ok(MethodInstantiation {
                params: params.to_vec(),
                variadic: variadic.cloned(),
                kw_variadic: kw_variadic.cloned(),
                subst: HashMap::new(),
                arguments: Vec::new(),
                declared: Vec::new(),
                attributes,
                origin_bindings: None,
                bracket_origins,
            });
        }
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let forwarded_pack = self.forwarded_pack_argument(name, args, variadic.is_some())?;
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error(name))?;
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error(name))?;
        if let Some((position, _)) = &forwarded_pack {
            mojito_ast::call::bind_spread(&matched, *position)
                .map_err(|error| error.into_type_error(name))?;
        }
        let collector = || {
            signature
                .variadic_name
                .clone()
                .unwrap_or_else(|| "args".to_string())
        };
        let mut patterns = Vec::new();
        let mut pattern_names = Vec::new();
        let mut actuals = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            pattern_names.push(signature.names[index].clone());
            let actual = self.solving_argument_ty(
                expression,
                &params[index],
                &signature.decls(),
                &explicit.subst,
            )?;
            let actual = self.callable_bound_argument(
                name,
                &signature.names[index],
                expression,
                actual,
                &params[index],
            )?;
            let (pattern, actual) = self.reference_solve_pair(
                signature.ref_params.get(index).and_then(Option::as_ref),
                &params[index],
                &actual,
                expression,
            );
            patterns.push(pattern);
            actuals.push(actual);
        }
        if let Some(element) = variadic {
            for position in matched.positional_overflow {
                patterns.push(element.clone());
                pattern_names.push(collector());
                let actual = match &forwarded_pack {
                    Some((spread, forwarded)) if *spread == position => self.bind_forwarded_pack(
                        name,
                        &args[position],
                        forwarded,
                        element,
                        Some(matches!(
                            signature.variadic_convention,
                            Some(ArgConvention::Var)
                        )),
                    )?,
                    _ => self.solving_argument_ty(
                        &args[position],
                        element,
                        &signature.decls(),
                        &explicit.subst,
                    )?,
                };
                actuals.push(actual);
            }
        }
        if let Some(element) = kw_variadic {
            for position in matched.keyword_overflow {
                patterns.push(element.clone());
                pattern_names.push(kwargs[position].name.clone());
                actuals.push(self.infer(&kwargs[position].value)?);
            }
            if let Some(actual) = forwarded_element {
                patterns.push(element.clone());
                pattern_names.push("kwargs".to_string());
                actuals.push(actual);
            }
        }
        let super::call_inference::SolvedCall {
            bound:
                BoundParameters {
                    subst,
                    arguments: tyargs,
                    attributes,
                    ..
                },
            origins: origin_bindings,
        } = self.resolve_call_params(
            name,
            &signature.declared,
            (param_args, &explicit),
            (&pattern_names, &patterns),
            &actuals,
        )?;
        let bound = |ty: &Ty| {
            origin_bindings
                .as_ref()
                .map_or_else(|| ty.clone(), |origins| origins.substitute(ty))
        };
        let params: Vec<Ty> = params.iter().map(bound).collect();
        let params = params.as_slice();
        let bound_variadic = variadic.map(bound);
        let variadic = bound_variadic.as_ref();
        let values = DependentValues::own(&self.param_context, &signature.decls(), &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = expand_solved_packs(ty, &signature.decls(), &tyargs);
            let substituted = self.resolve_assoc_ty(&substitute(&expanded, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        // A method-level type pack (`*args: *Ts`) inferred from the overflow
        // arguments resolves to the heterogeneous element list, so each
        // overflow argument scores and converts against its own element
        // (the specialized-pack shape the selection pass already handles).
        let resolved_variadic = match variadic.map(Ty::kind) {
            Some(TyKind::Param { binder, .. }) if binder.name.starts_with('*') => {
                let pack = signature
                    .decls()
                    .iter()
                    .position(|decl| decl.id() == &binder.id)
                    .and_then(|index| tyargs.get(index));
                match pack {
                    Some(TyArg::Val(CtValue::Tuple(values)))
                        if values.iter().all(|value| matches!(value, CtValue::Type(_))) =>
                    {
                        Some(
                            TyKind::RuntimePack(
                                values
                                    .iter()
                                    .map(|value| match value {
                                        CtValue::Type(ty) => ty.clone(),
                                        _ => unreachable!("checked above"),
                                    })
                                    .collect(),
                            )
                            .into(),
                        )
                    }
                    _ => variadic.map(resolve).transpose()?,
                }
            }
            _ => variadic.map(resolve).transpose()?,
        };
        let declared = self.declared_call_arguments(
            &signature.declared,
            &tyargs,
            &bracket_origins,
            origin_bindings.as_ref(),
        );
        Ok(MethodInstantiation {
            params: params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            variadic: resolved_variadic,
            kw_variadic: kw_variadic.map(resolve).transpose()?,
            subst,
            arguments: tyargs,
            declared,
            attributes,
            origin_bindings,
            bracket_origins,
        })
    }

    /// `signature` with each own value parameter's type closed under the
    /// receiver's arguments, when one names a pack element of the struct's
    /// (`elt_handler: def[index: Int](var element: Self.Ts[index])`) or a
    /// callable contract names its parameters (`h: def(Scalar[Self.dtype])`),
    /// or a type parameter's callable bound names them (`FuncType:
    /// def[w: SIMDLength](Self._T[w]) …`); `None` when no declaration does.
    pub(super) fn method_decls_at_receiver(
        &self,
        signature: &MethodSig,
        info: &StructInfo,
        targs: &[TyArg],
    ) -> Option<MethodSig> {
        struct CloseElements<'a> {
            checker: &'a Checker,
            decls: &'a [ParamDecl],
            pack: &'a [TyArg],
            closed: bool,
        }

        impl mojito_types::types::TyRewrite for CloseElements<'_> {
            fn whole(&mut self, ty: &Ty) -> Option<Ty> {
                if !matches!(ty.kind(), TyKind::Dependent(dependent) if dependent.pack_element().is_some())
                {
                    return None;
                }
                let closed = self
                    .checker
                    .close_pack_elements(ty.clone(), &[(self.decls, self.pack)]);
                self.closed |= closed != *ty;
                Some(closed)
            }

            fn expr(
                &mut self,
                expr: &ParamExpr,
            ) -> Result<ParamExpr, mojito_types::param_expr::ParamError> {
                Ok(expr.clone())
            }
        }

        if targs.is_empty()
            || !signature.declared.iter().any(|decl| {
                matches!(
                    decl,
                    ParamDecl::Value { .. }
                        | ParamDecl::Type {
                            default: Some(_),
                            ..
                        }
                        | ParamDecl::Type {
                            callable_bound: Some(_),
                            ..
                        }
                        | ParamDecl::Origin {
                            mutability: OriginMutability::Binder(_),
                            ..
                        }
                )
            })
        {
            return None;
        }
        let view = info.view_arguments(targs);
        let owner_values = owner_value_bindings(&info.decls(), &view);
        let pack = positional_pack_arguments(&info.decls(), &view);
        let mut rewrite = CloseElements {
            checker: self,
            decls: &info.decls(),
            pack: &pack,
            closed: false,
        };
        let mut closed = signature.clone();
        let mut retyped = false;
        for decl in &mut closed.declared {
            match decl {
                ParamDecl::Value { ty, default, .. } => {
                    let rewritten = mojito_types::types::rewrite_ty(ty, &mut rewrite)
                        .unwrap_or_else(|_| (*ty).clone());
                    // A value parameter typed by the struct's own type binder
                    // (`[v: Self.T]`) takes the receiver's argument for it.
                    let at_receiver = substitute_at(&rewritten, info, targs);
                    retyped |= at_receiver != rewritten;
                    *ty = at_receiver;
                    // A default reading the struct's own binder
                    // (`[n: Int = Self.k]`) reads the receiver's argument.
                    if let Some(value) = default
                        && let Ok(replaced) = mojito_types::param_expr::ParamContext::detached()
                            .replace(value, &owner_values)
                        && replaced != *value
                    {
                        *value = replaced;
                        retyped = true;
                    }
                }
                ParamDecl::Type {
                    default,
                    callable_bound,
                    ..
                } => {
                    for ty in callable_bound.iter_mut().chain(default.iter_mut()) {
                        let rewritten = mojito_types::types::rewrite_ty(ty, &mut rewrite)
                            .unwrap_or_else(|_| (*ty).clone());
                        let at_receiver = substitute_at(&rewritten, info, targs);
                        let at_receiver = mojito_types::types::replace_parameters(
                            &mojito_types::param_expr::ParamContext::detached(),
                            &at_receiver,
                            &owner_values,
                            0,
                        )
                        .unwrap_or(at_receiver);
                        retyped |= at_receiver != *ty;
                        *ty = at_receiver;
                    }
                }
                // An origin binder typed by the struct's own `Bool` binder
                // (`[t: Origin[mut=Self.mut]]`) takes the receiver's
                // argument for it.
                ParamDecl::Origin { mutability, .. } => {
                    if let OriginMutability::Binder(binder) = mutability
                        && let Some(TyArg::Val(CtValue::Bool(fixed))) = info
                            .params
                            .iter()
                            .position(|decl| decl.id() == &binder.id)
                            .and_then(|slot| targs.get(slot))
                    {
                        *mutability = OriginMutability::Fixed(*fixed);
                        retyped = true;
                    }
                }
            }
        }
        (rewrite.closed || retyped).then_some(closed)
    }

    /// `ty` with its member reads on concrete subjects
    /// ([`Checker::resolve_assoc_ty`]) and the method's own value binders
    /// closed at a call's solved arguments (`Scalar[dt]` at `dt =
    /// DType.int16` is `Int16`), as a generic `def`'s result closes; a binder
    /// the call leaves symbolic stays as it stands.
    pub(super) fn close_method_values(
        &self,
        ty: &Ty,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Ty {
        let ty = self.resolve_assoc_ty(ty);
        let values = DependentValues::own(&self.param_context, decls, arguments);
        if values.is_empty() {
            return ty;
        }
        self.resolve_dependent_ty(&ty, &values).unwrap_or(ty)
    }

    /// Type the `DType` arguments a method call's brackets spell
    /// (`lanes.double[DType.int32](3)`): a call its template serves passes
    /// each as a register of that type.
    pub(super) fn record_dtype_parameter_arguments(
        &self,
        param_args: &[mojito_ast::ast::ParamArg],
        arguments: &[TyArg],
    ) {
        if !arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Val(value) if mojito_types::types::dtype_of_value(value).is_some()))
        {
            return;
        }
        for argument in param_args {
            let expression = match argument {
                mojito_ast::ast::ParamArg::Value(expression) => expression,
                mojito_ast::ast::ParamArg::Named { value, .. } => match value.as_ref() {
                    mojito_ast::ast::ParamArg::Value(expression) => expression,
                    _ => continue,
                },
                mojito_ast::ast::ParamArg::Type(_) => continue,
            };
            if self
                .eval_associated_ct(expression)
                .is_ok_and(|value| mojito_types::types::dtype_of_value(&value).is_some())
            {
                // Its value was solved already; inference only records how
                // it lowers.
                let _ = self.infer(expression);
            }
        }
    }

    pub(super) fn method_constraints_apply(
        &self,
        signature: &MethodSig,
        arguments: &[TyArg],
        struct_decls: &[ParamDecl],
        struct_arguments: &[TyArg],
    ) -> bool {
        self.method_constraint_result(signature, arguments, struct_decls, struct_arguments)
            .is_ok()
    }

    /// Evaluate method availability, keeping what a call diagnostic needs:
    /// the retained `(condition, "message")` text, or the violated clause
    /// itself, which may explain a sole-candidate call failure. `arguments`
    /// bind the method's own parameters, its argument view or, as upstream
    /// evaluates a generator's clause with every parameter bound, its whole
    /// declared list ([`method_declared_arguments`]), and
    /// `struct_arguments` its struct's `struct_decls`, each position by
    /// position; a lone variadic struct parameter binds the whole
    /// positional list as its pack.
    pub(super) fn method_constraint_result<'signature>(
        &self,
        signature: &'signature MethodSig,
        arguments: &[TyArg],
        struct_decls: &[ParamDecl],
        struct_arguments: &[TyArg],
    ) -> Result<(), ConstraintFailure<'signature>> {
        let struct_arguments = positional_pack_arguments(struct_decls, struct_arguments);
        let mut environment = if arguments.len() == signature.declared.len() {
            ConstraintEnvironment::declared(&signature.declared, arguments)
        } else {
            ConstraintEnvironment::declared(&signature.decls(), arguments)
        };
        environment.bind_declared(struct_decls, &struct_arguments);
        for constraint in &signature.availability {
            match self.constraint_verdict(constraint, &environment) {
                Ok(ConstraintVerdict::Proven) => {}
                Ok(ConstraintVerdict::Residual(proposition))
                    if self.assumptions_prove(&proposition) => {}
                Ok(ConstraintVerdict::Residual(_)) => {
                    return Err(ConstraintFailure::Unproven(constraint));
                }
                Ok(ConstraintVerdict::Disproven) | Err(_) => {
                    return Err(match constraint {
                        GenericConstraint::WithMessage(_, message) => {
                            ConstraintFailure::Message(message.as_str())
                        }
                        violated => ConstraintFailure::Violated(violated),
                    });
                }
            }
        }
        Ok(())
    }
    /// Whether a bracket after `name` binds a callable's parameters: the
    /// name denotes a function, and it is a declared `def` or a compile-time
    /// callable (a callable parameter, a `comptime` binding) rather than a
    /// runtime value holding one.
    fn binds_bracket_parameters(&self, name: &str) -> bool {
        self.lookup(name).is_some_and(|ty| {
            matches!(
                ty.kind(),
                TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
            ) || super::is_closure_storage(ty)
        }) && (self.lookup_callable_origins(name).is_some()
            || self.is_compile_time_binding(name)
            || self.is_value_parameter(name))
    }

    /// The candidates `source` reads as a function value, or `None` when it
    /// names no function: the value's type (an `Overload` of several), each
    /// candidate's origin signature, and the symbol each lowers to.
    fn callable_value_candidates(
        &self,
        span: &SourceSpan,
        source: CallableValueSource<'_>,
        record: bool,
    ) -> Result<Option<CallableValueCandidates>, TypeError> {
        let (owner, method, receiver) = match source {
            CallableValueSource::Static {
                owner,
                method,
                receiver,
            } => (owner, method, receiver),
            CallableValueSource::Binding(name) => {
                let Some(callable) = self.lookup(name).cloned() else {
                    return Ok(None);
                };
                if !matches!(
                    callable.kind(),
                    TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
                ) && !super::is_closure_storage(&callable)
                {
                    return Ok(None);
                }
                self.check_capture_access(name, false)?;
                if record && let Some(owner) = self.lookup_owner(name) {
                    self.record_at(span.clone()).binding = Some(owner);
                }
                let targets = match callable.kind() {
                    TyKind::Overload(candidates) => candidates
                        .iter()
                        .map(|candidate| callable_lowered_name(name, candidate))
                        .collect(),
                    _ => vec![Some(name.to_string())],
                };
                return Ok(Some(CallableValueCandidates {
                    name: name.to_string(),
                    callable,
                    targets,
                }));
            }
        };
        let synthesized;
        let signatures = match self
            .structs
            .get(owner)
            .and_then(|info| info.methods.get(method))
        {
            Some(signatures) => signatures,
            None => match self.fieldwise_initializer_sig(owner, method) {
                Some(signature) => {
                    synthesized = vec![signature];
                    &synthesized
                }
                None => return Ok(None),
            },
        };
        // An initializer's `out self` is its result, as upstream's
        // `__init__` is a function returning the struct it initializes; an
        // instance method's receiver is its first parameter.
        let instance = TyKind::Struct(owner.to_string(), receiver.to_vec()).into();
        let statics = signatures
            .iter()
            .filter(|sig| !sig.takes_ref_receiver())
            .collect::<Vec<_>>();
        let info = self.structs.get(owner);
        let mut types = statics
            .iter()
            .map(|sig| {
                let ty = method_value_ty(sig, &instance);
                match info {
                    Some(info) if !receiver.is_empty() => {
                        self.resolve_assoc_ty(&substitute_at(&ty, info, receiver))
                    }
                    _ => ty,
                }
            })
            .collect::<Vec<_>>();
        // A method sharing its name with another lowers to its overload's
        // symbol, as the static call through the struct names it.
        let targets = statics
            .iter()
            .map(|sig| {
                Some(if signatures.len() > 1 {
                    method_lowered_name(owner, method, sig, self.self_instance_ty(owner).as_ref())
                } else {
                    format!("{owner}.{method}")
                })
            })
            .collect();
        let callable = match types.len() {
            0 => return Ok(None),
            1 => types.pop().expect("one static"),
            _ => TyKind::Overload(types).into(),
        };
        Ok(Some(CallableValueCandidates {
            name: format!("{owner}.{method}"),
            callable,
            targets,
        }))
    }

    /// `origin` folded as [`Self::fold_origin_mut_casts`] folds a type's
    /// origin arguments: a cast `o.unsafe_mut_cast[m]()` (`ImmOrigin(o)`
    /// at `m = False`) to the mutability `o` already has is `o`, as the pin
    /// compares them.
    pub(in crate::checker) fn fold_origin_mut_cast(
        &self,
        origin: &mojito_types::origin::Origin,
    ) -> mojito_types::origin::Origin {
        use mojito_types::origin::Origin;
        match origin {
            Origin::MutCast(inner, mutability) => {
                let inner = self.fold_origin_mut_cast(inner);
                if self.origin_has_mutability(&inner, mutability) {
                    inner
                } else {
                    Origin::mut_cast(inner, mutability.clone())
                }
            }
            Origin::Union(members) => Origin::Union(
                members
                    .iter()
                    .map(|member| self.fold_origin_mut_cast(member))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Whether `origin` is an `Origin[mut=mutability]` value already: an
    /// immutable one for `False`, a mutable one for `True`, and an origin
    /// binder declared `Origin[mut=m]` for a read of its own `m`.
    fn origin_has_mutability(
        &self,
        origin: &mojito_types::origin::Origin,
        mutability: &mojito_types::types::LitOriginMut,
    ) -> bool {
        use mojito_types::origin::{Mutability, Origin};
        use mojito_types::types::LitOriginMut;
        match mutability {
            LitOriginMut::Known(false) => self.origin_is_immutable(origin),
            LitOriginMut::Known(true) => {
                origin.static_mutability() == Some(true)
                    || self.bound_origin_mutability(origin) == Some(Mutability::Mutable)
            }
            LitOriginMut::Expr(read) => {
                let Origin::Param(id) = origin else {
                    return false;
                };
                let declared = |params: &[mojito_ast::ast::TypeParam]| {
                    self.origin_binder_at(params, id.0 as usize)
                        .and_then(|decl| match decl {
                            ParamDecl::Origin {
                                mutability: OriginMutability::Binder(binder),
                                ..
                            } => Some(binder.name),
                            _ => None,
                        })
                };
                let own = self.callable_type_params.borrow().last().cloned();
                read.as_decl_ref().is_some_and(|read| {
                    own.and_then(|own| declared(&own))
                        .or_else(|| declared(&self.enclosing_type_params))
                        .is_some_and(|binder| binder == read.name)
                })
            }
        }
    }

    /// Whether `origin` is an `Origin[mut=False]` value: a place whose owner
    /// in scope is read-only, a binder declared `mut=False`, a cast, an
    /// immutable builtin origin, or a union with an immutable member.
    fn origin_is_immutable(&self, origin: &mojito_types::origin::Origin) -> bool {
        use mojito_types::origin::{Mutability, Origin};
        match origin {
            Origin::Static
            | Origin::Untracked { mutable: false }
            | Origin::UnsafeAny { mutable: false } => true,
            Origin::Place(place) if self.owner_scope(place.root).is_none() => false,
            Origin::Union(members) => members
                .iter()
                .any(|member| self.origin_is_immutable(member)),
            _ => self.bound_origin_mutability(origin) == Some(Mutability::Immutable),
        }
    }

    /// The origin argument `argument`, at source `position`, of the origin
    /// binder `decl` declared at `index` as `id`, converted to the binder's
    /// type ([`Self::bound_bracket_origins`]).
    fn bound_bracket_origin(
        &self,
        (name, decls): (&str, &[ParamDecl]),
        (index, decl, id): (usize, &ParamDecl, mojito_types::origin::OriginParamId),
        (position, argument): (usize, &mojito_ast::ast::ParamArg),
        use_: BracketUse,
    ) -> Result<BoundOrigin, TypeError> {
        use mojito_types::origin::{Mutability, Origin};
        use mojito_types::types::OriginMutability;
        self.convert_bracket_origin(decls, decl, argument)?;
        self.check_origin_argument_representation(name, decl, argument)?;
        let (origin, mutability, attribute) = match use_ {
            BracketUse::Type(strictness) => {
                let (origin, mutability) = match self.resolve_origin_param_arg(argument) {
                    Ok(resolved) => super::type_resolution::immutable_slot_origin(decl, resolved),
                    // A signature annotation may name places only the body
                    // resolves (`origin_of(self.entries)`): an origin-shaped
                    // argument binds its slot unbound.
                    Err(_)
                        if self.signature_origin_leniency.get()
                            && super::type_resolution::syntactic_origin_argument(argument) =>
                    {
                        (Origin::Unbound, None)
                    }
                    Err(error) => return Err(error),
                };
                // A value-shaped origin argument of a storage annotation
                // is no attribute of the annotation's application.
                let attribute = (strictness == super::StorageStrictness::Off
                    || !matches!(argument, mojito_ast::ast::ParamArg::Value(_)))
                .then(|| TyArg::Origin(origin.clone()));
                (origin, mutability, attribute)
            }
            BracketUse::Call => {
                let origin = self.explicit_origin_argument(argument)?;
                let own = self
                    .resolve_origin_param_arg(argument)
                    .ok()
                    .and_then(|(_, own)| own);
                // A parametric binder's `mut=` is the argument's own
                // mutability, an `ImmOrigin(o)` conversion's `False`.
                let origin = match decl {
                    ParamDecl::Origin {
                        mutability: OriginMutability::Binder(_),
                        ..
                    } if own == Some(Mutability::Immutable) => Origin::immutable(origin),
                    _ => origin,
                };
                let attribute = bracket_attribute(argument, &TyArg::Origin(origin.clone()));
                (origin, own, attribute)
            }
        };
        Ok(BoundOrigin {
            slot: index,
            id,
            position,
            attribute,
            origin,
            mutability,
        })
    }

    /// The `mut=` argument `argument`, at source `position`, of the
    /// mutability binder `binder` declared at `index` among `decls`: a
    /// `Bool`, `True` only where the origin `origins` bind its slot to may
    /// be mutable. A call's `False` converts that origin to `ImmOrigin`, as
    /// upstream passes the origin argument at the binder's type.
    fn bound_bracket_mutability(
        &self,
        name: &str,
        decls: &[ParamDecl],
        (index, binder): (usize, &ParamDecl),
        (position, argument): (usize, &mojito_ast::ast::ParamArg),
        (origins, use_): (&mut [BoundOrigin], BracketUse),
    ) -> Result<BoundMutability, TypeError> {
        use mojito_types::origin::{Mutability, Origin};
        check_origin_mutability_argument(name, decls, binder, argument, origins)?;
        let value = self.origin_mutability_argument_value(binder, argument);
        if let CtValue::Expr(read) = &value
            && let Some(found) = read.meta().as_value()
            && !found.kind().is_bool()
        {
            return Err(TypeError::TypeMismatch {
                expected: "Bool".to_string(),
                found: found.to_string(),
                context: format!("parameter '{}' of '{name}'", binder.name()),
            });
        }
        if use_ == BracketUse::Call && matches!(value, CtValue::Bool(false)) {
            for bound in origins
                .iter_mut()
                .filter(|bound| governs_origin(decls, binder, bound.slot))
            {
                if bound.mutability != Some(Mutability::Immutable) {
                    bound.origin = Origin::immutable(bound.origin.clone());
                    bound.mutability = Some(Mutability::Immutable);
                    bound.attribute = bound
                        .attribute
                        .as_ref()
                        .map(|_| TyArg::Origin(bound.origin.clone()));
                }
            }
        }
        Ok(BoundMutability {
            slot: index,
            id: binder.id().clone(),
            position,
            value,
        })
    }

    /// The value an explicit `mut=` keyword `argument` gives the origin's
    /// mutability binder `binder`: a literal's `Bool`, the read of a value
    /// binder in scope, and [`CtValue::Deferred`] for `_` or a spelling
    /// that names no binder in scope, which the slot then infers.
    fn origin_mutability_argument_value(
        &self,
        binder: &ParamDecl,
        argument: &mojito_ast::ast::ParamArg,
    ) -> CtValue {
        use mojito_ast::ast::ParamArg;
        let mut value = argument;
        while let ParamArg::Named { value: inner, .. } = value {
            value = inner;
        }
        match value {
            ParamArg::Value(expression) => match &expression.kind {
                ExprKind::Bool(literal) => Some(CtValue::Bool(*literal)),
                ExprKind::Identifier(name) if name != "_" => {
                    self.value_parameter_in_scope(name).map(CtValue::Expr)
                }
                _ => None,
            },
            _ => None,
        }
        .unwrap_or_else(|| CtValue::Deferred(binder.binder()))
    }

    /// Convert bracket origin `argument` to the `Origin[mut=…]` type its
    /// binder `decl` declares among `decls`, as upstream passes a bracket
    /// value: an immutable origin does not convert to a `MutOrigin` binder,
    /// and a binder whose `mut=` reads a binder outside `decls` the call
    /// leaves symbolic (`Origin[mut=Self.mut]` through a receiver `S[_]`)
    /// takes only an origin of that same mutability.
    fn convert_bracket_origin(
        &self,
        decls: &[ParamDecl],
        decl: &ParamDecl,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<(), TypeError> {
        use mojito_types::origin::Mutability;
        let ParamDecl::Origin {
            mutability, lit, ..
        } = decl
        else {
            return Ok(());
        };
        let own = || {
            self.resolve_origin_param_arg(argument)
                .ok()
                .and_then(|(_, own)| own)
        };
        let (expected, found) = match mutability {
            OriginMutability::Fixed(true) if own() == Some(Mutability::Immutable) => (
                super::origin_value_type(Some(true), *lit),
                super::origin_value_type(Some(false), *lit),
            ),
            OriginMutability::Binder(binder)
                if !binder.is_unbound() && !decls.iter().any(|own| own.id() == &binder.id) =>
            {
                let Some(fixed @ (Mutability::Mutable | Mutability::Immutable)) = own() else {
                    return Ok(());
                };
                (
                    mutability.binder_type(*lit),
                    super::origin_value_type(Some(fixed == Mutability::Mutable), *lit),
                )
            }
            _ => return Ok(()),
        };
        Err(TypeError::ParamListConversion {
            param: decl.name().to_string(),
            from: found,
            to: expected,
        })
    }
}

/// Why an availability clause rejected a call.
pub(super) enum ConstraintFailure<'a> {
    /// The `(condition, "message")` form: the message is the diagnostic.
    Message(&'a str),
    /// A message-less clause: the diagnostic spells the clause itself.
    Violated(&'a GenericConstraint),
    /// A clause neither proven nor disproven, which the scope's
    /// assumptions do not discharge.
    Unproven(&'a GenericConstraint),
}

impl ConstraintFailure<'_> {
    /// The call diagnostic's reason. A retained message keeps its
    /// `constraint failed:` shape; a violated clause reports upstream's note.
    pub(super) fn reason(&self) -> String {
        match self {
            ConstraintFailure::Message(message) => format!("constraint failed: {message}"),
            ConstraintFailure::Violated(constraint) => violated_constraint_reason(constraint),
            ConstraintFailure::Unproven(constraint) => {
                format!("{LACKING_EVIDENCE}cannot prove constraint '{constraint}'")
            }
        }
    }
}

/// Upstream's wording for a failed message-less `where` clause: the error
/// line's `violated constraint` joined with its note naming the clause.
pub(super) fn violated_constraint_reason(constraint: &impl std::fmt::Display) -> String {
    format!(
        "violated constraint; constraint declared here evaluated to False, expected '{constraint}'"
    )
}

/// The function values a callable read selects among, as
/// [`Checker::callable_value_candidates`] resolves them.
struct CallableValueCandidates {
    /// The spelling diagnostics and parameter binding name the value by.
    name: String,
    callable: Ty,
    /// Per candidate (one for a callable that is no `Overload`), the
    /// symbol it lowers to; `None` for a candidate that names none.
    targets: Vec<Option<String>>,
}

/// The value type of the method `sig` read through its struct, the
/// instance `owner`: a thin function, since the read captures nothing,
/// generic over the method's own parameters. An initializer returns the
/// struct its `out self` initializes, and an instance method takes its
/// receiver as its first parameter, as upstream's method is a function
/// whose `self` is an ordinary argument.
/// The `Bool` literal a `mut=` argument of `name`'s mutability binder
/// `binder` spells, `None` for a value spelling one (`Self.mut`, `_`), and
/// a mismatch for any other literal or a type.
pub(super) fn origin_mutability_literal(
    name: &str,
    binder: &str,
    argument: &mojito_ast::ast::ParamArg,
) -> Result<Option<bool>, TypeError> {
    use mojito_ast::ast::ParamArg;
    let mut value = argument;
    while let ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    let mismatch = |found: &str| TypeError::TypeMismatch {
        expected: "Bool".to_string(),
        found: found.to_string(),
        context: format!("parameter '{binder}' of '{name}'"),
    };
    match value {
        ParamArg::Value(expression) => match &expression.kind {
            ExprKind::Bool(literal) => Ok(Some(*literal)),
            ExprKind::Int(_) => Err(mismatch("IntLiteral")),
            ExprKind::Float(_) => Err(mismatch("FloatLiteral")),
            ExprKind::Str(_) => Err(mismatch("StringLiteral")),
            _ => Ok(None),
        },
        ParamArg::Type(_) => Err(mismatch("a type")),
        ParamArg::Named { .. } => Ok(None),
    }
}

fn method_value_ty(sig: &MethodSig, owner: &Ty) -> Ty {
    let mut func = method_func_ty(sig, mojito_types::origin::CallableEnvironment::Thin);
    if is_initializer_sig(sig) {
        func.ret = sig.refined_self.clone().unwrap_or_else(|| owner.clone());
    } else if sig.has_self {
        func = with_receiver_parameter(func, sig.self_convention, owner);
    }
    if sig.decls().is_empty() {
        return TyKind::Func(Box::new(func)).into();
    }
    TyKind::GenericFunc(Box::new(GenericFuncTy {
        decls: sig.declared.clone(),
        func,
    }))
    .into()
}

/// `func`, a method's call surface, with its `self` receiver of type
/// `owner` passed under `convention` as the leading positional parameter:
/// each reference contract naming the receiver names that parameter, and
/// every other parameter's index moves past it.
fn with_receiver_parameter(
    mut func: FuncTy,
    convention: Option<mojito_ast::ast::ArgConvention>,
    owner: &Ty,
) -> FuncTy {
    let shift = |sig: &RefSig| RefSig {
        origin: receiver_shifted_origin(&sig.origin),
        mutability: sig.mutability.clone(),
        address_space: sig.address_space.clone(),
    };
    func.ref_params = std::iter::once(None)
        .chain(func.ref_params.iter().map(|sig| sig.as_ref().map(shift)))
        .collect();
    func.ref_return = func.ref_return.as_deref().map(|sig| Box::new(shift(sig)));
    func.params.insert(0, owner.clone());
    func.names.insert(0, "self".to_string());
    func.required.insert(0, true);
    func.conventions.insert(0, convention);
    func.positional_only = Some(func.positional_only.map_or(1, |count| count + 1));
    func.keyword_only = func.keyword_only.map(|index| index + 1);
    func
}

/// `origin`, a reference contract of a method's call surface, read on the
/// function whose first parameter is the receiver.
fn receiver_shifted_origin(origin: &SigOrigin) -> SigOrigin {
    match origin {
        SigOrigin::Self_ => SigOrigin::Param(0),
        SigOrigin::Param(index) => SigOrigin::Param(index + 1),
        SigOrigin::Projected(base, segments) => {
            SigOrigin::Projected(Box::new(receiver_shifted_origin(base)), segments.clone())
        }
        SigOrigin::Union(members) => {
            SigOrigin::Union(members.iter().map(receiver_shifted_origin).collect())
        }
        other => other.clone(),
    }
}

/// Whether `sig` is an initializer, whose `out self` receiver is its result.
fn is_initializer_sig(sig: &MethodSig) -> bool {
    sig.has_self && sig.self_convention == Some(mojito_ast::ast::ArgConvention::Out)
}

/// Whether a struct symbol names the bundled `Span` view (bare or
/// module-qualified).
fn is_span_struct_name(name: &str) -> bool {
    name == "Span" || name.ends_with("$Span")
}

/// The declaration-order compile-time arguments a generic method call
/// resolved, materialized for its specialization identity, when the
/// signature declares any parameter, origins included: upstream's method
/// with only origin binders is a generator too.
pub(super) fn method_instantiation_arguments(
    signature: &MethodSig,
    arguments: &[TyArg],
) -> Option<Vec<TyArg>> {
    (!signature.declared.is_empty() && arguments.len() == signature.decls().len())
        .then(|| arguments.to_vec())
}

/// [`method_instantiation_arguments`] over the whole declared list: the
/// call's `declared` arguments ([`Checker::declared_call_arguments`]) when
/// it resolved its argument view's.
pub(super) fn method_declared_arguments(
    signature: &MethodSig,
    arguments: &[TyArg],
    declared: &[TyArg],
) -> Option<Vec<TyArg>> {
    method_instantiation_arguments(signature, arguments)
        .map(|_| declared.to_vec())
        .filter(|declared| declared.len() == signature.declared.len())
}

/// [`substitute_at`], binding the receiver's origin tail into pointer
/// provenances too when `binder_mutability` resolves a parametric binder's
/// mutability (see [`Checker::substitute_at_instance`]).
fn substitute_at_with(
    ty: &Ty,
    info: &StructInfo,
    targs: &[TyArg],
    binder_mutability: Option<super::origins::BinderMutability<'_>>,
) -> Ty {
    let view = info.view_arguments(targs);
    let expanded = expand_bound_pack(ty, &info.decls(), &view);
    // The receiver's origin tail binds the struct's own origin binders in
    // struct-typed positions (`other: Self`, `-> Self`, a `RefBox[Self.o]`
    // parameter), so a member instantiated on `P[origin_of(xs)]` names that
    // origin where its declaration names `Self.o`. It binds the declaration
    // before the type arguments go in: a caller's origin binder inside a type
    // argument (`Span[Span[Int, o], _]`) shares the id space of the struct's
    // own slots, and must not be captured by them.
    let tails = info.instance_origin_bindings(targs);
    let mut bound = substitute_struct_origins(&expanded, &tails);
    if binder_mutability.is_some() {
        bound = super::origins::bind_pointer_origins(&bound, &tails, binder_mutability);
    }
    substitute_assoc(
        &bound,
        &AssocBindings {
            types: struct_subst(&info.params, &view),
            // The whole list, so a read of an origin's `mut=` binder
            // (`Self.mut`) takes the instance's value beside its origin.
            values: solved_value_bindings(&info.params, targs),
            ids: forwarded_pack_lists(&info.decls(), &view),
            origins: HashMap::new(),
        },
    )
}

/// `callable` with each `ref` contract over a bound origin binder rebound to
/// the origin `tails` binds it to.
fn bind_callable_ref_origins(
    mut callable: Ty,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
) -> Ty {
    callable.update(|kind| {
        let Some(FuncTy {
            ref_params,
            ref_return,
            ..
        }) = kind.callable_mut()
        else {
            return;
        };
        for signature in ref_params
            .iter_mut()
            .flatten()
            .chain(ref_return.as_deref_mut())
        {
            signature.origin = bind_sig_origin(&signature.origin, tails);
        }
    });
    callable
}

/// A generic function value's bracket bound
/// ([`Checker::bind_generic_callable_value`]).
struct BoundCallableValue {
    /// The function type at what the bracket bound.
    contract: Ty,
    /// The bracket's arguments over the callee's argument view.
    view: Vec<TyArg>,
    /// The bracket's arguments over the callee's whole declared list.
    declared: Vec<TyArg>,
    /// The origin binders the bracket left open.
    open: Vec<ParamDecl>,
}

/// `callable` with every struct origin tail of its parameter, collector,
/// and result types that names an explicitly bound origin binder rebound to
/// that origin (`g[origin_of(xs)]` over `-> Pointer[Span[Int, o], …]`), and
/// every pointer nested in them (`-> Pointer[Pointer[Int, o], …]`) too.
fn bind_callable_origins(
    mut callable: Ty,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
    binder_mutability: super::origins::BinderMutability<'_>,
) -> Ty {
    if tails.is_empty() {
        return callable;
    }
    if callable.callable().is_none() {
        return callable;
    }
    callable.update(|kind| {
        let Some(FuncTy {
            params,
            variadic,
            kw_variadic,
            ret,
            ..
        }) = kind.callable_mut()
        else {
            return;
        };
        let bind = |ty: &mut Ty| *ty = bind_struct_origins(ty, tails, binder_mutability);
        params.iter_mut().for_each(bind);
        variadic
            .iter_mut()
            .chain(kw_variadic.iter_mut())
            .for_each(&bind);
        bind(ret);
    });
    callable
}

/// The struct value binders `decls` bound to the receiver's arguments
/// `targs`, by identity, for replacing them inside a member's parameter
/// expressions.
fn owner_value_bindings(
    decls: &[ParamDecl],
    targs: &[TyArg],
) -> mojito_types::param_expr::ParamBindings {
    let context = mojito_types::param_expr::ParamContext::detached();
    let mut bindings = mojito_types::param_expr::ParamBindings::new();
    bindings.bind_value_arguments(&context, decls, targs);
    bindings
}

/// Whether the origin binder declared at `slot` among `decls` names
/// `binder` as its mutability (`o: Origin[mut=mut]`).
fn governs_origin(decls: &[ParamDecl], binder: &ParamDecl, slot: usize) -> bool {
    matches!(decls.get(slot), Some(ParamDecl::Origin {
        mutability: mojito_types::types::OriginMutability::Binder(mutability),
        ..
    }) if mutability.id == *binder.id())
}

/// Check a `mut=` argument binding the mutability binder `binder` of
/// `name`: a `Bool`, or a value spelling one (`Self.mut`, `_`), and `True`
/// only when the origin `origins` bind the slot it governs to may be
/// mutable.
fn check_origin_mutability_argument(
    name: &str,
    decls: &[ParamDecl],
    binder: &ParamDecl,
    argument: &mojito_ast::ast::ParamArg,
    origins: &[BoundOrigin],
) -> Result<(), TypeError> {
    let literal = origin_mutability_literal(name, binder.name(), argument)?;
    let immutable_slot = origins.iter().find(|bound| {
        bound.mutability == Some(mojito_types::origin::Mutability::Immutable)
            && governs_origin(decls, binder, bound.slot)
    });
    if literal == Some(true)
        && let Some(bound) = immutable_slot
    {
        return Err(TypeError::TypeMismatch {
            expected: format!(
                "a mutable origin for parameter '{}' of '{name}'",
                decls[bound.slot].name()
            ),
            found: "an immutable origin".to_string(),
            context: "explicit origin argument".to_string(),
        });
    }
    Ok(())
}
