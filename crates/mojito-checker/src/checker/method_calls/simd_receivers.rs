//! Method calls on `SIMD` and `DType` receivers.

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

    /// The compiler-known method of a `SIMD` value: `Copyable.copy` on a
    /// scalar or vector is the value read itself (the builtin-copy rule of
    /// the nominal resolver).
    pub(super) fn infer_simd_method(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite {
            object,
            method,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        // A name no compiler-known method answers falls through, keywords
        // and all, to the declared members (`__setitem__(i, val=…)`).
        if method != "copy" {
            return Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            });
        }
        reject_kwargs(kwargs)?;
        if !(args.is_empty() && param_args.is_empty()) {
            return Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            });
        }
        if is_place_expr(object) {
            self.record_at(object.source_span()).copy_place_value_use = true;
        }
        Ok(obj_ty.clone())
    }
}
