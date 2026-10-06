//! The one `__mlir_op` form Mojito admits: upstream's
//! `lit.ownership.mark_initialized` over a place, which the bundled
//! standard library spells before writing a value's storage through pointers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Type the `__mlir_op` statement `lit.ownership.mark_initialized` over
    /// `__get_mvalue_as_litref(place)`: it marks `place` initialized, so a
    /// constructor may write its storage through pointers (`Tuple.__init__`).
    /// It is accepted only in the
    /// bundled modules that reach compiler-private storage; every other
    /// `__mlir_op` spelling, and this one elsewhere, is rejected. The
    /// statement's checked record is `SemanticAdjustment::MarkInitialized`.
    pub(super) fn infer_mlir_op(
        &self,
        span: &SourceSpan,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        const ADMITTED: &str = "lit.ownership.mark_initialized";
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let rejected = || {
            TypeError::Unsupported(format!(
                "'__mlir_op' is accepted only as '{ADMITTED}' over '__get_mvalue_as_litref(place)' \
                 in the bundled standard library"
            ))
        };
        if method != ADMITTED
            || !param_args.is_empty()
            || !kwargs.is_empty()
            || !super::super::overload_support::is_bundled_private_storage_source(
                span.source.as_deref(),
            )
        {
            return Err(rejected());
        }
        let [
            Expr {
                kind:
                    ExprKind::Call {
                        name,
                        param_args,
                        args,
                        kwargs,
                    },
                ..
            },
        ] = args
        else {
            return Err(rejected());
        };
        let [place] = args.as_slice() else {
            return Err(rejected());
        };
        if name != "__get_mvalue_as_litref" || !param_args.is_empty() || !kwargs.is_empty() {
            return Err(rejected());
        }
        self.origin_place(place).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => rejected(),
        })?;
        self.infer(place)?;
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            mojito_checked::checked::SemanticAdjustment::MarkInitialized,
        );
        Ok(Ty::None)
    }
}
