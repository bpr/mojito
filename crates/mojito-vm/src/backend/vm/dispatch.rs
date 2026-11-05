//! Named-call dispatch, reference-carry queries, drops, slicing
//! bounds, and value formatting.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// The checked types a named call carries: the compile-time arguments a
/// builtin reads (`external_call`'s callee), its argument types, and the
/// struct its result declares, which a constructor call builds.
#[derive(Default)]
pub(super) struct CallTypes<'a> {
    pub(super) instantiated: &'a [mojito_types::types::TyArg],
    pub(super) arg_types: &'a [Option<mojito_types::types::Ty>],
    pub(super) constructed: Option<&'a str>,
}

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
    /// visited set for recursive aggregates. A stored `ref` handle and a
    /// single-pointee origin-bearing pointer (`Pointer[T, o]` over one place)
    /// are frame-rooted handles; multi-element pointer-backed views (`Span`)
    /// copy by value safely, and generic parameters are conservatively
    /// `false`.
    pub(super) fn type_carries_reference_handle(
        prog: &Prog,
        ty: &Ty,
        visited: &mut std::collections::HashSet<String>,
    ) -> bool {
        match ty.kind() {
            TyKind::Ref(_) => true,
            TyKind::Pointer { .. } if super::references::single_pointee_pointer(Some(ty)) => true,
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
                visited.insert(name.clone())
                    && prog.structs.get(name).is_some_and(|declaration| {
                        declaration.fields.iter().any(|(_, field_ty)| {
                            Self::type_carries_reference_handle(prog, field_ty, visited)
                        })
                    })
            }
            TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => items
                .iter()
                .any(|item| Self::type_carries_reference_handle(prog, item, visited)),
            _ => false,
        }
    }

    /// Destroy `value`, which a `drop.var` or `drop.place` of the static
    /// type `ty` ends, by running `destructor`, the `__deinit__` the drop
    /// names, on the whole value, else by dropping its fields: the value
    /// holds every part (`mir::verify`'s slot-end rule). A field, and what
    /// a cleanup list or any other teardown ends, still finds its
    /// destructor in its struct's member table. A destructor the compiler
    /// synthesized runs untraced: the source declares none.
    pub(super) fn drop_named(
        &mut self,
        prog: &Prog,
        value: Value,
        ty: Option<&Ty>,
        destructor: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let value = match ty {
            Some(ty) => erased_instance_value(prog, value, ty),
            None => value,
        };
        match value {
            Value::Struct { name, fields } => {
                let Some(destructor) = destructor else {
                    for (_, value) in fields {
                        self.drop_value(prog, value)?;
                    }
                    return Ok(());
                };
                let index = prog.index_of(destructor).ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "vm: the destructor `{destructor}` a drop names is not defined"
                    ))
                })?;
                if !prog.synthesized(destructor) {
                    self.record_lifecycle(format!("drop {name}"));
                }
                self.call_function(prog, index, vec![Value::Struct { name, fields }])?;
                Ok(())
            }
            value => self.drop_value(prog, value),
        }
    }

    /// Copy `value` of the static type `ty` by running `constructor`, the
    /// `__copyinit__` the copy names, the copy keeping `value`'s own
    /// instance names; a copy naming none copies by type, as
    /// [`Self::clone_value_as`].
    pub(super) fn copy_named(
        &mut self,
        prog: &Prog,
        value: &Value,
        ty: Option<&Ty>,
        constructor: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        match (constructor, value) {
            (Some(constructor), Value::Struct { .. }) => {
                let source = match ty {
                    Some(ty) => erased_instance_value(prog, value.clone(), ty),
                    None => value.clone(),
                };
                let copy = self.run_constructor(prog, constructor, source)?;
                Ok(with_names_of(value, copy))
            }
            _ => self.clone_value_as(prog, value, ty),
        }
    }

    /// Relocate `value` moved out of a slot of the static type `ty` by
    /// running `constructor`, the `__moveinit__` the move names, the moved
    /// value keeping `value`'s own instance names; a move naming none moves
    /// by type, as [`Self::move_value_as`].
    pub(super) fn move_named(
        &mut self,
        prog: &Prog,
        value: Value,
        ty: Option<&Ty>,
        constructor: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        match (constructor, &value) {
            (Some(constructor), Value::Struct { .. }) => {
                let names = value.clone();
                let source = match ty {
                    Some(ty) => erased_instance_value(prog, value, ty),
                    None => value,
                };
                let moved = self.run_constructor(prog, constructor, source)?;
                Ok(with_names_of(&names, moved))
            }
            _ => self.move_value_as(prog, value, ty),
        }
    }

    /// The payload a variant store takes `value` of the alternative `ty`
    /// as: moved in by `constructor`, the `__moveinit__` the store names,
    /// as upstream's `init_pointee_move`. The operand register is the
    /// store's to end, so no drop of it follows.
    pub(super) fn move_payload(
        &mut self,
        prog: &Prog,
        value: Value,
        ty: &Ty,
        constructor: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        let value = crate::runtime::coerce_checked(value, ty);
        if self.has_moveinit {
            self.move_named(prog, value, Some(ty), constructor)
        } else {
            Ok(value)
        }
    }

    /// Copy `value` held in a slot or register of the static type `ty`: as
    /// [`Self::clone_value`], through `ty`'s members where it is an erased
    /// struct instance ([`erased_instance_value`]). The copy keeps `value`'s
    /// own instance names, as it is the same instance wherever it goes.
    pub(super) fn clone_value_as(
        &mut self,
        prog: &Prog,
        value: &Value,
        ty: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        match ty.filter(|ty| mojito_types::types::erases_an_argument(ty)) {
            Some(ty) => {
                let erased = erased_instance_value(prog, value.clone(), ty);
                let copy = self.clone_value(prog, &erased)?;
                Ok(with_names_of(value, copy))
            }
            None => self.clone_value(prog, value),
        }
    }

    /// Relocate `value` moved out of a slot of the static type `ty`: as
    /// [`Self::move_typed_value`], through `ty`'s members where it is an
    /// erased struct instance ([`erased_instance_value`]), the moved value
    /// keeping `value`'s own instance names.
    pub(super) fn move_value_as(
        &mut self,
        prog: &Prog,
        value: Value,
        ty: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        match ty.filter(|ty| mojito_types::types::erases_an_argument(ty)) {
            Some(ty) => {
                let names = value.clone();
                let moved = self.move_typed_value(prog, erased_instance_value(prog, value, ty))?;
                Ok(with_names_of(&names, moved))
            }
            None => self.move_typed_value(prog, value),
        }
    }

    /// Recursively destroy a value (ASAP drop): run a struct's `__deinit__` if
    /// it defines one (its body consumes the receiver's fields at their last
    /// use), else drop the fields in declaration order — Mojo's order.
    /// Internal tuple/compile-time storage recurses through its elements.
    /// Scalars are a no-op.
    pub(super) fn drop_value(&mut self, prog: &Prog, v: Value) -> Result<(), RuntimeError> {
        match v {
            Value::Struct { name, fields, .. } => {
                if let Some(idx) = prog.lifecycle_member(&name, "__deinit__") {
                    self.record_lifecycle(format!("drop {name}"));
                    // `self` is the whole struct, owned by the destructor: its
                    // elaborated drops destroy the fields, each at its last
                    // use. The return value is discarded.
                    let self_val = Value::Struct { name, fields };
                    self.call_function(prog, idx, vec![self_val])?;
                    return Ok(());
                }
                for (_, value) in fields {
                    self.drop_value(prog, value)?;
                }
            }
            Value::ComptimeList(items) => {
                for item in items.into_iter().rev() {
                    self.drop_value(prog, item)?;
                }
            }
            // Pack storage a teardown reaches by type, left to right: a
            // drop of it names each element's destructor instead
            // (`mir::refine_pack_drops`), and a homogeneous collector's
            // `VariadicList.__deinit__` ends its storage itself.
            Value::Tuple(items) => {
                for item in items {
                    self.drop_value(prog, item)?;
                }
            }
            Value::Variant { value, .. } => self.drop_value(prog, *value)?,
            _ => {}
        }
        Ok(())
    }

    /// Destroy the payload of the variant value `old` a store replaces, by
    /// the destructor `destructors` names for its active alternative, as
    /// upstream's `Variant.__del__` calls the alternative's `__del__` under
    /// its tag test; a store naming none for it drops by type.
    pub(super) fn drop_variant_payload(
        &mut self,
        prog: &Prog,
        old: Value,
        destructors: &[Option<String>],
    ) -> Result<(), RuntimeError> {
        let Value::Variant {
            alternatives,
            index,
            value,
        } = old
        else {
            return self.drop_value(prog, old);
        };
        self.drop_named(
            prog,
            *value,
            alternatives.get(index),
            destructors.get(index).and_then(Option::as_deref),
        )
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
    /// user function (with default/keyword/`*args` slot-matching).
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn call_named(
        &mut self,
        prog: &Prog,
        name: &str,
        args: Vec<Value>,
        kwargs: &[(String, Value)],
        types: &CallTypes<'_>,
    ) -> Result<Value, RuntimeError> {
        let CallTypes {
            instantiated,
            arg_types,
            constructed,
        } = *types;
        // Built-ins take positional arguments only, and user functions handle
        // keywords through their signatures below. Struct constructors get a
        // narrow exception for Mojo's lifecycle copy constructor (`copy:`).
        if !kwargs.is_empty() && !prog.sigs.contains_key(name) && !prog.structs.contains_key(name) {
            return Err(RuntimeError::Unsupported(format!(
                "vm: keyword arguments to '{name}' are not supported"
            )));
        }
        // A resolved constructor symbol — an overload, a closed instance's
        // own clone (`Box.__init__$y3:Int`), an owner-less instance, or the
        // copy or move constructor a copy or move calls — constructs the
        // call's result struct through that exact body.
        if let Some((struct_name, "__init__" | "__copyinit__" | "__moveinit__")) =
            prog.lifecycle_constructor(name, constructed)
        {
            return self.construct_via_init(prog, struct_name, name, args, kwargs);
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
            // Upstream's libc FFI over the closed callee table: the callee
            // name is the call's first compile-time argument; the result
            // is coerced to the destination's checked type by the caller.
            "external_call" => {
                let Some(mojito_types::types::TyArg::Val(mojito_types::ct::CtValue::Str(callee))) =
                    instantiated.first()
                else {
                    return Err(RuntimeError::TypeError(
                        "vm: external_call without a callee parameter argument".to_string(),
                    ));
                };
                let callee = callee.clone();
                self.external_call(&callee, &args, arg_types)
            }
            // `std.builtin._format_float`'s staged leaf: the float's
            // display bytes written at the pointer, and their count.
            "_mojito_float_text" => {
                let [Value::Float64(value), Value::Pointer { allocation, offset }] =
                    args.as_slice()
                else {
                    return Err(RuntimeError::TypeError(
                        "vm: _mojito_float_text takes a Float64 and a byte pointer".to_string(),
                    ));
                };
                let (allocation, offset) = (*allocation, *offset);
                let text = crate::runtime::float_display(*value);
                for (index, byte) in (0_i64..).zip(text.bytes()) {
                    let (region, slot) = self.heap_index(allocation, offset, index)?;
                    self.heap_store(
                        region,
                        slot,
                        Value::Simd {
                            dtype: mojito_ast::ast::Dtype::UInt8,
                            lanes: crate::runtime::SimdLanes::Int(vec![i128::from(byte)]),
                        },
                    );
                }
                Ok(Value::Int(
                    i64::try_from(text.len()).expect("a float's text length fits i64"),
                ))
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
            // The compiler's own `len`: a pack cursor's length, and CTFE's
            // over a compile-time list. A program's `len` is the library's.
            "len" => match args.into_iter().next() {
                Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
                Some(Value::ComptimeList(items)) if self.ctfe_fuel.is_some() => {
                    Ok(Value::Int(items.len() as i64))
                }
                Some(Value::Tuple(items)) => Ok(Value::Int(items.len() as i64)),
                Some(Value::Simd { lanes, .. }) => Ok(Value::Int(lanes.width() as i64)),
                Some(
                    Value::Int(_)
                    | Value::UInt(_)
                    | Value::Float64(_)
                    | Value::IntLiteral(_)
                    | Value::FloatLiteral(_),
                ) => Ok(Value::Int(1)),
                _ => Err(RuntimeError::Unsupported(
                    "vm: len supports String, internal Tuple storage, and SIMD and its scalars"
                        .into(),
                )),
            },
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
            "input" => {
                let mut prompt = arg1(name, args)?;
                // A nominal String prompt reads back through the bridge.
                if matches!(&prompt, Value::Struct { name, .. }
                    if mojito_symbol::symbol::is_stdlib_string_struct(name))
                {
                    prompt = self.string_struct_literal(&prompt)?;
                }
                let line = if self.input_override.is_some() {
                    self.input_from_override(prompt)?
                } else {
                    builtin_input(prompt)?
                };
                match line {
                    Value::Str(text)
                        if constructed == Some(mojito_symbol::symbol::STDLIB_STRING_STRUCT) =>
                    {
                        self.nominal_string_value(prog, &text)
                    }
                    line => Ok(line),
                }
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
                        Some(sig) => self.bind_for_call(prog, name, sig, &args, kwargs)?.0,
                        None => args,
                    };
                    // An initializer of a receiver the VM holds as a value
                    // (a `SIMD`) enters with that value's unwritten storage.
                    match prog.constructed(name, &prog.mir.functions[idx].1.param_types) {
                        Some(Constructed::Value(skeleton)) => {
                            self.construct_entered(prog, skeleton, name, bound)
                        }
                        _ => self.call_function(prog, idx, bound),
                    }
                }
                None => Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support the built-in or callee '{name}' yet"
                ))),
            },
        }
    }

    /// Run the copy or move constructor `constructor` a copy or move names
    /// on `source`, a struct value, and return the value it initializes.
    fn run_constructor(
        &mut self,
        prog: &Prog,
        constructor: &str,
        source: Value,
    ) -> Result<Value, RuntimeError> {
        let Value::Struct { name, .. } = &source else {
            return Ok(source);
        };
        let index = prog.index_of(constructor).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: the constructor `{constructor}` a copy or move names is not defined"
            ))
        })?;
        let skeleton = Self::struct_skeleton(prog, name);
        let (_, frame_vars) = self.call_frame(prog, index, vec![skeleton, source])?;
        Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
    }
}

/// The host text a writer's `$write_formatted` (or, with `repr`,
/// `$write_repr_formatted`) writes for an intrinsic value with no
/// declared `write_to`: `DType`, `Error`, `None`, and the VM's internal
/// values. A declared value writes itself through its own member, so a
/// struct or `SIMD` value here is an unsupported boundary. A moved-out
/// tombstone (an unsafe take of never-written storage) traps as any use
/// of it does.
pub(super) fn format_value(value: &Value, repr: bool) -> Result<String, RuntimeError> {
    match value {
        Value::Moved => Err(RuntimeError::TypeError(
            "vm: display of a value after it was moved".to_string(),
        )),
        Value::Struct { name, .. } => Err(RuntimeError::Unsupported(format!(
            "vm: host-formatted write of the declared struct `{name}`"
        ))),
        Value::Int(_)
        | Value::UInt(_)
        | Value::Float64(_)
        | Value::Bool(_)
        | Value::Simd { .. } => Err(RuntimeError::Unsupported(
            "vm: host-formatted write of a declared `SIMD` or `Bool` value".to_string(),
        )),
        _ if repr => Ok(scalar_repr(value)),
        _ => Ok(value.to_string()),
    }
}

/// `value` as the erased struct instance `ty` its slot declares
/// (`Box$mono$V$REMOVED$ARG`, `List$mono$TBox$mono$V$REMOVED$ARG`), when it
/// is one: upstream calls a value's destructor, copy constructor, and move
/// constructor by its static type, and the erased instance lays out as every
/// instance of its template that agrees on the kept arguments, so the value
/// keeps its fields and takes the erased instance's name, and so its members.
/// Its fields of erased types follow. Any other value is returned as it is.
pub(super) fn erased_instance_value(prog: &Prog, value: Value, ty: &Ty) -> Value {
    let Some((erased, _)) = ty.kind().as_non_simd_struct() else {
        return value;
    };
    match value {
        Value::Struct { name, fields }
            if name != *erased
                && mojito_types::types::erases_an_argument(ty)
                && mojito_types::types::struct_template(&name)
                    == mojito_types::types::struct_template(erased) =>
        {
            let declared = prog.structs.get(erased.as_str());
            let fields = fields
                .into_iter()
                .map(|(field, value)| {
                    let declared = declared.and_then(|declared| {
                        declared.fields.iter().find(|(name, _)| *name == field)
                    });
                    let value = match declared {
                        Some((_, ty)) => erased_instance_value(prog, value, ty),
                        None => value,
                    };
                    (field, value)
                })
                .collect();
            Value::Struct {
                name: erased.clone(),
                fields,
            }
        }
        value => value,
    }
}

/// `value` with the struct instance names of `original`, field by field:
/// the copy or move of `original` an erased instance's member made.
fn with_names_of(original: &Value, value: Value) -> Value {
    match (original, value) {
        (
            Value::Struct {
                name,
                fields: originals,
            },
            Value::Struct { fields, .. },
        ) => Value::Struct {
            name: name.clone(),
            fields: fields
                .into_iter()
                .map(|(field, value)| {
                    let value = match originals.iter().find(|(name, _)| *name == field) {
                        Some((_, original)) => with_names_of(original, value),
                        None => value,
                    };
                    (field, value)
                })
                .collect(),
        },
        (_, value) => value,
    }
}

/// Upstream's `repr` text for a value no member table displays: a
/// literal as the `Int` or `Float64` it materializes to, `DType.int32`, a
/// single-quoted string with backslash escapes; everything else prints its
/// text.
fn scalar_repr(value: &Value) -> String {
    match value {
        Value::FloatLiteral(literal) => format!(
            "Float64({})",
            Value::Float64(literal.to_f64().unwrap_or(f64::NAN))
        ),
        Value::IntLiteral(_) => format!("Int({value})"),
        Value::Dtype(dtype) => format!("DType.{}", dtype.name()),
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
        other => other.to_string(),
    }
}
