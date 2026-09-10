//! Constructor origin binding: the struct origin binders a hand-written
//! `__init__`'s parameters name — a `Pointer[Self.T, Self.origin]` parameter
//! (upstream `Span(unsafe_ptr=, length=)`) or a `ref [Self.origin]`
//! parameter — bound from the call's arguments and checked against an
//! explicitly applied origin (`Span[Byte, origin_of(self)](...)`).

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::origin::{Origin, OriginParamId, PointerOrigin};

impl Checker {
    /// Bind the struct origin binders the selected constructor's pointer
    /// parameters name from the arguments filling those slots, and check
    /// every slot naming an explicitly applied origin against its argument.
    ///
    /// `bound` pairs each argument expression with the parameter index it
    /// fills and that parameter's declared (unsubstituted) type; `arg_tys`
    /// runs parallel to it. The result maps a struct binder to the pointer
    /// provenance the call binds it to, for substitution into the parameter
    /// types before coercion ([`substitute_pointer_origin_params`]).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn bind_constructor_origins(
        &self,
        struct_name: &str,
        callee: &str,
        source_params: &[mojito_ast::ast::TypeParam],
        sig: &MethodSig,
        bound: &[(usize, &Expr, &Ty)],
        arg_tys: &[Ty],
        explicit: &[super::super::type_resolution::ExplicitStructOrigin],
    ) -> Result<HashMap<OriginParamId, PointerOrigin>, TypeError> {
        let mut bindings: HashMap<OriginParamId, PointerOrigin> = HashMap::new();
        for ((index, expression, pattern), actual) in bound.iter().zip(arg_tys) {
            let parameter = sig.names.get(*index).map_or("?", String::as_str);
            let context = || format!("argument '{parameter}' to '{struct_name}.{callee}'");
            if let Ty::Pointer {
                origin:
                    PointerOrigin::Param {
                        id,
                        interior,
                        subtree,
                        ..
                    },
                ..
            } = pattern
                && (id.0 as usize) < source_params.len()
                && let Ty::Pointer {
                    origin: actual_origin,
                    ..
                } = actual
            {
                let slot = &source_params[id.0 as usize];
                let requires_mut = matches!(
                    slot.origin_mutability.as_ref().map(|e| &e.kind),
                    Some(ExprKind::Bool(true))
                );
                if requires_mut && actual_origin.statically_mutable() == Some(false) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a mutable-origin pointer for parameter '{}' of '{struct_name}'",
                            slot.name
                        ),
                        found: "an immutable-origin pointer".to_string(),
                        context: context(),
                    });
                }
                if let Some(explicit) = explicit.iter().find(|origin| origin.id == *id) {
                    let within = match actual_origin.as_origin() {
                        Some(actual) => origin_is_within(&actual, &explicit.origin),
                        None => matches!(explicit.origin, Origin::Untracked { .. }),
                    };
                    if !within {
                        return Err(TypeError::TypeMismatch {
                            expected: format!(
                                "a pointer whose origin lies within the supplied '{}' argument",
                                slot.name
                            ),
                            found: format!("a pointer of origin {actual_origin:?}"),
                            context: context(),
                        });
                    }
                }
                // A projected parameter origin (`Self.origin._get_owned_interior[..]`)
                // names a domain below the binder; it is checked, not bound.
                if !interior.is_empty() || *subtree {
                    continue;
                }
                match bindings.get(id) {
                    None => {
                        bindings.insert(*id, actual_origin.clone());
                    }
                    Some(existing) if existing == actual_origin => {}
                    Some(_) => {
                        return Err(TypeError::BadCall {
                            func: struct_name.to_string(),
                            reason: format!(
                                "arguments bind conflicting origins for parameter '{}'",
                                slot.name
                            ),
                        });
                    }
                }
                continue;
            }
            if let Some(Some(PointerOrigin::Param { id, .. })) = sig.origin_binders.get(*index)
                && let Some(explicit) = explicit.iter().find(|origin| origin.id == *id)
            {
                let actual = self.materialized_reference_actual(expression)?;
                if !origin_is_within(&actual.origin, &explicit.origin) {
                    let slot = &source_params[id.0 as usize];
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a reference whose origin lies within the supplied '{}' argument",
                            slot.name
                        ),
                        found: format!("a reference of origin {:?}", actual.origin),
                        context: context(),
                    });
                }
            }
        }
        Ok(bindings)
    }
}
