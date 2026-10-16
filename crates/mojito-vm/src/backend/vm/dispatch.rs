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
            TyKind::Struct(name, _) => {
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

    /// Destroy the residual fields of a consumed aggregate (a named
    /// destructor's receiver, or a consumed place).
    pub(super) fn drop_struct_fields(
        &mut self,
        prog: &Prog,
        fields: Vec<(String, Value)>,
    ) -> Result<(), RuntimeError> {
        for (_, value) in fields {
            self.drop_value(prog, value)?;
        }
        Ok(())
    }

    /// Destroy the elements of an owning positional collector, last to
    /// first — the order Mojo destroys a `var *args` pack in.
    pub(super) fn drop_owned_pack(
        &mut self,
        prog: &Prog,
        items: Vec<Value>,
    ) -> Result<(), RuntimeError> {
        for item in items.into_iter().rev() {
            self.drop_value(prog, item)?;
        }
        Ok(())
    }

    /// Destroy `value`, which a `drop.var` or `drop.place` of the static
    /// type `ty` ends, by running `destructor`, the `__deinit__` the drop
    /// names, on the whole value, else by dropping its fields. A partial
    /// aggregate drops only its residual fields, as [`Self::drop_value`].
    /// A field, and what a cleanup list or any other teardown ends, still
    /// finds its destructor in its struct's member table. A destructor the
    /// compiler synthesized runs untraced: the source declares none.
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
            Value::Struct { name, fields }
                if !fields
                    .iter()
                    .any(|(_, value)| matches!(value, Value::Moved)) =>
            {
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

    /// Destroy `value` held in a slot of the static type `ty`: as
    /// [`Self::drop_value`], through `ty`'s members where it is an erased
    /// struct instance ([`erased_instance_value`]).
    pub(super) fn drop_value_as(
        &mut self,
        prog: &Prog,
        value: Value,
        ty: Option<&Ty>,
    ) -> Result<(), RuntimeError> {
        let value = match ty {
            Some(ty) => erased_instance_value(prog, value, ty),
            None => value,
        };
        self.drop_value(prog, value)
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
                // A partial aggregate cannot run its whole-value destructor:
                // that method may inspect a field already transferred or
                // explicitly destroyed. Drop only the initialized residual
                // fields. Checked explicit-destroy obligations guarantee that
                // an intact linear value never reaches an automatic DropVar, so
                // this rule does not need to reconstruct generic conditional
                // deletability from the runtime struct name.
                if fields
                    .iter()
                    .any(|(_, value)| matches!(value, Value::Moved))
                {
                    for (_, value) in fields {
                        if !matches!(value, Value::Moved) {
                            self.drop_value(prog, value)?;
                        }
                    }
                    return Ok(());
                }
                if let Some(idx) = prog.lifecycle_member(&name, "__deinit__") {
                    self.record_lifecycle(format!("drop {name}"));
                    // `self` is the whole struct, owned by the destructor: its
                    // elaborated `ConsumeVar` destroys the residual fields at
                    // their last use. The return value is discarded.
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
            // `Tuple`'s private backing storage is destroyed left to right;
            // the nominal wrapper reaches it through the struct branch above.
            // An owning collector goes through `drop_owned_pack` instead.
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

    /// Materialize one intrinsic Slice bound through the bundled
    /// `Optional[Int]` constructor its read names in `constructors`: the
    /// positional one-argument one when the bound is set, the nullary one
    /// otherwise. The projection's declared result `target` names the
    /// constructed struct.
    pub(super) fn slice_bound_optional(
        &mut self,
        prog: &Prog,
        bound: Option<i64>,
        target: Option<&Ty>,
        constructors: &[String],
    ) -> Result<Value, RuntimeError> {
        let name = prog.declared_struct(target).ok_or_else(|| {
            RuntimeError::Unsupported(
                "vm: Slice bound access requires its declared Optional result".to_string(),
            )
        })?;
        let arguments = bound
            .map(|value| vec![Value::Int(value)])
            .unwrap_or_default();
        let expected_params = arguments.len() + 1;
        let constructor = constructors
            .iter()
            .find(|constructor| {
                prog.index_of(constructor)
                    .is_some_and(|index| prog.mir.functions[index].1.n_params == expected_params)
            })
            .ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: Slice bound access names no {}-argument `{name}` constructor",
                    arguments.len()
                ))
            })?;
        self.construct_via_init(prog, name, Some(constructor), arguments, Vec::new())
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
        kwargs: Vec<(String, Value)>,
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
        if !kwargs.is_empty()
            && name != "print"
            && !prog.sigs.contains_key(name)
            && !prog.structs.contains_key(name)
        {
            return Err(RuntimeError::Unsupported(format!(
                "vm: keyword arguments to '{name}' are not supported"
            )));
        }
        // A resolved constructor symbol — an overload, a closed instance's
        // own clone (`Box.__init__$y3:Int`), an owner-less instance, or a
        // `T(copy=x)`'s copy constructor — constructs the call's result
        // struct through that exact body.
        if let Some((struct_name, "__init__" | "__copyinit__")) =
            prog.lifecycle_constructor(name, constructed)
        {
            return self.construct_via_init(prog, struct_name, Some(name), args, kwargs);
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
            // `print(*args, sep=, end=, flush=, file=)`: the joined cells go
            // to the captured stdout, or through the descriptor table when
            // `file=` names another descriptor (`flush` is a no-op: the
            // captured stream is unbuffered).
            "print" => {
                let mut cells = Vec::with_capacity(args.len());
                for value in args {
                    cells.push(self.format_value(prog, value, false)?);
                }
                let mut sep = " ".to_string();
                let mut end = "\n".to_string();
                let mut fd = 1;
                for (key, value) in kwargs {
                    match key.as_str() {
                        "sep" => sep = self.format_value(prog, value, false)?,
                        "end" => end = self.format_value(prog, value, false)?,
                        "flush" => {}
                        "file" => fd = descriptor_value(&value)?,
                        other => {
                            return Err(RuntimeError::Unsupported(format!(
                                "vm: keyword argument '{other}' to 'print'"
                            )));
                        }
                    }
                }
                let mut text = cells.join(&sep);
                text.push_str(&end);
                if fd == 1 {
                    self.output.push_str(&text);
                } else if let Err(error) = self.host_write_bytes(fd, text.as_bytes()) {
                    return Err(RuntimeError::Abort(format!(
                        "expected amount of bytes not written: {error}"
                    )));
                }
                Ok(Value::None)
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
            // A method whose elaboration failed: the elaborator reports it
            // where a call instantiates the method, before concrete MIR runs.
            "_mojito_instantiation_failed" => Err(RuntimeError::Unsupported(match args.first() {
                Some(Value::Str(message)) => format!("function instantiation failed: {message}"),
                _ => "function instantiation failed".to_string(),
            })),
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
                Some(Value::Simd { lanes, .. }) => Ok(Value::Int(lanes.width() as i64)),
                Some(Value::Float64(_)) => Ok(Value::Int(1)),
                Some(Value::Struct {
                    name,
                    fields,
                }) => {
                    let recv = Value::Struct {
                        name: name.clone(),
                        fields,
                    };
                    self.call_dunder(prog, &name, "__len__", vec![recv])
                }
                _ => Err(RuntimeError::Unsupported(
                    "vm: len supports String, internal Tuple storage, SIMD, and nominal structs with __len__"
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
                    // A nominal `Optional[Int]` bound: read its owned slot.
                    Value::Struct { .. } => {
                        let size = value_as_index(&get_field(value, "_size")?)?;
                        if size == 0 {
                            return Ok(None);
                        }
                        let Value::Pointer { allocation, offset } = get_field(value, "data")?
                        else {
                            return Err(RuntimeError::TypeError(
                                "slice bound Optional storage requires Pointer".to_string(),
                            ));
                        };
                        match self.heap_read(allocation, offset, 0)? {
                            Value::Int(value) => Ok(Some(value)),
                            other => Err(RuntimeError::TypeError(format!(
                                "slice bound must be Int or None, got {}",
                                crate::runtime::type_name(&other)
                            ))),
                        }
                    }
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
                builtin_round(&value)
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
            // The elaborator rewrites a conversion of a struct value into
            // a call of its member, so a struct reaching here fails in
            // `builtin_convert`.
            "Int" | "Float64" | "Bool" => builtin_convert(name, arg1(name, args)?),
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
                let constructor = prog.constructor_name(name, args.len());
                if (!args.is_empty() || kwargs.len() != 1 || kwargs[0].0 != "copy")
                    && prog.index_of(&constructor).is_some()
                {
                    return self.construct_via_init(
                        prog,
                        name,
                        None,
                        args,
                        kwargs,
                    );
                }
                if !kwargs.is_empty() {
                    self.construct_via_copy(prog, name, &args, &kwargs)
                } else if prog.index_of(&constructor).is_some() {
                    self.construct_via_init(
                        prog,
                        name,
                        None,
                        args,
                        Vec::new(),
                    )
                } else {
                    construct(&prog.structs[name], name, args)
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
                        Some(sig) => self.bind_for_call(prog, name, sig, &args, kwargs)?.0,
                        None => args,
                    };
                    self.call_function(prog, idx, bound)
                }
                None => Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support the built-in or callee '{name}' yet"
                ))),
            },
        }
    }

    /// Format one value as `print`/`String`/`repr`/`Writer.write` do. A
    /// nominal struct writes itself through `write_to`/`write_repr_to`,
    /// dispatched by runtime struct name — or, when the caller knows the
    /// argument's checked static type and it names a closed generic-struct
    /// instance, through that instance's per-instantiation clone.
    pub(super) fn format_value(
        &mut self,
        prog: &Prog,
        value: Value,
        repr: bool,
    ) -> Result<String, RuntimeError> {
        let Value::Struct { name, fields } = value else {
            // Integer text is the bundled `std._intrinsics._int_digits`
            // body — the same digits the native backend calls it for —
            // wrapped in `repr`'s constructor spelling when asked.
            if let Some(digits) = self.integer_digits(prog, &value)? {
                return Ok(match (repr, &value) {
                    (false, _) => digits,
                    (true, Value::UInt(_)) => format!("UInt({digits})"),
                    (true, _) => format!("Int({digits})"),
                });
            }
            if let Value::Simd { dtype, lanes } = &value {
                let cells = self.simd_lane_texts(prog, *dtype, lanes)?;
                return Ok(match (repr, cells.as_slice()) {
                    (true, _) => simd_repr(*dtype, &cells),
                    (false, [lane]) => lane.clone(),
                    (false, _) => format!("[{}]", cells.join(", ")),
                });
            }
            if repr {
                return Ok(scalar_repr(&value));
            }
            return Ok(value.to_string());
        };
        let method = if repr { "write_repr_to" } else { "write_to" };
        // The display member is the struct's member-table instance whose
        // writer monomorphization bound to the builtin string writer.
        let mut witnesses = prog
            .struct_member_instances(&name, method)
            .filter(|(_, symbol)| {
                prog.sigs.get(*symbol).is_none_or(|sig| {
                    sig.param_types.first().is_some_and(|parameter| {
                        *parameter == mojito_types::types::TyKind::StringLiteral
                            || mojito_types::types::is_writer_parameter(parameter)
                    })
                })
            });
        let witness = witnesses.next();
        if witnesses.next().is_some() {
            return Err(RuntimeError::Unsupported(format!(
                "vm: display of `{name}` with ambiguous `{method}` instances"
            )));
        }
        if let Some((index, symbol)) = witness {
            let receiver = Value::Struct { name, fields };
            let (_, variables) =
                self.call_frame(prog, index, vec![receiver, Value::Str(String::new())])?;
            return match variables.get(1) {
                Some(Value::Str(text)) => Ok(text.clone()),
                other => Err(RuntimeError::TypeError(format!(
                    "{symbol} did not leave a Writer value, got {}",
                    other.map_or_else(|| "missing".to_string(), crate::runtime::type_name)
                ))),
            };
        }
        let mut cells = Vec::with_capacity(fields.len());
        for (field, value) in fields {
            cells.push(format!("{field}={}", self.format_value(prog, value, repr)?));
        }
        Ok(format!("{name}({})", cells.join(", ")))
    }

    /// The decimal text of a runtime integer: the bundled
    /// `std._intrinsics._int_digits`/`_uint_digits` bodies formatting into a
    /// reused heap scratch buffer, which is what the native backend calls
    /// into its own formatting alloca. `None` for a non-integer value, and
    /// for a program whose bundled stdlib is not linked (the phase-composed
    /// seam), where the Rust `Display` rendering stands in.
    fn integer_digits(
        &mut self,
        prog: &Prog,
        value: &Value,
    ) -> Result<Option<String>, RuntimeError> {
        let symbol = match value {
            Value::Int(_) => mojito_symbol::symbol::INT_DIGITS_SYMBOL,
            Value::UInt(_) => mojito_symbol::symbol::UINT_DIGITS_SYMBOL,
            _ => return Ok(None),
        };
        let Some(index) = prog.index_of(symbol) else {
            return Ok(None);
        };
        let scratch = self.digit_scratch()?;
        let (length, _) = self.call_frame(prog, index, vec![value.clone(), scratch.clone()])?;
        let Value::Int(length) = length else {
            return Err(RuntimeError::TypeError(format!(
                "`{symbol}` did not return a byte count, got {}",
                crate::runtime::type_name(&length)
            )));
        };
        let bytes = self.heap_bytes(&scratch, length)?;
        Ok(Some(
            std::string::String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }

    /// The text of each lane of a `SIMD` value, as upstream's
    /// `_write_scalar` writes it: integer lanes through the bundled digit
    /// bodies at the dtype's signedness, as native `print_simd` emits them,
    /// and float and bool lanes through their display.
    fn simd_lane_texts(
        &mut self,
        prog: &Prog,
        dtype: mojito_ast::ast::Dtype,
        lanes: &crate::runtime::SimdLanes,
    ) -> Result<Vec<String>, RuntimeError> {
        use crate::runtime::SimdLanes;
        match lanes {
            SimdLanes::Int(lanes) => {
                let signed =
                    crate::runtime::integer_dtype_bits(dtype).is_some_and(|(_, signed)| signed);
                lanes
                    .iter()
                    .map(|&lane| {
                        let scalar = if signed {
                            Value::Int(lane as i64)
                        } else {
                            Value::UInt(lane as u64)
                        };
                        Ok(self
                            .integer_digits(prog, &scalar)?
                            .unwrap_or_else(|| lane.to_string()))
                    })
                    .collect()
            }
            SimdLanes::Float(lanes) => Ok(lanes
                .iter()
                .map(|&lane| crate::runtime::float_display(lane))
                .collect()),
            SimdLanes::Bool(lanes) => Ok(lanes
                .iter()
                .map(|&lane| if lane { "True" } else { "False" }.to_string())
                .collect()),
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

/// `value` as the erased struct instance `ty` its slot declares
/// (`Box$mono$V$REMOVED$ARG`, `List$mono$TBox$mono$V$REMOVED$ARG`), when it
/// is one: upstream calls a value's destructor, copy constructor, and move
/// constructor by its static type, and the erased instance lays out as every
/// instance of its template that agrees on the kept arguments, so the value
/// keeps its fields and takes the erased instance's name, and so its members.
/// Its fields of erased types follow. Any other value is returned as it is.
pub(super) fn erased_instance_value(prog: &Prog, value: Value, ty: &Ty) -> Value {
    let TyKind::Struct(erased, _) = ty.kind() else {
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

/// Upstream's `SIMD.write_repr_to` over lane texts: the scalar alias at
/// width 1 (`Int32(-5)`, `Float32(0.5)`), else `SIMD[DType.int32, 2](1, 2)`.
fn simd_repr(dtype: mojito_ast::ast::Dtype, cells: &[String]) -> String {
    let lanes = cells.join(", ");
    match (cells.len(), dtype.repr_alias()) {
        (1, Some(alias)) => format!("{alias}({lanes})"),
        (width, _) => format!("SIMD[DType.{}, {width}]({lanes})", dtype.name()),
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

/// The raw descriptor held by a `FileDescriptor` value (`print(file=)`).
fn descriptor_value(value: &Value) -> Result<i32, RuntimeError> {
    let Value::Struct { fields, .. } = value else {
        return Err(RuntimeError::TypeError(
            "vm: 'file' argument to 'print' is not a FileDescriptor".to_string(),
        ));
    };
    match fields.iter().find(|(name, _)| name == "value") {
        Some((_, Value::Int(fd))) => Ok(*fd as i32),
        _ => Err(RuntimeError::TypeError(
            "vm: FileDescriptor value without an Int descriptor".to_string(),
        )),
    }
}
