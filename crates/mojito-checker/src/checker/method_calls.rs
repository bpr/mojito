//! Method-call type inference: `infer_method_call` dispatch by receiver
//! family, signature resolution and overload scoring, call-boundary
//! snapshots/adjustments, receiver and argument effects, the checked call
//! contract, static- and pointer-method inference, struct dunder resolution,
//! and List/Tuple method inference. See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

mod builtin_types;
mod call_contract;
mod intrinsic_receivers;
mod mc_infer;
mod mlir_op;
mod receiver_effects;
mod resolution;
mod selection;
mod simd_receivers;
mod statics;
mod type_receivers;

use call_contract::SelectedMethodCall;
pub use intrinsic_receivers::INTRINSIC_COLLECTOR_METHODS;

/// One value-receiver method call as the receiver families see it: the call
/// expression's span, the receiver and its inferred type, and the arguments.
#[derive(Clone, Copy)]
struct MethodCallSite<'a> {
    span: &'a SourceSpan,
    object: &'a Expr,
    method: &'a str,
    call: MethodCallArguments<'a>,
    obj_ty: &'a Ty,
}

/// A receiver family's answer: the selected signature, `None` when the
/// receiver declares no method of that name, or why selection failed.
type MethodSelection = Result<Option<MethodCallResolution>, OverloadSelect>;
