//! The `__mlir_op` forms Mojito admits: upstream's
//! `lit.ownership.mark_initialized` and `lit.ownership.mark_destroyed` over a
//! place, which the bundled standard library spells before writing a value's
//! storage through pointers and after moving it out through them.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Type the `__mlir_op` statement `lit.ownership.mark_initialized` or
    /// `lit.ownership.mark_destroyed` over `__get_mvalue_as_litref(place)`.
    /// The first marks `place` initialized, so a constructor may write its
    /// storage through pointers (`Tuple.__init__`); the second ends its
    /// value without destroying it, after its storage was moved out through
    /// pointers (`Tuple.consume_elements`). Each is accepted only in the
    /// bundled modules that reach compiler-private storage; every other
    /// `__mlir_op` spelling, and these elsewhere, is rejected. The
    /// statement's checked record is `SemanticAdjustment::MarkInitialized`
    /// or `SemanticAdjustment::MarkDestroyed`.
    pub(super) fn infer_mlir_op(
        &self,
        span: &SourceSpan,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        const INITIALIZED: &str = "lit.ownership.mark_initialized";
        const DESTROYED: &str = "lit.ownership.mark_destroyed";
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let rejected = || {
            TypeError::Unsupported(format!(
                "'__mlir_op' is accepted only as '{INITIALIZED}' or '{DESTROYED}' over \
                 '__get_mvalue_as_litref(place)' in the bundled standard library"
            ))
        };
        if !matches!(method, INITIALIZED | DESTROYED)
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
            if method == INITIALIZED {
                mojito_checked::checked::SemanticAdjustment::MarkInitialized
            } else {
                mojito_checked::checked::SemanticAdjustment::MarkDestroyed
            },
        );
        Ok(Ty::None)
    }
}
