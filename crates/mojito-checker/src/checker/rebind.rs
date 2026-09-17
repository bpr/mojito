//! The `rebind[Dest](value)` builtin: upstream's static assertion that a
//! parametric operand type resolves to `Dest` once instantiated, yielding the
//! operand itself at that type. Extracted from `checker.rs`; see
//! `docs/symbol-map.md`.
//!
//! The call is erased in place before checking (`erase_rebinds`): the operand
//! keeps its own node, so every place, origin, and transfer rule reads it as
//! the value it is, and lowering never sees a call. What remains is a
//! retyping recorded at the operand's span: source validation, where the
//! operand's type is still symbolic, takes `Dest` on faith (the operand's
//! bounds prove nothing about `Dest`, as upstream); the executable check,
//! where both sides are concrete, demands that they agree.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_ast::visit::{MutVisitor, walk_block_mut};

/// Replace every well-formed `rebind[Dest](value)` call by its operand and
/// return the retyping each operand's span now carries. A malformed call
/// (arity, a transferred operand) is left for the checker to reject.
pub(super) fn erase_rebinds(statements: &mut [Stmt]) -> HashMap<SourceSpan, ParamArg> {
    struct Eraser {
        targets: HashMap<SourceSpan, ParamArg>,
    }

    impl MutVisitor for Eraser {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            let ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } = &mut expr.kind
            else {
                return;
            };
            if name != "rebind"
                || !kwargs.is_empty()
                || param_args.len() != 1
                || args.len() != 1
                || matches!(args[0].kind, ExprKind::Transfer(_))
            {
                return;
            }
            let operand = args.pop().expect("one operand");
            let target = param_args.pop().expect("one target");
            self.targets.insert(operand.source_span(), target);
            *expr = operand;
        }
    }

    let mut eraser = Eraser {
        targets: HashMap::new(),
    };
    walk_block_mut(&mut eraser, statements);
    eraser.targets
}

impl Checker {
    /// The type an erased `rebind` gives its operand, or the operand's own
    /// type when the span carries no retyping. Under source validation the
    /// target stands in for the still-symbolic operand type; the executable
    /// check requires the two to be the same type.
    pub(super) fn apply_rebind_target(&self, expr: &Expr, ty: Ty) -> Result<Ty, TypeError> {
        let Some(target) = self.rebind_targets.get(&expr.source_span()).cloned() else {
            return Ok(ty);
        };
        let dest = self.rebind_target_ty(&target)?;
        if !self.source_validation && ty != dest {
            return Err(TypeError::TypeMismatch {
                expected: dest.to_string(),
                found: ty.to_string(),
                context: "rebind: the input type does not match the result type".to_string(),
            });
        }
        Ok(dest)
    }

    /// Reject writing through an erased `rebind` that upstream rebinds by
    /// value. `rebind` is overloaded on its operand: a
    /// `TrivialRegisterPassable` operand is rebound by value, any other
    /// through `ref[src]`, and only the reference is a place. The overload is
    /// selected once, on the declaration, so a compiler-generated (`$`-mangled)
    /// specialization keeps the selection source validation made with the
    /// operand's type still symbolic instead of re-selecting on the concrete
    /// type it was cloned with.
    pub(super) fn check_rebind_place(&self, place: &Expr, operand: &Ty) -> Result<(), TypeError> {
        if !self.rebind_targets.contains_key(&place.source_span())
            || !self.is_trivial_register_passable(operand)
        {
            return Ok(());
        }
        let generated_body = !self.source_validation
            && self
                .transfer_frames
                .borrow()
                .iter()
                .any(|frame| frame.callable.contains('$'));
        if generated_body {
            return Ok(());
        }
        Err(TypeError::ImmutableBinding(
            place_root_name(place).unwrap_or("rebind").to_string(),
        ))
    }

    /// The rejection for a `rebind` call the eraser left in place.
    pub(super) fn rebind_shape_error(param_args: &[ParamArg], args: &[Expr]) -> TypeError {
        if param_args.len() != 1 {
            return TypeError::WrongTypeArgCount {
                name: "rebind".to_string(),
                expected: 1,
                got: param_args.len(),
            };
        }
        if args.len() != 1 {
            return TypeError::ArityMismatch {
                name: "rebind".to_string(),
                expected: 1,
                got: args.len(),
            };
        }
        TypeError::Unsupported(
            "rebind takes its operand by reference; transfer the rebound value rather than the \
             operand"
                .to_string(),
        )
    }

    fn rebind_target_ty(&self, target: &ParamArg) -> Result<Ty, TypeError> {
        let unsupported = || {
            TypeError::Unsupported(
                "rebind takes one type argument (`rebind[Dest](value)`)".to_string(),
            )
        };
        let annotation = match target {
            ParamArg::Type(annotation) => annotation.clone(),
            // A bare or subscripted type name parses as a value argument.
            ParamArg::Value(expr) => {
                super::constraints::assoc_body_source_type(expr).map_err(|_| unsupported())?
            }
            ParamArg::Named { .. } => return Err(unsupported()),
        };
        self.ty_from_anno(&annotation)
    }
}
