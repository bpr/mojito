//! The `__mlir_op` expressions over a `!kgen.pointer` or a `!lit.ref` that
//! upstream's `struct Pointer` bodies are written in
//! ([`mojito_types::mlir::PointerOp`]): `lit.ref.to_pointer` and
//! `lit.ref.from_pointer` between a `!lit.ref` and a `!kgen.pointer`,
//! `pop.pointer.bitcast`, `pop.offset`, `pop.pointer_to_index`, and the
//! `pop.load` and `pop.store` of a register-passable element, typed as
//! their MLIR verifiers type them, with the attributes they take.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::mlir::PointerOp;

impl Checker {
    /// Type ``__mlir_op.`name`[attributes](operands)`` for the pointer op
    /// `op`, recording it as `SemanticAdjustment::MlirPointerOp`.
    /// `lit.ref.to_pointer` takes a `!lit.ref<T, o>` to the
    /// `!kgen.pointer<T>` it addresses; `lit.ref.from_pointer` takes a
    /// `!kgen.pointer<T>` back to the `!lit.ref<T, o>` its `_type` names,
    /// with a `!kgen.string` `name`; `pop.pointer.bitcast` retypes a
    /// `!kgen.pointer` to the one its `_type` names; `pop.offset` moves one
    /// by a `!kgen.scalar<index>` count; `pop.pointer_to_index` reads its
    /// address as a `!kgen.scalar<index>`; `pop.load` reads its
    /// register-passable element, a bitwise copy that yields an owned
    /// value; and `pop.store` writes the bits of a value of its element
    /// through it, leaving its operand to be destroyed. The memory ops' `alignment` and flags are
    /// compile-time attributes; a volatile access is outside the subset.
    pub(super) fn infer_pointer_op(
        &self,
        span: &SourceSpan,
        op: PointerOp,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let name = op.name();
        let invalid = |reason: String| TypeError::BadCall {
            func: name.to_string(),
            reason,
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional".to_string()));
        }
        let mut attributes: Vec<(&str, &ParamArg)> = Vec::new();
        for argument in param_args {
            let ParamArg::Named {
                name: attribute,
                value,
            } = argument
            else {
                return Err(invalid("op attributes are named".to_string()));
            };
            if !op.attributes().iter().any(|(known, _)| known == attribute) {
                return Err(invalid(format!(
                    "attribute '{attribute}' is not an inherent attribute of '{name}'"
                )));
            }
            attributes.push((attribute.as_str(), value.as_ref()));
        }
        let attribute = |wanted: &str| {
            attributes
                .iter()
                .find_map(|(attribute, value)| (*attribute == wanted).then_some(*value))
        };
        if let Some((missing, _)) = op
            .attributes()
            .iter()
            .find(|(attribute_name, required)| *required && attribute(attribute_name).is_none())
        {
            return Err(invalid(format!("op requires attribute '{missing}'")));
        }
        if args.len() != op.arity() {
            return Err(invalid(format!(
                "op takes {} operand(s), got {}",
                op.arity(),
                args.len()
            )));
        }
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let operand_must = |index: usize, expected: &str| {
            invalid(format!(
                "operand #{index} must be {expected}, but got '{}'",
                operands[index]
            ))
        };
        let pointer_element = |index: usize| match operands[index].kind() {
            TyKind::KgenPointer { element, .. } => Ok(element.clone()),
            _ => Err(operand_must(index, "a '!kgen.pointer'")),
        };
        let offset_ty = mojito_types::types::storage_ty(&Ty::int());
        let result = match op {
            PointerOp::LitRefToPointer => match operands[0].kind() {
                TyKind::LitRef(reference) => {
                    Ty::kgen_pointer(reference.referent.clone(), reference.address_space.clone())
                }
                _ => return Err(operand_must(0, "a '!lit.ref'")),
            },
            PointerOp::LitRefFromPointer => {
                let element = pointer_element(0)?;
                let result = self.mlir_result_type(attribute("_type").expect("required"))?;
                let TyKind::LitRef(mojito_types::origin::RefTy { referent, .. }) = result.kind()
                else {
                    return Err(invalid(format!(
                        "result type must be a '!lit.ref', but got '{result}'"
                    )));
                };
                if !same_element(referent, &element)
                    || result.kind().pointer_address_space()
                        != operands[0].kind().pointer_address_space()
                {
                    return Err(invalid(format!(
                        "result type '{result}' does not reference the element of '{}'",
                        operands[0]
                    )));
                }
                self.check_lit_ref_name(name, attribute("name").expect("required"))?;
                result
            }
            PointerOp::Bitcast => {
                pointer_element(0)?;
                let result = self.mlir_result_type(attribute("_type").expect("required"))?;
                if !matches!(result.kind(), TyKind::KgenPointer { .. }) {
                    return Err(invalid(format!(
                        "result type must be a '!kgen.pointer', but got '{result}'"
                    )));
                }
                result
            }
            PointerOp::Offset => {
                pointer_element(0)?;
                if operands[1] != offset_ty {
                    return Err(operand_must(1, &offset_ty.to_string()));
                }
                operands[0].clone()
            }
            PointerOp::PointerToIndex => {
                pointer_element(0)?;
                offset_ty
            }
            PointerOp::Load => {
                let element = pointer_element(0)?;
                self.check_register_passable_element(op, &element)?;
                element
            }
            PointerOp::Store => {
                let element = pointer_element(1)?;
                if !same_element(&operands[0], &element) {
                    return Err(invalid(format!(
                        "operand #0 of type '{}' is not the element of '{}'",
                        operands[0], operands[1]
                    )));
                }
                self.check_register_passable_element(op, &element)?;
                self.check_stored_operand_destroyed(&operands[0])?;
                Ty::none()
            }
        };
        if let Some(alignment) = attribute("alignment") {
            self.memory_attribute(name, "alignment", "__mlir_index__", alignment)?;
        }
        for flag in ["isInvariant", "isNonTemporal"] {
            if let Some(value) = attribute(flag) {
                self.memory_attribute(name, flag, "__mlir_i1__", value)?;
            }
        }
        if let Some(value) = attribute("isVolatile") {
            let volatile = self.memory_attribute(name, "isVolatile", "__mlir_i1__", value)?;
            self.require_not_volatile(span, name, &volatile)?;
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::MlirPointerOp { op },
        ));
        Ok(result)
    }

    /// Check a `pop.load` or `pop.store` element: upstream returns and
    /// takes only a register-passable value from an `__mlir_op`, a scalar,
    /// a pointer, or any `RegisterPassable` struct, whose bits the access
    /// copies.
    fn check_register_passable_element(
        &self,
        op: PointerOp,
        element: &Ty,
    ) -> Result<(), TypeError> {
        let pointer = element.is_pointer() || matches!(element.kind(), TyKind::KgenPointer { .. });
        if pointer
            || self.register_passability(element) == super::traits::RegisterPassability::Proven
        {
            return Ok(());
        }
        let reason = if op == PointerOp::Load {
            format!(
                "'{element}' cannot be returned directly from __mlir_op as it is not a \
                 'RegisterPassable' type"
            )
        } else {
            format!("cannot load non-register passable type '{element}' into SSA register")
        };
        Err(TypeError::BadCall {
            func: op.name().to_string(),
            reason,
        })
    }

    /// Check that `pop.store`'s operand can be destroyed: the op copies
    /// the operand's bits and leaves the owned operand behind, which is
    /// destroyed after the op, so a type the body cannot implicitly destroy
    /// is abandoned, as upstream reports it.
    fn check_stored_operand_destroyed(&self, operand: &Ty) -> Result<(), TypeError> {
        let Some(linear) = self.destruction_verdict(operand) else {
            return Ok(());
        };
        let (var, message) = ("(anonymous value)".to_string(), linear.message);
        Err(if linear.archetype {
            TypeError::LinearAbandoned { var, message }
        } else {
            TypeError::Abandoned { var, message }
        })
    }

    /// Check `lit.ref.from_pointer`'s `name` attribute, a `!kgen.string`
    /// (`"(pointer pointee)".value`).
    fn check_lit_ref_name(&self, name: &str, value: &ParamArg) -> Result<(), TypeError> {
        let ParamArg::Value(expression) = value else {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "attribute 'name' takes a '!kgen.string'".to_string(),
            });
        };
        let found = self.infer(expression)?;
        if matches!(found.kind(), TyKind::KgenString) {
            return Ok(());
        }
        Err(TypeError::BadCall {
            func: name.to_string(),
            reason: format!("attribute 'name' takes a '!kgen.string', but got '{found}'"),
        })
    }

    /// Check a memory op's `alignment` or flag attribute, a compile-time
    /// `Int`'s `__mlir_index__()` or `Bool`'s `__mlir_i1__()` (`method`),
    /// as upstream's `unsafe_load` spells them, returning its value.
    fn memory_attribute(
        &self,
        name: &str,
        attribute: &str,
        method: &str,
        value: &ParamArg,
    ) -> Result<ParamExpr, TypeError> {
        let object = match value {
            ParamArg::Value(Expr {
                kind:
                    ExprKind::MethodCall {
                        object,
                        method: called,
                        args,
                        kwargs,
                    },
                ..
            }) if called == method && args.is_empty() && kwargs.is_empty() => object,
            _ => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' takes its '{attribute}' attribute as a compile-time '{}.{method}()'",
                    if method == "__mlir_i1__" {
                        "Bool"
                    } else {
                        "Int"
                    }
                )));
            }
        };
        let built = self.compile_dependent_ct_expr(object)?;
        let typed = built.meta().as_value().is_some_and(|ty| {
            if method == "__mlir_i1__" {
                ty.is_bool()
            } else {
                ty.is_int()
            }
        });
        if !typed {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: format!(
                    "attribute '{attribute}' takes a compile-time '{}'",
                    if method == "__mlir_i1__" {
                        "Bool"
                    } else {
                        "Int"
                    }
                ),
            });
        }
        Ok(built)
    }

    /// A volatile access must stay one access in program order, which
    /// neither backend promises: a closed `isVolatile` must be false, and a
    /// symbolic one is asserted false in each instance.
    fn require_not_volatile(
        &self,
        span: &SourceSpan,
        name: &str,
        volatile: &ParamExpr,
    ) -> Result<(), TypeError> {
        let message = format!("a volatile '{name}' is outside the subset");
        match volatile.as_constant() {
            Some(CtValue::Bool(false)) => Ok(()),
            Some(_) => Err(TypeError::Unsupported(message)),
            None => {
                let context = &self.param_context;
                let value = context
                    .constant(CtValue::Bool(false))
                    .and_then(|absent| context.infix(InfixOp::Eq, volatile, &absent))
                    .map_err(|error| TypeError::Unsupported(error.to_string()))?;
                self.record_instance_constraint(span, value, &message);
                Ok(())
            }
        }
    }
}

/// Whether `found` and `element` are one element type, origins aside.
fn same_element(found: &Ty, element: &Ty) -> bool {
    found == element
        || mojito_types::types::erase_origins(found) == mojito_types::types::erase_origins(element)
}
