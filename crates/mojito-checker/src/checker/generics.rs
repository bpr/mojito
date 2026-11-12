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
                && let Some(TyArg::Ty(element_pattern)) = pargs.first()
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
        TyKind::Pointer {
            element: pattern_element,
            ..
        } => {
            if let TyKind::Pointer { element, .. } = actual.kind() {
                unify(pattern_element, element, subst, deduce)?;
            }
            Ok(())
        }
        // So do its raw `!kgen.pointer` and `!lit.ref`.
        TyKind::KgenPointer(pattern_element) => match actual.kind() {
            TyKind::KgenPointer(element) => unify(pattern_element, element, subst, deduce),
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
    let expanded = expand_bound_pack(ty, &info.decls, targs);
    // The receiver's origin tail binds the struct's own origin binders in
    // struct-typed positions (`other: Self`, `-> Self`, a `RefBox[Self.o]`
    // parameter), so a member instantiated on `P[origin_of(xs)]` names that
    // origin where its declaration names `Self.o`. It binds the declaration
    // before the type arguments go in: a caller's origin binder inside a type
    // argument (`Span[Span[Int, o], _]`) shares the id space of the struct's
    // own slots, and must not be captured by them.
    let bound = substitute_struct_origin_tails(&expanded, &info.tail_origin_bindings(targs));
    substitute_assoc(
        &bound,
        &AssocBindings {
            types: struct_subst(&info.decls, targs),
            values: solved_value_bindings(&info.decls, targs),
            ids: mojito_types::param_expr::ParamBindings::new(),
            origins: HashMap::new(),
        },
    )
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
    let Some(declared) = info.decls.iter().find_map(|decl| match decl {
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
    if let Some((_, mut elements)) = bound_pack_elements(&info.decls, targs) {
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
    decls
        .iter()
        .zip(tyargs)
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
            decls
                .iter()
                .zip(targs)
                .find_map(|(decl, argument)| match (decl, argument) {
                    (
                        ParamDecl::Type { variadic: true, .. },
                        TyArg::Val(binding @ CtValue::Tuple(_)),
                    ) => Some((decl.spelling().to_string(), binding.clone())),
                    _ => None,
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
    decls
        .iter()
        .zip(tyargs)
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
        (TyKind::Ref(pattern_reference), TyKind::Ref(actual_reference)) => {
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
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: substitute_self(element, replacement),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => {
            TyKind::KgenPointer(substitute_self(element, replacement)).into()
        }
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: substitute_self(element, replacement),
            origin: origin.clone(),
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
    let bound = mojito_types::types::substitute_origin_params(ty, &bindings.origins);
    let mut values = bindings.ids.clone();
    values.bind_named_values(&ParamContext::detached(), &bindings.values);
    substitute_values(&substitute(&bound, &bindings.types), &values)
}

fn substitute_values(ty: &Ty, bindings: &mojito_types::param_expr::ParamBindings) -> Ty {
    let recur = |t: &Ty| substitute_values(t, bindings);
    let context = ParamContext::detached();
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
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recur(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(recur(element)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: recur(element),
            origin: origin.clone(),
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
impl Checker {
    /// Split the source parameter list at an explicit specialization site.
    /// Origin arguments are erased from the ordinary list handed to the
    /// generic binder, which binds the list as this split does
    /// ([`mojito_ast::call::match_parameter_slots`]), so an ordinary argument
    /// keeps its source form.
    pub(super) fn split_callable_specialization(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        signature: &CallableOriginSignature,
    ) -> Result<SplitCallableSpecialization, TypeError> {
        use mojito_ast::ast::ParamArg;

        if signature.origins.is_empty() {
            return Ok((
                arguments.to_vec(),
                ForwardedBrackets::all(arguments.len()),
                Vec::new(),
                HashMap::new(),
            ));
        }
        let slots = mojito_ast::call::match_parameter_slots(
            &signature.source,
            &mojito_ast::call::bracket_keywords(arguments),
        )
        .map_err(|error| error.into_type_error(name))?;
        let mut origins = vec![None; signature.origins.len()];
        let mut ordinary = Vec::new();
        let mut forwarded = ForwardedBrackets::empty(arguments.len());
        for (position, (argument, index)) in arguments.iter().zip(slots).enumerate() {
            let (is_named, value) = match argument {
                ParamArg::Named { value, .. } => (true, (**value).clone()),
                other => (false, other.clone()),
            };
            let parameter = &signature.source[index];
            if let Some(origin_index) = parameter.origin {
                let origin = self.explicit_origin_argument(&value)?;
                forwarded.keep(
                    position,
                    bracket_attribute(&value, &TyArg::Origin(origin.clone())),
                );
                origins[origin_index] = Some(origin);
            } else if parameter.ordinary {
                forwarded.forward(position);
                ordinary.push(if is_named {
                    ParamArg::Named {
                        name: parameter.name.trim_start_matches('*').to_string(),
                        value: Box::new(value),
                    }
                } else {
                    value
                });
            } else {
                return Err(TypeError::Unsupported(format!(
                    "semantic parameter '{}' is inferred and cannot be supplied explicitly",
                    parameter.name
                )));
            }
        }

        // A value type names an origin binder by its source slot
        // (`Span[Int, o]` in a result or parameter type), which the explicit
        // argument binds as it binds the `ref` signatures naming it.
        let tails = signature
            .source
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                let origin = origins.get(parameter.origin?)?.clone()?;
                Some((
                    mojito_types::origin::OriginParamId(u32::try_from(index).ok()?),
                    origin,
                ))
            })
            .collect();
        let bindings = signature
            .origins
            .iter()
            .zip(origins)
            .filter_map(|(parameter, origin)| {
                origin.map(|origin| (parameter.slots.clone(), origin))
            })
            .collect::<Vec<_>>();
        Ok((ordinary, forwarded, bindings, tails))
    }

    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn bind_callable_origins(
        &self,
        mut callable: Ty,
        bindings: &[(Vec<usize>, mojito_types::origin::Origin)],
    ) -> Ty {
        if callable.callable().is_none() {
            return callable;
        }
        callable.update(|kind| {
            let Some(FuncTy {
                ref_params,
                ref_return,
                ..
            }) = kind.callable_mut()
            else {
                return;
            };
            for signature in ref_params.iter_mut().flatten() {
                signature.origin = bind_sig_origin(&signature.origin, bindings);
            }
            if let Some(signature) = ref_return {
                signature.origin = bind_sig_origin(&signature.origin, bindings);
            }
        });
        callable
    }

    pub(super) fn prepare_callable_specialization(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<(Ty, Vec<mojito_ast::ast::ParamArg>, ForwardedBrackets), TypeError> {
        let Some(signature) = signature else {
            return Ok((
                callable,
                arguments.to_vec(),
                ForwardedBrackets::all(arguments.len()),
            ));
        };
        let (ordinary, forwarded, bindings, tails) =
            self.split_callable_specialization(name, arguments, signature)?;
        let callable =
            bind_callable_origin_tails(self.bind_callable_origins(callable, &bindings), &tails);
        Ok((callable, ordinary, forwarded))
    }

    /// Materialize the monomorphic checked view of an explicitly specialized
    /// generic function value. Generic execution remains type-erased; only its
    /// callable contract is instantiated here.
    pub(super) fn instantiate_generic_callable_value(
        &self,
        name: &str,
        callable: Ty,
        arguments: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        let TyKind::GenericFunc(generic) = callable.kind() else {
            return Ok((callable, Vec::new()));
        };
        let GenericFuncTy { decls, func } = (**generic).clone();
        let BoundParameters {
            subst,
            arguments: tyargs,
            ..
        } = self.resolve_use_params(name, &decls, arguments, &[], &[])?;
        let values = DependentValues::own(&self.param_context, &decls, &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = expand_solved_packs(ty, &decls, &tyargs);
            let substituted = self.resolve_assoc_ty(&substitute(&expanded, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let contract = TyKind::Func(Box::new(func.try_map_types(resolve)?));
        Ok((contract.into(), tyargs))
    }

    pub(super) fn specialize_callable_value_candidate(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        let (callable, ordinary, _) =
            self.prepare_callable_specialization(name, arguments, callable, signature)?;
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
            TyKind::GenericFunc(_) => {
                self.instantiate_generic_callable_value(name, callable, &ordinary)
            }
            TyKind::Func(_) if ordinary.is_empty() => Ok((callable, Vec::new())),
            TyKind::Func(_) => Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 0,
                got: ordinary.len(),
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
    /// subscripted. A `TypeApply`, which only the checker and the
    /// elaborator synthesize from a type spelling, is always an application.
    pub(super) fn parameter_application<'e>(
        &self,
        expr: &'e Expr,
    ) -> Option<(&'e str, std::borrow::Cow<'e, [mojito_ast::ast::ParamArg]>)> {
        let (name, args) = expr.bracket_application()?;
        (matches!(expr.kind, ExprKind::TypeApply { .. })
            || self.lookup(name).is_none()
            || self.binds_bracket_parameters(name))
        .then_some((name, args))
    }

    /// Type a bracket over a function, which binds its parameters, or
    /// `None` for any other base. A bracket over a function's name is
    /// spelled as the parameter application it is (`twice[4]`), and one
    /// over a static read through its struct binds the static's
    /// (`S.gst[10]`).
    pub(super) fn infer_bracket_callable(&self, expr: &Expr) -> Option<Result<Ty, TypeError>> {
        if let Some((name, args)) = self.bracket_callable(expr) {
            let spelled = super::initializer_list::derived_construction(
                expr,
                ExprKind::TypeApply { name, args },
            );
            return Some(self.infer_spelled(expr, spelled));
        }
        self.static_method_bracket(expr)
            .map(|bracket| self.infer_static_method_bracket(expr, &bracket))
    }

    /// Whether `value` reads a function its bracket specialized
    /// (`borrow[origin_of(v)]`, `twice[4]`) as a value.
    pub(super) fn specializes_callable_value(&self, value: &Expr) -> bool {
        let spelled = self.spelled_construction(value);
        let applied = spelled.as_ref().unwrap_or(value);
        matches!(applied.kind, ExprKind::TypeApply { .. })
            && self.has_fact(&applied.source_span(), |facts| {
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
        let Some(CallableValueCandidates {
            name,
            callable,
            signatures,
            targets,
        }) = self.callable_value_candidates(&span, source, record)?
        else {
            return Ok(None);
        };
        let name = name.as_str();
        let ((selected, instance_arguments), target) =
            if let TyKind::Overload(candidates) = callable.kind() {
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
                            self.specialize_callable_value_candidate(
                                name,
                                arguments,
                                candidate.clone(),
                                signatures.get(index),
                            )
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
                    signatures.get(index),
                )?;
                (specialized, target)
            } else {
                let specialized = self.specialize_callable_value_candidate(
                    name,
                    arguments,
                    callable,
                    signatures.first(),
                )?;
                let target = targets
                    .into_iter()
                    .next()
                    .flatten()
                    .unwrap_or_else(|| name.to_string());
                (specialized, target)
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
                        folded_arguments: Vec::new(),
                    });
            }
            self.record_at(span.clone()).overload_target = Some(target);
            self.record_at(span).ty = Some(selected.clone());
        }
        Ok(Some(selected))
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
        let bakeable = !info.decls.is_empty()
            && info.decls.iter().all(|decl| {
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
            && info.decls.len() == arguments.len()
            && !info.source_params.iter().any(|parameter| {
                matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                    || parameter.is_origin_mutability_binder(&info.source_params)
            })
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
        if signature.decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok((
                params.to_vec(),
                variadic.cloned(),
                kw_variadic.cloned(),
                HashMap::new(),
                Vec::new(),
                Vec::new(),
            ));
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
        let explicit = self.explicit_type_args(name, &signature.decls, param_args)?;
        let mut patterns = Vec::new();
        let mut actuals = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            patterns.push(params[index].clone());
            actuals.push(self.solving_argument_ty(
                expression,
                &params[index],
                &signature.decls,
                &explicit.subst,
            )?);
        }
        if let Some(element) = variadic {
            for position in matched.positional_overflow {
                patterns.push(element.clone());
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
                        &signature.decls,
                        &explicit.subst,
                    )?,
                };
                actuals.push(actual);
            }
        }
        if let Some(element) = kw_variadic {
            for position in matched.keyword_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&kwargs[position].value)?);
            }
            if let Some(actual) = forwarded_element {
                patterns.push(element.clone());
                actuals.push(actual);
            }
        }
        let BoundParameters {
            subst,
            arguments: tyargs,
            attributes,
        } = self.resolve_explicit_use_params(
            name,
            &signature.decls,
            (param_args, &explicit),
            &patterns,
            &actuals,
        )?;
        let values = DependentValues::own(&self.param_context, &signature.decls, &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = expand_solved_packs(ty, &signature.decls, &tyargs);
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
                    .decls
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
        Ok((
            params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            resolved_variadic,
            kw_variadic.map(resolve).transpose()?,
            subst,
            tyargs,
            attributes,
        ))
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
            || !signature.decls.iter().any(|decl| {
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
                )
            })
        {
            return None;
        }
        let owner_values = owner_value_bindings(&info.decls, targs);
        let pack = positional_pack_arguments(&info.decls, targs);
        let mut rewrite = CloseElements {
            checker: self,
            decls: &info.decls,
            pack: &pack,
            closed: false,
        };
        let mut closed = signature.clone();
        let mut retyped = false;
        for decl in &mut closed.decls {
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
    /// bind the method's own parameters and `struct_arguments` its struct's
    /// `struct_decls`, each position by position; a lone variadic struct
    /// parameter binds the whole positional list as its pack.
    pub(super) fn method_constraint_result<'signature>(
        &self,
        signature: &'signature MethodSig,
        arguments: &[TyArg],
        struct_decls: &[ParamDecl],
        struct_arguments: &[TyArg],
    ) -> Result<(), ConstraintFailure<'signature>> {
        let struct_arguments = positional_pack_arguments(struct_decls, struct_arguments);
        let mut environment = ConstraintEnvironment::declared(&signature.decls, arguments);
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
                    signatures: self.lookup_callable_origins(name).unwrap_or_default(),
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
            signatures: Vec::new(),
            targets,
        }))
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
    signatures: Vec<CallableOriginSignature>,
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
fn method_value_ty(sig: &MethodSig, owner: &Ty) -> Ty {
    let mut func = method_func_ty(sig, mojito_types::origin::CallableEnvironment::Thin);
    if is_initializer_sig(sig) {
        func.ret = sig.refined_self.clone().unwrap_or_else(|| owner.clone());
    } else if sig.has_self {
        func = with_receiver_parameter(func, sig.self_convention, owner);
    }
    if sig.decls.is_empty() {
        return TyKind::Func(Box::new(func)).into();
    }
    TyKind::GenericFunc(Box::new(GenericFuncTy {
        decls: sig.decls.clone(),
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
/// signature declares any.
pub(super) fn method_instantiation_arguments(
    signature: &MethodSig,
    arguments: &[TyArg],
) -> Option<Vec<TyArg>> {
    (!signature.decls.is_empty() && arguments.len() == signature.decls.len())
        .then(|| arguments.to_vec())
}

/// `callable` with every struct origin tail of its parameter, collector,
/// and result types that names an explicitly bound origin binder rebound to
/// that origin (`g[origin_of(xs)]` over `-> Pointer[Span[Int, o], …]`), and
/// every pointer nested in them (`-> Pointer[Pointer[Int, o], …]`) too.
fn bind_callable_origin_tails(
    mut callable: Ty,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
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
        let bind = |ty: &mut Ty| {
            *ty = bind_pointer_origin_tails(
                &super::origins::substitute_struct_origin_tails(ty, tails),
                tails,
            );
        };
        params.iter_mut().for_each(bind);
        variadic
            .iter_mut()
            .chain(kw_variadic.iter_mut())
            .for_each(&bind);
        bind(ret);
    });
    callable
}

/// `ty` with each pointer origin naming an explicitly bound binder of fixed
/// mutability rebound to the place it binds, below which it re-applies its
/// interior and subtree projection (see [`bind_callable_origin_tails`]).
fn bind_pointer_origin_tails(
    ty: &Ty,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
) -> Ty {
    use mojito_types::origin::{Mutability, Origin, OriginSeg, PointerOrigin};
    let recur = |ty: &Ty| bind_pointer_origin_tails(ty, tails);
    let bind = |origin: &PointerOrigin| {
        let bound = match origin {
            PointerOrigin::Param {
                id,
                mutability,
                interior,
                subtree,
            } => match (tails.get(id), mutability) {
                (Some(Origin::Place(place)), Mutability::Mutable | Mutability::Immutable) => {
                    let mut place = place.clone();
                    place.path.extend(
                        interior
                            .iter()
                            .map(|tag| OriginSeg::Interior(tag.clone()))
                            .chain(subtree.then_some(OriginSeg::Subtree)),
                    );
                    Some(PointerOrigin::Place {
                        place,
                        mutable: *mutability == Mutability::Mutable,
                    })
                }
                _ => None,
            },
            _ => None,
        };
        bound.unwrap_or_else(|| origin.clone())
    };
    match ty.kind() {
        TyKind::Pointer { element, origin } | TyKind::LitRef { element, origin } => {
            let (element, origin) = (recur(element), bind(origin));
            if matches!(ty.kind(), TyKind::LitRef { .. }) {
                TyKind::LitRef { element, origin }.into()
            } else {
                TyKind::Pointer { element, origin }.into()
            }
        }
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements.iter().map(recur).collect(),
            origin: bind(origin),
        }
        .into(),
        struct_ty if let Some((name, arguments)) = struct_ty.as_non_simd_struct() => {
            ty.reusing(TyKind::Struct(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => TyArg::Ty(recur(ty)),
                        other => other.clone(),
                    })
                    .collect(),
            ))
        }
        TyKind::Ref(reference) => TyKind::Ref(mojito_types::origin::RefTy {
            referent: recur(&reference.referent),
            origin: reference.origin.clone(),
            mutability: reference.mutability,
        })
        .into(),
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        _ => ty.clone(),
    }
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
