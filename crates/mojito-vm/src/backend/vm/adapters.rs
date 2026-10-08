//! Checked-result adapters and dunder-backed index loads.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl VmBackend {
    /// Apply an explicit checker-proven abstract-result adapter after runtime
    /// retargeting has selected the concrete declaration.  The declaration ABI,
    /// not the returned `Value` shape, decides whether a read is needed: a
    /// value-returning method may legitimately return a reference-valued value.
    pub(super) fn apply_checked_result_adapter(
        &mut self,
        prog: &Prog,
        value: Value,
        adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
        concrete_returns_reference: bool,
        frames: ResultAdapterFrames<'_>,
    ) -> Result<Value, RuntimeError> {
        let ResultAdapterFrames {
            current,
            current_variables,
            mut returned,
        } = frames;
        match adapter {
            None => Ok(value),
            Some(mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference)
                if !concrete_returns_reference =>
            {
                Ok(value)
            }
            Some(mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference) => {
                let value = match self.read_reference(&value, current, current_variables) {
                    Ok(value) => value,
                    Err(error) => {
                        // A nominal method can execute a read-only, consuming,
                        // or write-back receiver in a temporary callee frame. A
                        // reference to that receiver's own field still names the
                        // just-returned frame; materialize it from the returned
                        // receiver slots before those slots are discarded.
                        let Some((returned_frame_id, returned)) = returned.as_mut() else {
                            return Err(error);
                        };
                        let Value::Ref {
                            frame,
                            slot,
                            projection,
                        } = &value
                        else {
                            return Err(error);
                        };
                        if *frame != returned_frame_id.0 {
                            return Err(error);
                        }
                        let Some(root) = returned.get(*slot) else {
                            return Err(error);
                        };
                        self.read_reference_projection(
                            current,
                            current_variables,
                            root,
                            projection,
                        )?
                    }
                };
                if self.has_copyinit {
                    self.clone_value_with_reachable_frames(
                        prog,
                        &value,
                        current,
                        current_variables,
                        returned,
                    )
                } else {
                    Ok(value)
                }
            }
        }
    }

    /// Materialize an intrinsic result at its checked MIR type boundary. Public
    /// `Tuple` is a nominal library struct even when a primitive operation can
    /// compute its elements most conveniently in private `Value::Tuple` pack
    /// storage (currently `divmod` and `Slice.indices`). The checked destination
    /// type selects the exact concrete Tuple specialization; no runtime element
    /// guessing or source-AST reconstruction is involved.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn materialize_checked_result(
        &self,
        prog: &Prog,
        value: Value,
        target: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        let Some(target @ Ty::Struct(name, _)) = target else {
            return Ok(match target {
                Some(target) => crate::runtime::coerce_checked(value, target),
                None => value,
            });
        };
        let Some(public_elements) = mojito_types::types::tuple_elements(target) else {
            return Ok(crate::runtime::coerce_checked(value, target));
        };
        let Value::Tuple(mut items) = value else {
            return Ok(crate::runtime::coerce_checked(value, target));
        };
        // `ContiguousSlice.indices` is checked as the two-element `(start,
        // end)` while the intrinsic computes the three normalized bounds; the
        // checked destination selects the shape.
        if items.len() == 3 && public_elements.len() == 2 {
            items.truncate(2);
        }
        // Ordinary generic functions are type-erased: while their body runs,
        // an intrinsic such as `divmod` can have the symbolic checked result
        // `Tuple[T, T]`. There is deliberately no nominal implementation for an
        // open type. Keep the private pack transient through that boundary; the
        // direct-call instruction in the concrete caller carries the fully
        // substituted destination type and materializes its `Tuple` instance
        // below. A closed missing instance remains a compiler invariant error
        // rather than falling back to runtime guessing.
        if public_elements
            .iter()
            .any(|element| mojito_types::types::is_symbolic(element))
        {
            return Ok(Value::Tuple(items));
        }
        let definition = prog.structs.get(name).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: checked public Tuple result targets missing specialization '{name}'"
            ))
        })?;
        let [(field, Ty::Tuple(storage_elements))] = definition.fields.as_slice() else {
            return Err(RuntimeError::TypeError(format!(
                "vm: public Tuple specialization '{name}' does not have one private runtime-pack field"
            )));
        };
        // The erased template's storage is its pack, whatever its length.
        if field == "storage" && mojito_types::types::pack_spread(storage_elements).is_some() {
            return Ok(Value::Struct {
                name: name.clone(),
                fields: vec![(field.clone(), Value::Tuple(items))],
                value_params: Vec::new(),
            });
        }
        if field != "storage"
            || storage_elements.len() != items.len()
            || public_elements.len() != items.len()
            || !public_elements
                .iter()
                .zip(storage_elements)
                // Exact literals can survive on the expression result while
                // specialization deliberately materializes its executable
                // field (`IntLiteral` -> `Int`, for example). This is the same
                // checked, directional coercion used by MIR verification.
                .all(|(public, storage)| mojito_types::types::value_coerces(public, storage))
        {
            return Err(RuntimeError::TypeError(format!(
                "vm: public Tuple result does not match specialization '{name}' \
                 (public={public_elements:?}, storage={storage_elements:?}, arity={})",
                items.len()
            )));
        }
        let storage = Value::Tuple(
            items
                .into_iter()
                .zip(storage_elements)
                .map(|(item, ty)| crate::runtime::coerce_checked(item, ty))
                .collect(),
        );
        Ok(Value::Struct {
            name: name.clone(),
            fields: vec![(field.clone(), storage)],
            value_params: Vec::new(),
        })
    }

    /// Materialize a parameter constant's value at its checked type `target`:
    /// each tuple, at any depth of tuples and struct fields, becomes the
    /// nominal `Tuple` instance its checked type names over the private pack
    /// storage ([`Self::materialize_checked_result`]), and each string at the
    /// nominal `String` a `String` over a fresh buffer.
    pub(super) fn materialize_parameter_value(
        &mut self,
        prog: &Prog,
        value: Value,
        target: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        match value {
            Value::Tuple(items) => {
                let instance = target.and_then(|target| match target {
                    Ty::Struct(name, _) => {
                        prog.structs.get(name).map(|definition| (name, definition))
                    }
                    _ => None,
                });
                let storage =
                    instance.and_then(|(_, definition)| match definition.fields.as_slice() {
                        [(field, Ty::Tuple(elements))] => Some((field.clone(), elements.clone())),
                        _ => None,
                    });
                let items = items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let element = storage
                            .as_ref()
                            .and_then(|(_, elements)| elements.get(index));
                        self.materialize_parameter_value(prog, item, element)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // A concrete `Tuple` instance holds the elements as its
                // storage, each at its storage type.
                match (instance, storage) {
                    (Some((name, _)), Some((field, elements)))
                        if elements.len() == items.len()
                            && mojito_types::types::pack_spread(&elements).is_none() =>
                    {
                        let storage = items
                            .into_iter()
                            .zip(&elements)
                            .map(|(item, ty)| crate::runtime::coerce_checked(item, ty))
                            .collect();
                        Ok(Value::Struct {
                            name: name.clone(),
                            fields: vec![(field, Value::Tuple(storage))],
                            value_params: Vec::new(),
                        })
                    }
                    _ => self.materialize_checked_result(prog, Value::Tuple(items), target),
                }
            }
            Value::Struct {
                name,
                fields,
                value_params,
            } => {
                let declared = prog.structs.get(&name).map(|definition| &definition.fields);
                let fields = fields
                    .into_iter()
                    .map(|(field, value)| {
                        let ty = declared.and_then(|declared| {
                            declared
                                .iter()
                                .find(|(candidate, _)| *candidate == field)
                                .map(|(_, ty)| ty)
                        });
                        Ok((field, self.materialize_parameter_value(prog, value, ty)?))
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                Ok(Value::Struct {
                    name,
                    fields,
                    value_params,
                })
            }
            Value::Str(text)
                if matches!(target, Some(Ty::Struct(name, _))
                    if mojito_symbol::symbol::is_stdlib_string_struct(name)) =>
            {
                self.nominal_string_value(prog, &text)
            }
            value => Ok(value),
        }
    }

    /// Materialize the element a `comptime for` header just bound into its
    /// slot at the slot's checked type, as a parameter constant is
    /// ([`Self::materialize_parameter_value`]).
    pub(super) fn materialize_comptime_binder(
        &mut self,
        prog: &Prog,
        function: &MirFunction,
        header: &MirTerm,
        variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let MirTerm::ComptimeFor { slot, .. } = header else {
            return Ok(());
        };
        let value = std::mem::replace(&mut variables[*slot as usize], Value::None);
        variables[*slot as usize] =
            self.materialize_parameter_value(prog, value, function.var_tys.get(slot))?;
        Ok(())
    }

    /// Build an uninitialized `self` skeleton for `name` (fields = `None`), carrying
    /// the given reified `value_params`. Shared by `__init__`/`__copyinit__`/
    /// `__moveinit__` construction.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn struct_skeleton(
        &self,
        prog: &Prog,
        name: &str,
        value_params: Vec<(String, Value)>,
    ) -> Value {
        let fields = prog.structs[name]
            .fields
            .iter()
            .map(|(f, _)| (f.clone(), Value::None))
            .collect();
        Value::Struct {
            name: name.to_string(),
            fields,
            value_params,
        }
    }

    /// If `place` is `c[i]` with `c` a user struct or an `UnsafePointer`, read it via
    /// `c.__getitem__(i)` / the heap arena — the read half of `c[i] += e` on such a
    /// container (a projected `LoadPlace`). Returns `None` otherwise, so the caller
    /// uses `load_place` (a slot read or a SIMD-lane read).
    pub(super) fn load_index_dunder(
        &mut self,
        prog: &Prog,
        place: &MirPlace,
        regs: &[Value],
        vars: &mut [Value],
        frame_id: FrameId,
    ) -> Result<Option<Value>, RuntimeError> {
        let Some((Proj::Index(ireg), prefix)) = place.proj.split_last() else {
            return Ok(None);
        };
        let parent = MirPlace {
            root: place.root,
            root_ty: place.root_ty.clone(),
            proj: prefix.to_vec(),
            projection_tys: place.projection_tys[..prefix.len()].to_vec(),
            ty: if prefix.is_empty() {
                place.root_ty.clone()
            } else {
                place.projection_tys.get(prefix.len() - 1).cloned()
            },
            through: place.through,
        };
        // A parent reached through a `ref`-typed field (`self.src.data[i]`
        // with `src: ref[origin] Optional[T]`) is not raw storage: read it
        // through the reference walk, which chases stored handles
        // mid-projection, exactly like a plain `LoadPlace` does.
        let recv = if parent
            .projection_tys
            .iter()
            .any(|ty| matches!(ty, mojito_types::types::Ty::Ref(_)))
        {
            let composed = Value::Ref {
                frame: frame_id.0,
                slot: parent.root as usize,
                projection: Vec::new(),
            };
            let composed = self
                .extend_reference(&composed, &parent, regs)?
                .expect("a composed root handle extends");
            let value = self.read_reference(&composed, frame_id, vars)?;
            if matches!(value, Value::Ref { .. }) {
                self.read_reference(&value, frame_id, vars)?
            } else {
                value
            }
        } else {
            nav_mut(vars, regs, &parent)?.clone()
        };
        match &recv {
            Value::Struct { name, .. } => {
                let sname = name.clone();
                let idx = regs[ireg.0 as usize].clone();
                Ok(Some(self.call_dunder(
                    prog,
                    &sname,
                    "__getitem__",
                    vec![recv, idx],
                )?))
            }
            Value::Pointer { allocation, offset } => {
                let off = value_as_index(&regs[ireg.0 as usize])?;
                let value = self.heap_read(*allocation, *offset, off)?;
                Ok(Some(if self.has_copyinit {
                    self.clone_value(prog, &value)?
                } else {
                    value
                }))
            }
            _ => Ok(None),
        }
    }
}
