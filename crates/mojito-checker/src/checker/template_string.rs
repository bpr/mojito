//! Template strings: a `t"…"` literal written as the call of the bundled
//! entry point `__make_tstring` over its interleaved segments and
//! interpolations, as upstream's `TStringExprNode` lowers one in a single
//! pass.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The linked name of the entry point a template string calls,
/// `std.format.tstring`'s `__make_tstring`, which no user spelling reaches.
pub fn template_string_entry() -> String {
    mojito_module::module::linked_item_name(TSTRING_MODULE, "__make_tstring")
}

impl Checker {
    /// Type a template string as the construction it stands for: the call
    /// `__make_tstring(String("seg"), value, …)`, named by module path and
    /// checked as any call is. A literal segment is a `String`; an
    /// interpolation is its value, except that a place whose type is not
    /// `ImplicitlyCopyable` is captured as `String(place)`, formatted at
    /// creation, so the pack the call consumes leaves the place usable.
    pub(super) fn infer_template_string(
        &self,
        expression: &Expr,
        parts: &[TStringPart],
    ) -> Result<Ty, TypeError> {
        let entry = template_string_entry();
        if self.lookup(&entry).is_none() {
            return Err(TypeError::Unsupported(format!(
                "a t-string needs the bundled '{TSTRING_MODULE}' module"
            )));
        }
        let mut ordinal = 0;
        let mut spelled = |kind, span| {
            ordinal += 1;
            initializer_list::spelled_node(expression, ordinal, kind, span)
        };
        // The linked `String`, so each segment is the construction a user's
        // `String(…)` is; a seam without the stdlib struct keeps the builtin.
        let string = if self
            .structs
            .contains_key(mojito_symbol::symbol::STDLIB_STRING_STRUCT)
        {
            mojito_symbol::symbol::STDLIB_STRING_STRUCT
        } else {
            "String"
        };
        let mut args = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                TStringPart::Literal(text) => {
                    let text = spelled(ExprKind::Str(text.clone()), expression.span);
                    args.push(spelled(string_call(string, text), expression.span));
                }
                TStringPart::Expr(value) => {
                    let ty = self.infer(value)?;
                    if !self.conforms_to(&ty, "Writable") {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "interpolation".to_string(),
                            ty: ty.to_string(),
                            trait_name: "Writable".to_string(),
                            reason: self.trait_failure_reason(&ty, "Writable"),
                        });
                    }
                    let value = value.as_ref().clone();
                    if is_place_expr(&value) && !self.is_implicitly_copyable(&ty) {
                        let span = value.span;
                        args.push(spelled(string_call(string, value), span));
                    } else {
                        args.push(value);
                    }
                }
            }
        }
        let construction = initializer_list::spelled_node(
            expression,
            0,
            ExprKind::Call {
                name: entry,
                param_args: Vec::new(),
                args,
                kwargs: Vec::new(),
            },
            expression.span,
        );
        let ty = self.infer(&construction)?;
        self.record_spelled_construction(expression, construction, &ty);
        Ok(ty)
    }
}

/// The bundled module declaring the entry point a template string calls.
const TSTRING_MODULE: &str = "std.format.tstring";

/// The conversion `String(value)`, calling `string` by its linked name.
fn string_call(string: &str, value: Expr) -> ExprKind {
    ExprKind::Call {
        name: string.to_string(),
        param_args: Vec::new(),
        args: vec![value],
        kwargs: Vec::new(),
    }
}
