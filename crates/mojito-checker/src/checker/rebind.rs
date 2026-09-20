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
//! where both sides are concrete, demands that they agree. An assignment to
//! a rebound variable (`rebind[Dest](x) = value`) becomes the plain
//! assignment `x = value`, its retyping recorded at the statement's span.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_ast::visit::{MutVisitor, walk_block_mut};

/// The bodies source validation must check because they hold a `rebind`:
/// every module-level `def` body and struct method body naming the builtin,
/// keyed at the body's first statement.
///
/// A `rebind` asserts that a parametric operand type resolves to its target
/// once instantiated, so the elaborator keys specialization on it exactly as
/// on a `comptime if`: the template is stubbed and only clones are checked
/// executably. Validation is then the only place the template is judged, and
/// it must run on the source as written — `erase_rebinds` removes the calls
/// this scan looks for, so the scan precedes it.
pub fn rebind_keyed_bodies(statements: &[Stmt]) -> HashSet<SourceSpan> {
    let mut keyed = HashSet::new();
    let mut record = |body: &[Stmt]| {
        if let Some(first) = body.first()
            && block_has_rebind(body)
        {
            keyed.insert(first.source_span());
        }
    };
    for statement in statements {
        match &statement.kind {
            StmtKind::Def { body, .. } => record(body),
            StmtKind::Struct { methods, .. } => {
                for method in methods {
                    record(&method.body);
                }
            }
            _ => {}
        }
    }
    keyed
}

/// Whether `body` is one of the bodies [`rebind_keyed_bodies`] recorded.
pub fn body_keys_rebind(body: &[Stmt], keyed: &HashSet<SourceSpan>) -> bool {
    body.first()
        .is_some_and(|first| keyed.contains(&first.source_span()))
}

/// The retypings `erase_rebinds` recorded.
#[derive(Clone, Default)]
pub(super) struct RebindTargets {
    /// Each erased call's target, at its operand's span.
    operands: HashMap<SourceSpan, ParamArg>,
    /// Each rebound whole-variable assignment's target, at the span of the
    /// `Assign` statement it became.
    assignments: HashMap<SourceSpan, ParamArg>,
}

/// Replace every well-formed `rebind[Dest](value)` call by its operand and
/// return the retypings the erasure leaves behind. A malformed call (arity, a
/// transferred operand) is left for the checker to reject.
pub(super) fn erase_rebinds(statements: &mut [Stmt]) -> RebindTargets {
    struct Eraser {
        targets: RebindTargets,
    }

    impl MutVisitor for Eraser {
        // A whole variable written through a rebind is an ordinary
        // reassignment of that variable: it replaces (and destroys) the old
        // value, and a `mut` parameter writes through its handle.
        fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
            let StmtKind::SetPlace { place, .. } = &statement.kind else {
                return;
            };
            let Some((_, ExprKind::Identifier(_))) = erasable_rebind(place) else {
                return;
            };
            let StmtKind::SetPlace { place, value } =
                std::mem::replace(&mut statement.kind, StmtKind::Pass)
            else {
                unreachable!("matched a SetPlace above");
            };
            let ExprKind::Call {
                mut param_args,
                mut args,
                ..
            } = place.kind
            else {
                unreachable!("an erasable rebind is a call");
            };
            let ExprKind::Identifier(name) = args.pop().expect("one operand").kind else {
                unreachable!("matched an identifier operand above");
            };
            self.targets.assignments.insert(
                statement.source_span(),
                param_args.pop().expect("one target"),
            );
            statement.kind = StmtKind::Assign { name, value };
        }

        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if erasable_rebind(expr).is_none() {
                return;
            }
            let ExprKind::Call {
                param_args, args, ..
            } = &mut expr.kind
            else {
                unreachable!("an erasable rebind is a call");
            };
            let operand = args.pop().expect("one operand");
            let target = param_args.pop().expect("one target");
            self.targets.operands.insert(operand.source_span(), target);
            *expr = operand;
        }
    }

    let mut eraser = Eraser {
        targets: RebindTargets::default(),
    };
    walk_block_mut(&mut eraser, statements);
    eraser.targets
}

impl Checker {
    /// The type an erased `rebind` gives its operand, or the operand's own
    /// type when the span carries no retyping.
    pub(super) fn apply_rebind_target(&self, expr: &Expr, ty: Ty) -> Result<Ty, TypeError> {
        match self.rebind_targets.operands.get(&expr.source_span()) {
            Some(target) => self.rebound_ty(target, &ty, &expr.source_span()),
            None => Ok(ty),
        }
    }

    /// Reject writing through an erased `rebind` that upstream rebinds by
    /// value (see `rebinds_by_value`).
    pub(super) fn check_rebind_place(&self, place: &Expr, operand: &Ty) -> Result<(), TypeError> {
        if self
            .rebind_targets
            .operands
            .contains_key(&place.source_span())
            && self.rebinds_by_value(operand)
        {
            return Err(TypeError::ImmutableBinding(
                place_root_name(place).unwrap_or("rebind").to_string(),
            ));
        }
        Ok(())
    }

    /// The type a reassignment of `name` writes when the statement was
    /// `rebind[Dest](name) = value`: the variable's own type retyped to
    /// `Dest` (behind the handle, for a `ref` binding). A by-value rebind is
    /// not a place and is rejected as the target.
    pub(super) fn rebind_assignment_target(
        &self,
        statement: &Stmt,
        name: &str,
        target: Option<Ty>,
    ) -> Result<Option<Ty>, TypeError> {
        let Some(rebind) = self
            .rebind_targets
            .assignments
            .get(&statement.source_span())
        else {
            return Ok(target);
        };
        let Some(target) = target else {
            return Ok(None);
        };
        let rebound = |operand: &Ty| {
            if self.rebinds_by_value(operand) {
                return Err(TypeError::ImmutableBinding(name.to_string()));
            }
            self.rebound_ty(rebind, operand, &statement.source_span())
        };
        Ok(Some(match target {
            Ty::Ref(mut reference) => {
                reference.referent = Box::new(rebound(&reference.referent)?);
                Ty::Ref(reference)
            }
            other => rebound(&other)?,
        }))
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

    /// Whether upstream rebinds this operand by value. `rebind` is
    /// overloaded on its operand: a `TrivialRegisterPassable` operand is
    /// rebound by value, any other through `ref[src]`, and only the reference
    /// is a place. The overload is selected once, on the declaration, so a
    /// compiler-generated (`$`-mangled) specialization keeps the selection
    /// source validation made with the operand's type still symbolic instead
    /// of re-selecting on the concrete type it was cloned with.
    fn rebinds_by_value(&self, operand: &Ty) -> bool {
        self.is_trivial_register_passable(operand)
            && (self.source_validation
                || !self
                    .transfer_frames
                    .borrow()
                    .iter()
                    .any(|frame| frame.callable.contains('$')))
    }

    /// `Dest` for an operand of type `ty`. Under source validation the target
    /// stands in for the still-symbolic operand type; the executable check
    /// requires the two to be the same type.
    fn rebound_ty(&self, target: &ParamArg, ty: &Ty, site: &SourceSpan) -> Result<Ty, TypeError> {
        let dest = self.rebind_target_ty(target)?;
        self.rebind_assertions.borrow_mut().insert(
            site.clone(),
            mojito_checked::templates::RebindAssertion {
                operand: ty.clone(),
                dest: dest.clone(),
                by_value: self.rebinds_by_value(ty),
            },
        );
        if !self.source_validation && *ty != dest {
            return Err(TypeError::TypeMismatch {
                expected: dest.to_string(),
                found: ty.to_string(),
                context: "rebind: the input type does not match the result type".to_string(),
            });
        }
        Ok(dest)
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

/// Whether a block names `rebind[Dest](value)` anywhere below it.
fn block_has_rebind(statements: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder {
        fn visit_expr(&mut self, expr: &Expr) {
            if matches!(&expr.kind, ExprKind::TypeApply { name, .. } | ExprKind::Call { name, .. } if name == "rebind")
            {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { found: false };
    mojito_ast::visit::walk_block(&mut finder, statements);
    finder.found
}

/// The target and operand of a `rebind[Dest](value)` call the eraser removes:
/// one type argument, one borrowed operand, no keywords.
fn erasable_rebind(expr: &Expr) -> Option<(&ParamArg, &ExprKind)> {
    let ExprKind::Call {
        name,
        param_args,
        args,
        kwargs,
    } = &expr.kind
    else {
        return None;
    };
    match (param_args.as_slice(), args.as_slice()) {
        ([target], [operand])
            if name == "rebind"
                && kwargs.is_empty()
                && !matches!(operand.kind, ExprKind::Transfer(_)) =>
        {
            Some((target, &operand.kind))
        }
        _ => None,
    }
}
