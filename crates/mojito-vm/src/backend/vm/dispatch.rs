//! Named-call dispatch, reference-carry queries, drops, slicing
//! bounds, and value formatting.

use super::*;

impl VmBackend {
    /// Whether a function's result can hold reference handles into its
    /// receiver: it returns a reference directly, or its checked return type
    /// structurally contains a `ref` field. Such a method's borrowed receiver
    /// must enter as a caller-place handle rather than a value copy.
    pub(super) fn function_result_carries_reference(prog: &Prog, index: usize) -> bool {
        let function = &prog.mir.functions[index].1;
        if function.returns_reference {
            return true;
        }
        let Some(ret) = &function.ret_ty else {
            return false;
        };
        let mut visited = std::collections::HashSet::new();
        Self::type_carries_reference_handle(prog, ret, &mut visited)
    }

    /// Structural reference-content test over MIR struct declarations, with a
    /// visited set for recursive aggregates. Conservative `false` for generic
    /// parameters and non-aggregate types: pointer-backed views (`Span`) copy
    /// by value safely; only stored `ref` handles are frame-rooted.
    pub(super) fn type_carries_reference_handle(
        prog: &Prog,
        ty: &Ty,
        visited: &mut std::collections::HashSet<String>,
    ) -> bool {
        match ty {
            Ty::Ref(_) => true,
            Ty::Struct(name, _) => {
                visited.insert(name.clone())
                    && prog.structs.get(name).is_some_and(|declaration| {
                        declaration.fields.iter().any(|(_, field_ty)| {
                            Self::type_carries_reference_handle(prog, field_ty, visited)
                        })
                    })
            }
            Ty::Tuple(items) | Ty::RuntimePack(items) | Ty::Variant(items) => items
                .iter()
                .any(|item| Self::type_carries_reference_handle(prog, item, visited)),
            _ => false,
        }
    }

    /// Recursively destroy a value (ASAP drop): run a struct's `__deinit__` if it
    /// defines one, then drop its fields in reverse declaration order. Internal
    /// tuple/compile-time storage recurses through its elements. Scalars are a
    /// no-op; a destructor-less struct still recursively destroys its fields.
    pub(super) fn drop_value(&mut self, prog: &Prog, v: Value) -> Result<(), RuntimeError> {
        match v {
            Value::Struct { name, fields, .. } => {
                // A partial aggregate cannot run its whole-value destructor:
                // that method may inspect a field already transferred or
                // explicitly destroyed. Drop only the initialized residual
                // fields. Checked explicit-destroy obligations guarantee that
                // an intact linear value never reaches an automatic DropVar, so
                // this rule does not need to reconstruct generic conditional
                // deletability from the erased runtime struct name.
                if fields
                    .iter()
                    .any(|(_, value)| matches!(value, Value::Moved))
                {
                    for (_, field) in fields.into_iter().rev() {
                        if !matches!(field, Value::Moved) {
                            self.drop_value(prog, field)?;
                        }
                    }
                    return Ok(());
                }
                let del = format!("{name}.__deinit__");
                if let Some(idx) = prog.index_of(&del) {
                    self.record_lifecycle(format!("drop {name}"));
                    // `self` is the whole struct; the return value is discarded.
                    let self_val = Value::Struct {
                        name: name.clone(),
                        fields: fields.clone(),
                        value_params: Vec::new(),
                    };
                    self.call_function(prog, idx, vec![self_val], &[])?;
                }
                for (_, fv) in fields.into_iter().rev() {
                    self.drop_value(prog, fv)?;
                }
            }
            Value::ComptimeList(items) => {
                for item in items.into_iter().rev() {
                    self.drop_value(prog, item)?;
                }
            }
            // Private heterogeneous pack storage follows Mojo's element
            // destruction order (left-to-right). Public Tuple is the nominal
            // one-field wrapper handled by the struct branch above.
            Value::Tuple(items) => {
                for item in items {
                    self.drop_value(prog, item)?;
                }
            }
            Value::Variant { value, .. } => self.drop_value(prog, *value)?,
            Value::Closure { captures, .. } => {
                for capture in captures.into_iter().rev() {
                    if capture.owned && !matches!(capture.value, Value::Moved) {
                        self.drop_value(prog, capture.value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Materialize one intrinsic Slice bound through the ordinary bundled
    /// `Optional[Int]` constructor. The linker may keep Optional module-qualified,
    /// so accept its unique nominal suffix but never guess between declarations.
    pub(super) fn slice_bound_optional(
        &mut self,
        prog: &Prog,
        bound: Option<i64>,
    ) -> Result<Value, RuntimeError> {
        let name = if prog.structs.contains_key("Optional") {
            "Optional".to_string()
        } else {
            let mut candidates = prog
                .structs
                .keys()
                .filter(|name| name.ends_with("$Optional"));
            let name = candidates.next().cloned().ok_or_else(|| {
                RuntimeError::Unsupported(
                    "vm: Slice bound access requires the nominal Optional declaration".to_string(),
                )
            })?;
            if candidates.next().is_some() {
                return Err(RuntimeError::Unsupported(
                    "vm: Slice bound access found ambiguous nominal Optional declarations"
                        .to_string(),
                ));
            }
            name
        };
        let arguments = bound
            .map(|value| vec![Value::Int(value)])
            .unwrap_or_default();
        // Optional's overloaded constructors include keyword-only forms
        // (`init_with=`, `copy:`) and the `NoneType` conversion constructor at
        // the same arity as the positional value constructor, so arity-based
        // overload selection is ambiguous here. Select the unique positional
        // (non-`$kw$`) overload explicitly; a bound is an `Int`, never `None`,
        // so the `NoneType` overload is excluded too.
        let init = format!("{name}.__init__");
        let expected_params = arguments.len() + 1;
        let mut constructors = prog.mir.functions.iter().filter(|(fname, function)| {
            mojito_symbol::symbol::is_overload_of(fname, &init)
                && function.n_params == expected_params
                && !fname.contains("$kw$")
                && !mojito_symbol::symbol::is_none_overload(fname)
        });
        let target = constructors.next().map(|(fname, _)| fname.clone());
        if let Some(target) = target
            && constructors.next().is_none()
        {
            return self.construct_via_init(prog, &name, Some(&target), arguments, Vec::new(), &[]);
        }
        self.call_named(prog, &name, arguments, Vec::new(), &[])
    }

    /// Normalize an `Indexer` to the VM's signed index representation. Int-like
    /// values take the intrinsic path; user conformers execute
    /// `__mlir_index__`, which is the source-level contract even though MIR
    /// represents its result as an `Int` rather than an MLIR index type.
    pub(super) fn normalize_index(
        &mut self,
        prog: &Prog,
        value: &Value,
    ) -> Result<i64, RuntimeError> {
        if let Value::Struct { name, .. } = value {
            let normalized = self.call_dunder(prog, name, "__mlir_index__", vec![value.clone()])?;
            value_as_index(&normalized)
        } else {
            value_as_index(value)
        }
    }

    /// Dispatch a call by name: a built-in intrinsic, a struct constructor, or a
    /// user function (with default/keyword/`*args` slot-matching). `param_vals`
    /// holds the supplied compile-time value-parameter arguments (`Name[...](...)`),
    /// used to reify a constructed struct's `value_params`.
    pub(super) fn call_named(
        &mut self,
        prog: &Prog,
        name: &str,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        param_vals: &[Option<Value>],
    ) -> Result<Value, RuntimeError> {
        // Built-ins take positional arguments only, and user functions handle
        // keywords through their signatures below. Struct constructors get a
        // narrow exception for Mojo's lifecycle copy constructor (`copy:`).
        if !kwargs.is_empty() && !prog.sigs.contains_key(name) && !prog.structs.contains_key(name) {
            return Err(RuntimeError::Unsupported(format!(
                "vm: keyword arguments to '{name}' are not supported"
            )));
        }
        if let Some(struct_name) = mojito_symbol::symbol::init_overload_struct(name)
            && prog.structs.contains_key(struct_name)
        {
            return self.construct_via_init(
                prog,
                struct_name,
                Some(name),
                args,
                kwargs,
                param_vals,
            );
        }
        match name {
            // Unlinked VM-CTFE programs have the checker-known `range` builtin
            // but no nominal std.range declarations. Materialize its finite
            // compile-time sequence explicitly; GetIter/HasNext/Next reserve
            // their method-free path for this `ComptimeList` representation.
            // Normal linked execution resolves the source `range` overloads and
            // never reaches this branch.
            "range" if self.ctfe_fuel.is_some() => {
                if !(1..=3).contains(&args.len()) {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: if args.is_empty() { 1 } else { 3 },
                        got: args.len(),
                    });
                }
                let bounds = args
                    .iter()
                    .map(value_as_index)
                    .collect::<Result<Vec<_>, _>>()?;
                let (mut current, stop, step) = match bounds.as_slice() {
                    [stop] => (0, *stop, 1),
                    [start, stop] => (*start, *stop, 1),
                    [start, stop, step] => (*start, *stop, *step),
                    _ => unreachable!("range arity checked above"),
                };
                let limit = self.ctfe_fuel.unwrap_or(0);
                let mut values = Vec::new();
                while (step > 0 && current < stop) || (step < 0 && current > stop) {
                    if values.len() >= limit {
                        return Err(RuntimeError::Unsupported(
                            "compile-time execution exceeded the VM CTFE fuel quota".to_string(),
                        ));
                    }
                    values.push(Value::Int(current));
                    current = current.checked_add(step).ok_or_else(|| {
                        RuntimeError::TypeError("range() iteration overflowed Int".to_string())
                    })?;
                }
                Ok(Value::ComptimeList(values))
            }
            "print" => {
                let mut cells = Vec::with_capacity(args.len());
                for value in args {
                    cells.push(self.format_value(prog, value, false)?);
                }
                self.output.push_str(&cells.join(" "));
                self.output.push('\n');
                Ok(Value::None)
            }
            // The `std.os.abort` crossing: an uncatchable trap carrying the
            // nominal String message (only `Raised` is catchable).
            "_mojito_abort" => {
                let message = match args.first() {
                    Some(value @ Value::Struct { .. }) => {
                        match self.string_struct_literal(value)? {
                            Value::Str(text) => text,
                            _ => unreachable!("the string bridge returns a builtin string"),
                        }
                    }
                    Some(Value::Str(text)) => text.clone(),
                    _ => String::new(),
                };
                Err(RuntimeError::Abort(message))
            }
            "String" => Ok(Value::Str(match args.into_iter().next() {
                Some(value) => self.format_value(prog, value, false)?,
                None => String::new(),
            })),
            "repr" => match args.into_iter().next() {
                Some(value) => Ok(Value::Str(self.format_value(prog, value, true)?)),
                None => Err(RuntimeError::ArityMismatch {
                    name: "repr".to_string(),
                    expected: 1,
                    got: 0,
                }),
            },
            // `len(c)` on a user struct dispatches to `c.__len__()`.
            "len" => match args.into_iter().next() {
                Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
                Some(Value::ComptimeList(items)) if self.ctfe_fuel.is_some() => {
                    Ok(Value::Int(items.len() as i64))
                }
                Some(Value::Tuple(items)) => Ok(Value::Int(items.len() as i64)),
                Some(Value::Struct {
                    name,
                    fields,
                    value_params,
                }) => {
                    let recv = Value::Struct {
                        name: name.clone(),
                        fields,
                        value_params,
                    };
                    self.call_dunder(prog, &name, "__len__", vec![recv])
                }
                _ => Err(RuntimeError::Unsupported(
                    "vm: len supports String, internal Tuple storage, and nominal structs with __len__"
                        .into(),
                )),
            },
            "Slice" | "slice" => {
                let optional = |value: &Value| match value {
                    Value::Int(value) => Ok(Some(*value)),
                    Value::IntLiteral(value) => {
                        value.wrapping_signed(64).map(Some).ok_or_else(|| {
                            RuntimeError::TypeError(
                                "slice bound cannot materialize as Int".to_string(),
                            )
                        })
                    }
                    Value::None => Ok(None),
                    other => Err(RuntimeError::TypeError(format!(
                        "slice bound must be Int or None, got {}",
                        crate::runtime::type_name(other)
                    ))),
                };
                let (start, end, step) = match (name, args.as_slice()) {
                    ("slice", [end]) => (None, optional(end)?, None),
                    ("slice" | "Slice", [start, end]) => (optional(start)?, optional(end)?, None),
                    ("slice" | "Slice", [start, end, step]) => {
                        (optional(start)?, optional(end)?, optional(step)?)
                    }
                    _ => {
                        return Err(RuntimeError::ArityMismatch {
                            name: name.to_string(),
                            expected: if name == "Slice" { 2 } else { 1 },
                            got: args.len(),
                        });
                    }
                };
                Ok(Value::Slice {
                    kind: mojito_types::types::SliceKind::Slice,
                    start,
                    end,
                    step,
                })
            }
            // Utility numeric built-ins use the shared runtime value helpers; a
            // struct operand routes through the same dunder the checker resolved.
            "abs" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, "__abs__", vec![value]);
                }
                builtin_abs(value)
            }
            "min" => {
                let (a, b) = arg2(name, args)?;
                builtin_min_max(true, a, b)
            }
            "max" => {
                let (a, b) = arg2(name, args)?;
                builtin_min_max(false, a, b)
            }
            "round" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, "__round__", vec![value]);
                }
                builtin_round(value)
            }
            "input" => {
                let mut prompt = arg1(name, args)?;
                // A nominal String prompt reads back through the bridge.
                if matches!(&prompt, Value::Struct { name, .. }
                    if mojito_symbol::symbol::is_stdlib_string_struct(name))
                {
                    prompt = self.string_struct_literal(&prompt)?;
                }
                if self.input_override.is_some() {
                    return self.input_from_override(prompt);
                }
                builtin_input(prompt)
            }
            "Int" | "Float64" | "Bool" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let dunder = match name {
                        "Float64" => "__float__",
                        "Bool" => "__bool__",
                        _ => "__int__",
                    };
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, dunder, vec![value]);
                }
                builtin_convert(name, value)
            }
            // `Scalar[DType.x](arg)` lowers through the recorded SIMD
            // construction (`MakeSimd`), never a direct builtin call — a bare
            // `Scalar` name here has lost its dtype and cannot be executed.
            "UInt" => builtin_convert(name, arg1(name, args)?),
            "divmod" => {
                let (a, b) = arg2(name, args)?;
                builtin_divmod(a, b)
            }
            "Error" => {
                let mut argument = arg1(name, args)?;
                // A nominal String message reads back through the
                // struct-to-literal bridge before wrapping.
                if matches!(&argument, Value::Struct { name, .. }
                    if mojito_symbol::symbol::is_stdlib_string_struct(name))
                {
                    argument = self.string_struct_literal(&argument)?;
                }
                builtin_error(argument)
            }
            // A struct constructor. A hand-written `def __init__(out self, …)`
            // takes precedence over the fieldwise constructor: build an uninitialized
            // `self` skeleton, run `__init__`, and return the initialized value.
            _ if prog.structs.contains_key(name) => {
                let init_name = format!("{name}.__init__");
                if (!args.is_empty() || kwargs.len() != 1 || kwargs[0].0 != "copy")
                    && (prog.index_of(&init_name).is_some()
                        || prog
                            .index_of(&prog.overload_name(&init_name, args.len()))
                            .is_some())
                {
                    return self.construct_via_init(prog, name, None, args, kwargs, param_vals);
                }
                if !kwargs.is_empty() {
                    self.construct_via_copy(prog, name, args, kwargs, param_vals)
                } else if prog
                    .index_of(&prog.overload_name(&init_name, args.len()))
                    .is_some()
                {
                    self.construct_via_init(prog, name, None, args, Vec::new(), param_vals)
                } else {
                    construct(&prog.structs[name], name, args, param_vals)
                }
            }
            // `UnsafePointer[T].alloc(n)` — reserve `n` slots in the heap arena and
            // return a pointer to the base (the element type is erased).
            "UnsafePointer.alloc" => {
                let n = crate::runtime::value_as_index(&arg1(name, args)?)?;
                self.heap_alloc(n, std::mem::align_of::<Value>() as i64)
            }
            "UnsafePointer.alloc_aligned" => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 2,
                        got: args.len(),
                    });
                }
                let n = crate::runtime::value_as_index(&args[0])?;
                let alignment = crate::runtime::value_as_index(&args[1])?;
                self.heap_alloc(n, alignment)
            }
            "UnsafePointer.unsafe_dangling" | "Pointer.unsafe_dangling" => {
                if !args.is_empty() {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Value::Pointer {
                    allocation: 0,
                    offset: 0,
                })
            }
            _ => match prog.index_of(name) {
                Some(idx) => {
                    // Match positional + keyword args to the parameter slots (fill
                    // defaults, collect `*args`) when a signature is known; else a
                    // plain positional call.
                    let bound = match prog.sigs.get(name) {
                        Some(sig) => self.bind_for_call(prog, name, sig, args, kwargs)?.0,
                        None => args,
                    };
                    // Reify the function's value parameters (`doubled[21]()`): pair
                    // each declared value parameter with its supplied comptime arg.
                    let value_params: Vec<(String, Value)> = match prog.sigs.get(name) {
                        Some(sig) => reify_value_parameters(&sig.param_decls, param_vals),
                        None => Vec::new(),
                    };
                    self.call_function(prog, idx, bound, &value_params)
                }
                None => Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support the built-in or callee '{name}' yet"
                ))),
            },
        }
    }

    pub(super) fn format_value(
        &mut self,
        prog: &Prog,
        value: Value,
        repr: bool,
    ) -> Result<String, RuntimeError> {
        let Value::Struct {
            name,
            fields,
            value_params,
        } = value
        else {
            if repr {
                return Ok(scalar_repr(&value));
            }
            return Ok(value.to_string());
        };
        let method = if repr { "write_repr_to" } else { "write_to" };
        let source = format!("{name}.{method}");
        if let Some(index) = prog.index_of(&prog.overload_name(&source, 1)) {
            let receiver = Value::Struct {
                name,
                fields,
                value_params,
            };
            let (_, variables) =
                self.call_frame(prog, index, vec![receiver, Value::Str(String::new())], &[])?;
            return match variables.get(1) {
                Some(Value::Str(text)) => Ok(text.clone()),
                other => Err(RuntimeError::TypeError(format!(
                    "{source} did not leave a Writer value, got {}",
                    other
                        .map(crate::runtime::type_name)
                        .unwrap_or_else(|| "missing".to_string())
                ))),
            };
        }
        let mut cells = Vec::with_capacity(fields.len());
        for (field, value) in fields {
            cells.push(format!("{field}={}", self.format_value(prog, value, repr)?));
        }
        Ok(format!("{name}({})", cells.join(", ")))
    }

    pub(super) fn format_template(
        &mut self,
        prog: &Prog,
        template: &str,
        arguments: &[Value],
    ) -> Result<String, RuntimeError> {
        let chars: Vec<char> = template.chars().collect();
        let mut output = String::new();
        let mut automatic = 0usize;
        let mut cursor = 0usize;
        while cursor < chars.len() {
            if chars[cursor] == '{' {
                if chars.get(cursor + 1) == Some(&'{') {
                    output.push('{');
                    cursor += 2;
                    continue;
                }
                let Some(end_offset) = chars[cursor + 1..].iter().position(|ch| *ch == '}') else {
                    return Err(RuntimeError::TypeError("unclosed format field".to_string()));
                };
                let end = cursor + 1 + end_offset;
                let field: String = chars[cursor + 1..end].iter().collect();
                let repr = field.contains("!r");
                let spec = field.split_once(':').map(|(_, spec)| spec).unwrap_or("");
                let selector = field.split(['!', ':']).next().unwrap_or_default();
                let index = if selector.is_empty() {
                    let index = automatic;
                    automatic += 1;
                    index
                } else {
                    selector.parse::<usize>().map_err(|_| {
                        RuntimeError::TypeError(format!("invalid format field '{{{field}}}'"))
                    })?
                };
                let value = arguments
                    .get(index)
                    .ok_or_else(|| RuntimeError::ArityMismatch {
                        name: "String.format".to_string(),
                        expected: index + 1,
                        got: arguments.len(),
                    })?;
                let rendered = self.format_value(prog, value.clone(), repr)?;
                output.push_str(&apply_format_spec(value, &rendered, spec)?);
                cursor = end + 1;
                continue;
            }
            if chars[cursor] == '}' && chars.get(cursor + 1) == Some(&'}') {
                output.push('}');
                cursor += 2;
                continue;
            }
            output.push(chars[cursor]);
            cursor += 1;
        }
        Ok(output)
    }
}

/// Upstream's `repr` text for non-struct values: `Int(7)`, `UInt(7)`,
/// `Float64(2.5)`, a single-quoted string with backslash escapes,
/// `Slice(start=1, end=4, step=None)`; everything else prints its text.
fn scalar_repr(value: &Value) -> String {
    match value {
        Value::Int(n) => format!("Int({n})"),
        Value::UInt(n) => format!("UInt({n})"),
        Value::Float64(_) => format!("Float64({value})"),
        Value::FloatLiteral(literal) => format!(
            "Float64({})",
            Value::Float64(literal.to_f64().unwrap_or(f64::NAN))
        ),
        Value::IntLiteral(_) => format!("Int({value})"),
        Value::Str(text) => {
            let mut out = String::from("'");
            for ch in text.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '\'' => out.push_str("\\'"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    other => out.push(other),
                }
            }
            out.push('\'');
            out
        }
        Value::Slice {
            start, end, step, ..
        } => {
            let bound = |bound: &Option<i64>| match bound {
                Some(value) => value.to_string(),
                None => "None".to_string(),
            };
            format!(
                "Slice(start={}, end={}, step={})",
                bound(start),
                bound(end),
                bound(step)
            )
        }
        other => other.to_string(),
    }
}
