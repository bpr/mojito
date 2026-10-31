//! Method calls on `DType` receivers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// `DType`'s `Bool` queries (`is_integral()`, `is_floating_point()`, …).
    pub(super) fn infer_dtype_predicate(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite { method, call, .. } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        reject_kwargs(kwargs)?;
        if !param_args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: format!("DType.{method}"),
                expected: 0,
                got: param_args.len(),
            });
        }
        self.builtin_args(&format!("DType.{method}"), 0, args)?;
        Ok(Ty::bool())
    }
}
