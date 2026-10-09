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
        })
    }

    /// Thaw frozen pointer memory into fresh allocations at the checked element
    /// type, leaving uninitialized slots unwritten. Materialize the value at `target`:
    /// each tuple, at any depth of tuples and struct fields, becomes the
    /// nominal `Tuple` instance its checked type names over the private pack
    /// storage ([`Self::materialize_checked_result`]), and each string at the
    /// nominal `String` a `String` over a fresh buffer.
    pub(super) fn thaw(
        &mut self,
        prog: &Prog,
        value: &mojito_types::ct::CtValue,
        target: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        use mojito_types::ct::CtValue;
        let materialized = match value {
            CtValue::Pointer { memory, offset } => {
                let Some(Ty::Pointer { element, .. }) = target else {
                    return Err(RuntimeError::Unsupported(
                        "frozen pointer has no checked element type".to_string(),
                    ));
                };
                let allocation = match memory {
                    None => 0,
                    Some(slots) => {
                        let Value::Pointer { allocation, .. } =
                            self.heap_alloc(slots.len() as i64, 8)?
                        else {
                            return Err(RuntimeError::Unsupported(
                                "allocation did not produce a pointer".to_string(),
                            ));
                        };
                        for (index, slot) in slots.iter().enumerate() {
                            if let Some(slot) = slot {
                                let value = self.thaw(prog, slot, Some(element))?;
                                self.heap_store(allocation as usize - 1, index, value);
                            }
                        }
                        allocation
                    }
                };
                Value::Pointer {
                    allocation,
                    offset: *offset,
                }
            }
            CtValue::Struct { name, fields } => {
                let declared = prog.structs.get(name).map(|definition| &definition.fields);
                let fields = fields
                    .iter()
                    .map(|(field, value)| {
                        let ty = declared.and_then(|fields| {
                            fields
                                .iter()
                                .find(|(name, _)| name == field)
                                .map(|(_, ty)| ty)
                        });
                        Ok((field.clone(), self.thaw(prog, value, ty)?))
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                Value::Struct {
                    name: name.clone(),
                    fields,
                }
            }
            CtValue::Tuple(items) => {
                let storage = match target {
                    Some(Ty::Struct(name, _)) => prog.structs.get(name).and_then(|definition| {
                        match definition.fields.as_slice() {
                            [(field, Ty::Tuple(types))] => Some((name, field, types.as_slice())),
                            _ => None,
                        }
                    }),
                    _ => None,
                };
                let types = match (storage, target) {
                    (Some((_, _, types)), _) => Some(types),
                    (None, Some(Ty::Tuple(types) | Ty::RuntimePack(types))) => {
                        Some(types.as_slice())
                    }
                    _ => None,
                };
                let items = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        self.thaw(prog, item, types.and_then(|types| types.get(index)))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // A concrete `Tuple` instance holds the elements as its
                // storage, each at its storage type.
                match storage {
                    Some((name, field, types))
                        if types.len() == items.len()
                            && mojito_types::types::pack_spread(types).is_none() =>
                    {
                        Value::Struct {
                            name: name.clone(),
                            fields: vec![(
                                field.clone(),
                                Value::Tuple(
                                    items
                                        .into_iter()
                                        .zip(types)
                                        .map(|(item, ty)| crate::runtime::coerce_checked(item, ty))
                                        .collect(),
                                ),
                            )],
                        }
                    }
                    _ => self.materialize_checked_result(prog, Value::Tuple(items), target)?,
                }
            }
            CtValue::Str(text) if matches!(target, Some(Ty::Struct(name, _)) if mojito_symbol::symbol::is_stdlib_string_struct(name)) => {
                self.nominal_string_value(prog, text)?
            }
            _ => crate::crossing::ct_to_vm(value)?,
        };
        Ok(match target {
            Some(ty) => crate::runtime::coerce_checked(materialized, ty),
            None => materialized,
        })
    }

    /// Build an uninitialized `self` skeleton for `name` (fields = `None`).
    /// Shared by `__init__`/`__copyinit__`/`__moveinit__` construction.
    pub(super) fn struct_skeleton(prog: &Prog, name: &str) -> Value {
        let fields = prog.structs[name]
            .fields
            .iter()
            .map(|(f, _)| (f.clone(), Value::None))
            .collect();
        Value::Struct {
            name: name.to_string(),
            fields,
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
        let place = &*place.storage();
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
