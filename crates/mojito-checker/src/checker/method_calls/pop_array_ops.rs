//! The `pop.array` `__mlir_op` expressions: upstream's `pop.array.repeat`,
//! `pop.array.get`, and `pop.array.replace` over a `!pop.array<n, T>`,
//! the storage `StaticTuple` builds, reads, and rebuilds, and
//! `pop.array.gep`, the address of one element through a `!kgen.pointer`
//! to that storage, as `VariadicList` and `StaticTuple._unsafe_ref` take
//! it, typed as their MLIR verifiers type them, with the `_type` and
//! `index` attributes they take.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::SemanticAdjustment;

pub(super) const POP_ARRAY_REPEAT: &str = "pop.array.repeat";
pub(super) const POP_ARRAY_GET: &str = "pop.array.get";
pub(super) const POP_ARRAY_REPLACE: &str = "pop.array.replace";
pub(super) const POP_ARRAY_GEP: &str = "pop.array.gep";

impl Checker {
    /// Type ``__mlir_op.`pop.array.<op>`[_type=T, index=i.__mlir_index__()](…)``
    /// as upstream's verifiers do, recording `SemanticAdjustment::PopArrayRepeat`,
    /// `PopArrayGet`, or `PopArrayReplace`:
    ///
    /// - `repeat(v)` is the `_type` array holding `v` in every element;
    /// - `get(a)` is element `index` of `a`, of the `_type` element type;
    /// - `replace(v, a)` is `a`, of the `_type` array type, with element
    ///   `index` replaced by `v`.
    ///
    /// The value is of the array's element type, which Mojito admits only
    /// trivially register-passable. A closed index is checked against a
    /// known size here; an index or size a template leaves open is checked
    /// by each instance (`native::mono` closes the op), as the pin's pass
    /// manager checks it.
    pub(super) fn infer_pop_array_op(
        &self,
        span: &SourceSpan,
        name: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let invalid = |reason: &str| TypeError::BadCall {
            func: name.to_string(),
            reason: reason.to_string(),
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional"));
        }
        let takes_index = name != POP_ARRAY_REPEAT;
        let (mut result_type, mut index) = (None, None);
        for argument in param_args {
            let mojito_ast::ast::ParamArg::Named {
                name: attribute,
                value,
            } = argument
            else {
                return Err(invalid("op attributes are named"));
            };
            match attribute.as_str() {
                "_type" => result_type = Some(value.as_ref()),
                "index" if takes_index => index = Some(value.as_ref()),
                _ => {
                    return Err(invalid(&format!(
                        "attribute '{attribute}' is not an inherent attribute of '{name}'"
                    )));
                }
            }
        }
        let Some(result_type) = result_type else {
            return Err(invalid(&format!(
                "unable to infer result type from MLIR operation '{name}'"
            )));
        };
        let index = match (takes_index, index) {
            (true, Some(index)) => Some(self.pop_array_index(name, index)?),
            (true, None) => return Err(invalid("op requires attribute 'index'")),
            (false, _) => None,
        };
        let arity = if name == POP_ARRAY_REPLACE { 2 } else { 1 };
        if args.len() != arity {
            return Err(invalid(&format!(
                "op expected {arity} operands, but found {}",
                args.len()
            )));
        }
        let result = self.mlir_result_type(result_type)?;
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let element_mismatch =
            || invalid("op failed to verify that value type matches array element type");
        let (array, value) = match name {
            POP_ARRAY_REPEAT => (&result, &operands[0]),
            POP_ARRAY_GET => (&operands[0], &result),
            _ => {
                if operands[1] != result {
                    return Err(invalid(
                        "op failed to verify that all of {array, result} have same type",
                    ));
                }
                (&result, &operands[0])
            }
        };
        let TyKind::PopArray { size, element } = array.kind() else {
            return Err(invalid(&format!(
                "op operand must be a '!pop.array', but got '{array}'"
            )));
        };
        if value != element {
            return Err(element_mismatch());
        }
        self.pop_array_element(name, element)?;
        if let (Some(index), Some(size)) =
            (index.as_ref().and_then(ParamExpr::as_i64), size.known())
            && u64::try_from(index).map_or(true, |index| index >= size)
        {
            return Err(invalid(&format!("op array index out of bounds: {index}")));
        }
        let adjustment = match (name, index) {
            (POP_ARRAY_GET, Some(index)) => SemanticAdjustment::PopArrayGet { index },
            (POP_ARRAY_REPLACE, Some(index)) => SemanticAdjustment::PopArrayReplace { index },
            _ => SemanticAdjustment::PopArrayRepeat,
        };
        self.record_at(span.clone()).adjustment = Some(Box::new(adjustment));
        Ok(result)
    }

    /// Type ``__mlir_op.`pop.array.gep`(p, i)`` as upstream's verifier
    /// does, recording `SemanticAdjustment::PopArrayGep`: the
    /// `!kgen.pointer<T>` to element `i` of the `!pop.array<n, T>` that the
    /// `!kgen.pointer` `p` addresses. The index is a run-time `index`
    /// operand, which the pin does not bound-check; an optional `_type`
    /// attribute must name the result the operand infers.
    pub(super) fn infer_pop_array_gep(
        &self,
        span: &SourceSpan,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let invalid = |reason: String| TypeError::BadCall {
            func: POP_ARRAY_GEP.to_string(),
            reason,
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional".to_string()));
        }
        let mut result_type = None;
        for argument in param_args {
            match argument {
                mojito_ast::ast::ParamArg::Named { name, value } if name == "_type" => {
                    result_type = Some(self.mlir_result_type(value)?);
                }
                mojito_ast::ast::ParamArg::Named { name, .. } => {
                    return Err(invalid(format!(
                        "attribute '{name}' is not an inherent attribute of '{POP_ARRAY_GEP}'"
                    )));
                }
                _ => return Err(invalid("op attributes are named".to_string())),
            }
        }
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let element = operands.first().and_then(|pointer| match pointer.kind() {
            TyKind::KgenPointer(array) => match array.kind() {
                TyKind::PopArray { element, .. } => Some(element.clone()),
                _ => None,
            },
            _ => None,
        });
        let Some(result) = element
            .clone()
            .map(|element| Ty::from(TyKind::KgenPointer(element)))
            .or_else(|| result_type.clone())
        else {
            return Err(invalid(format!(
                "unable to infer result type from MLIR operation '{POP_ARRAY_GEP}'"
            )));
        };
        let [pointer, index] = operands.as_slice() else {
            return Err(invalid(format!(
                "op expected 2 operands, but found {}",
                operands.len()
            )));
        };
        if element.is_none() {
            return Err(invalid(format!(
                "op operand #0 must be a '!kgen.pointer' to a '!pop.array', but got '{pointer}'"
            )));
        }
        if !matches!(index.kind(), TyKind::Index) {
            return Err(invalid(format!(
                "op operand #1 must be index, but got '{index}'"
            )));
        }
        if result_type.is_some_and(|expected| expected != result) {
            return Err(invalid(
                "op failed to verify that pointer to array element type".to_string(),
            ));
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(SemanticAdjustment::PopArrayGep));
        Ok(result)
    }

    /// The `index` attribute of a `pop.array` op: `i.__mlir_index__()` of
    /// a compile-time `Int` or an integer literal, an `index` value. A
    /// run-time operand is rejected as the pin rejects it.
    fn pop_array_index(
        &self,
        name: &str,
        index: &mojito_ast::ast::ParamArg,
    ) -> Result<ParamExpr, TypeError> {
        let not_index = || {
            TypeError::Unsupported(format!(
                "'{name}' takes its 'index' attribute as a compile-time 'Int.__mlir_index__()'"
            ))
        };
        let mojito_ast::ast::ParamArg::Value(
            expression @ Expr {
                kind: ExprKind::MethodCall { object, method, .. },
                ..
            },
        ) = index
        else {
            return Err(not_index());
        };
        if method != "__mlir_index__" {
            return Err(not_index());
        }
        let built = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(expression)
        };
        // An operand that types as a run-time value is the pin's dynamic
        // MLIR operand.
        let built = built.map_err(|error| match self.infer(object) {
            Ok(_) => TypeError::BadCall {
                func: name.to_string(),
                reason: "cannot use a dynamic value in MLIR magic".to_string(),
            },
            Err(_) => error,
        })?;
        if !matches!(built.meta().as_value().map(Ty::kind), Some(TyKind::Index)) {
            return Err(not_index());
        }
        Ok(built)
    }

    /// The element rule of the `pop.array` ops: a register-passable
    /// element, as the pin loads it into a register, which Mojito admits
    /// only trivially register-passable, since the pin's ops copy an
    /// element's bits and never destroy one. A type parameter bounded
    /// `RegisterPassable` is judged per instance, by the elaborator.
    fn pop_array_element(&self, name: &str, element: &Ty) -> Result<(), TypeError> {
        use super::super::traits::RegisterPassability::{Excluded, Possible, Proven};
        if matches!(element.kind(), TyKind::Index | TyKind::I1)
            || self.is_trivial_register_passable(element)
        {
            return Ok(());
        }
        match (element.kind(), self.register_passability(element)) {
            (TyKind::Param { .. }, Proven) => Ok(()),
            (TyKind::Param { .. }, Possible) | (_, Excluded) => Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "cannot load non-register passable type into SSA register".to_string(),
            }),
            _ => Err(TypeError::Unsupported(format!(
                "'{name}' over '{element}' is outside the subset: Mojito's '!pop.array' holds \
                 trivially register-passable elements"
            ))),
        }
    }
}
