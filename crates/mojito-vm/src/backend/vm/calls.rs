//! VM call-slot materialization, defaults, variadics, and frame construction.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// Const-fold a default-argument expression to a value. Handles the literal forms
/// (and a unary minus over one) that defaults use in practice; a non-constant
/// default folds to `None` and errors only if that slot is actually taken.
pub(super) fn checked_const_value(value: &CheckedConst) -> Value {
    match value {
        CheckedConst::Int(value) => Value::IntLiteral(value.clone()),
        CheckedConst::Float(value) => Value::FloatLiteral(value.clone()),
        CheckedConst::Bool(value) => Value::Bool(*value),
        CheckedConst::String(value) => Value::Str(value.clone()),
        CheckedConst::None => Value::None,
        CheckedConst::Dtype(dtype) => Value::Dtype(*dtype),
        // A `Construct` or `Evaluate` default runs code in interpreter
        // context (`bind_for_call`), so it never reaches this pure
        // value-folder.
        CheckedConst::Construct { .. } | CheckedConst::Evaluate { .. } => Value::None,
    }
}

/// Match positional + keyword arguments to a function's parameter slots, producing
/// the ordered argument values the frame binds — filling defaults and binding the
/// positional and keyword collectors the call passes whole, according to the
/// shared call contract.
pub(super) fn bind_args(
    name: &str,
    sig: &FnSig,
    argv: &[Value],
    kwargs: &[(String, Value)],
    mut make_default: impl FnMut(usize) -> Result<Value, RuntimeError>,
) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
    let kw_names: Vec<&str> = kwargs.iter().map(|(n, _)| n.as_str()).collect();
    let matched = match_call_slots(
        &sig.param_names,
        &sig.required,
        sig.positional_only,
        sig.keyword_only,
        argv.len(),
        &kw_names,
        CallVariadics {
            positional: sig.variadic.is_some(),
            keyword: sig.kw_variadic.is_some(),
        },
    )
    .map_err(|error| runtime_match_error(error, name))?;
    let (slots, overflow) = (matched.slots, matched.positional_overflow);

    let mut regular_values = Vec::with_capacity(slots.len());
    for (i, slot) in slots.iter().enumerate() {
        let value = match slot {
            ArgSlot::Positional(p) => argv[*p].clone(),
            ArgSlot::Keyword(k) => kwargs[*k].1.clone(),
            ArgSlot::Default => make_default(i)?,
        };
        regular_values.push(crate::runtime::coerce_checked(value, &sig.param_types[i]));
    }
    // The call passes the collector it builds (`tuple.make`, a homogeneous
    // one's wrapped in its `VariadicList`) as its one overflow operand, as
    // upstream's call passes the pack.
    let (out, frame_slots) = if let Some(elem_ty) = &sig.variadic {
        let packed = match (overflow.as_slice(), elem_ty.kind()) {
            ([collector], TyKind::RuntimePack(element_types)) => match &argv[*collector] {
                Value::Tuple(elements) if elements.len() != element_types.len() => {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: element_types.len(),
                        got: elements.len(),
                    });
                }
                collected @ Value::Tuple(_) => collected.clone(),
                _ => return Err(no_collector(name)),
            },
            ([collector], _)
                if matches!(argv[*collector], Value::Tuple(_) | Value::Struct { .. }) =>
            {
                argv[*collector].clone()
            }
            _ => return Err(no_collector(name)),
        };
        let idx = sig.variadic_index.unwrap_or(regular_values.len());
        let mut out = Vec::with_capacity(regular_values.len() + 1);
        out.extend(regular_values[..idx].iter().cloned());
        out.push(packed);
        out.extend(regular_values[idx..].iter().cloned());
        let mut frame_slots = Vec::with_capacity(slots.len() + 1);
        frame_slots.extend(slots[..idx].iter().copied());
        frame_slots.push(ArgSlot::Default);
        frame_slots.extend(slots[idx..].iter().copied());
        (out, frame_slots)
    } else {
        (regular_values, slots)
    };
    let (mut out, mut frame_slots) = (out, frame_slots);
    // The call passes the keyword collector it builds (`StringDict[V]()`
    // filled by `_insert`), or the one it forwards (`**kwargs^`), as its one
    // keyword overflow operand, as upstream's call passes the dictionary.
    if let Some(index) = sig.kw_variadic_index {
        let [collector] = matched.keyword_overflow.as_slice() else {
            return Err(no_keyword_collector(name));
        };
        let (keyword, value) = &kwargs[*collector];
        if keyword != mojito_ast::ast::FORWARDED_KWARGS_NAME {
            return Err(no_keyword_collector(name));
        }
        out.insert(index, value.clone());
        frame_slots.insert(index, ArgSlot::Default);
    }
    Ok((out, frame_slots))
}

/// Build a struct instance (fieldwise), coercing each argument to its field type.
pub(super) fn construct(
    def: &StructDef,
    name: &str,
    args: Vec<Value>,
) -> Result<Value, RuntimeError> {
    if !def.fieldwise_init {
        return Err(RuntimeError::TypeError(format!(
            "struct '{name}' has no constructor"
        )));
    }
    struct_value(def, name, args)
}

/// The struct value `name` of one value per field, in declaration order,
/// each coerced to its field type, running no constructor: a fieldwise
/// construction, or a `struct.create`.
pub(super) fn struct_value(
    def: &StructDef,
    name: &str,
    args: Vec<Value>,
) -> Result<Value, RuntimeError> {
    if def.fields.len() != args.len() {
        return Err(RuntimeError::ArityMismatch {
            name: name.to_string(),
            expected: def.fields.len(),
            got: args.len(),
        });
    }
    let fields = def
        .fields
        .iter()
        .zip(args)
        .map(|((fname, fty), arg)| (fname.clone(), crate::runtime::coerce_checked(arg, fty)))
        .collect();
    Ok(Value::Struct {
        name: name.to_string(),
        fields,
    })
}

/// A call of `function`, which collects a positional pack, that passes no
/// collector: MIR lowering builds one at every call.
fn no_collector(function: &str) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "a call of '{function}' passes no positional collector"
    ))
}

fn no_keyword_collector(function: &str) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "a call of '{function}' passes no keyword collector"
    ))
}

fn runtime_match_error(error: mojito_ast::call::MatchError, function: &str) -> RuntimeError {
    use mojito_ast::call::MatchError;
    match error {
        MatchError::TooManyPositional { expected, got } => RuntimeError::ArityMismatch {
            name: function.to_string(),
            expected,
            got,
        },
        MatchError::UnknownKeyword(keyword) => RuntimeError::TypeError(format!(
            "'{function}' got an unexpected keyword argument '{keyword}'"
        )),
        MatchError::PositionalOnly(keyword) => RuntimeError::TypeError(format!(
            "'{function}' got positional-only argument '{keyword}' passed as keyword"
        )),
        MatchError::Duplicate(keyword) => RuntimeError::TypeError(format!(
            "'{function}' got argument '{keyword}' more than once"
        )),
        MatchError::Missing(parameter) => RuntimeError::TypeError(format!(
            "'{function}' missing required argument '{parameter}'"
        )),
        // The checker rejects each of these shapes before a call reaches
        // the VM.
        spread @ (MatchError::RepeatedSpread
        | MatchError::PositionalAfterSpread
        | MatchError::SpreadOutsideVariadic
        | MatchError::KeywordsBesideForward) => {
            RuntimeError::TypeError(spread.into_type_error(function).to_string())
        }
    }
}
