//! Runtime reference handle read/write/projection, the reference-pointer
//! boundary, closure capture arguments, and reference extension.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

use super::*;

impl VmBackend {
    pub(super) fn reference_to_place(
        &self,
        frame: &Frame,
        place: &MirPlace,
    ) -> Result<Value, RuntimeError> {
        Self::reference_to_place_parts(frame.id, &frame.registers, &frame.variables, place)
    }

    pub(super) fn reference_to_place_parts(
        frame_id: FrameId,
        registers: &[Value],
        variables: &[Value],
        place: &MirPlace,
    ) -> Result<Value, RuntimeError> {
        let (target_frame, slot, mut projection) = match &variables[place.root as usize] {
            Value::Ref {
                frame,
                slot,
                projection,
            } => (*frame, *slot, projection.clone()),
            _ => (frame_id.0, place.root as usize, Vec::new()),
        };
        for segment in &place.proj {
            projection.push(match segment {
                Proj::Field(name) => RefProjection::Field(name.clone()),
                Proj::Index(register) => {
                    RefProjection::Index(value_as_index(&registers[register.0 as usize])? as usize)
                }
                Proj::ConstIndex(index) => RefProjection::Index(*index),
                Proj::Variant(index) => RefProjection::Variant(*index),
            });
        }
        Ok(Value::Ref {
            frame: target_frame,
            slot,
            projection,
        })
    }

    /// Re-root a reference at the storage it ultimately designates. Applied to a
    /// reference *returned* from a method while the returning frame is still live:
    /// a reference projected through a stored handle — a `ref[origin]` field's
    /// element — is rooted at that frame and would dangle once it unwinds. Follow
    /// the root handle and the first handle reached along the projection,
    /// recursively across frames, until the reference is rooted in surviving
    /// storage (a live caller frame, reached even through a `mut`/`ref self`
    /// receiver handle). Segments past an unnavigable step (a pointer or
    /// list-storage index into the heap) are retained for lazy read-time
    /// resolution against that surviving root. When nothing along the path is a
    /// stored handle, the result is the original `(frame, slot, projection)` — so a
    /// directly-returned handle and an ordinary local reference are unchanged.
    /// Re-root every reference handle inside a value that is about to outlive
    /// this frame — a returned value or a `mut` writeback — while the frame's
    /// variables are still live. A handle rooted at borrowed storage outside
    /// the frame is unchanged; one rooted here is re-rooted at the storage it
    /// projects through, exactly like a directly returned handle.
    pub(super) fn canonicalize_value_references(
        &self,
        current: FrameId,
        current_variables: &[Value],
        value: &mut Value,
    ) {
        match value {
            Value::Ref {
                frame,
                slot,
                projection,
            } => {
                let (new_frame, new_slot, new_projection) = self.canonical_reference_parts(
                    current,
                    current_variables,
                    *frame,
                    *slot,
                    std::mem::take(projection),
                );
                *frame = new_frame;
                *slot = new_slot;
                *projection = new_projection;
            }
            Value::Struct {
                fields,
                value_params,
                ..
            } => {
                for (_, field) in fields {
                    self.canonicalize_value_references(current, current_variables, field);
                }
                for (_, parameter) in value_params {
                    self.canonicalize_value_references(current, current_variables, parameter);
                }
            }
            Value::ComptimeList(values) | Value::Tuple(values) => {
                for element in values {
                    self.canonicalize_value_references(current, current_variables, element);
                }
            }
            Value::Variant { value, .. } => {
                self.canonicalize_value_references(current, current_variables, value);
            }
            Value::Closure { captures, .. } => {
                for capture in captures {
                    self.canonicalize_value_references(
                        current,
                        current_variables,
                        &mut capture.value,
                    );
                }
            }
            _ => {}
        }
    }

    pub(super) fn canonical_reference_parts(
        &self,
        current: FrameId,
        current_variables: &[Value],
        frame: u64,
        slot: usize,
        projection: Vec<RefProjection>,
    ) -> (u64, usize, Vec<RefProjection>) {
        let storage = if frame == current.0 {
            current_variables.get(slot)
        } else {
            self.frames
                .iter()
                .find(|candidate| candidate.id.0 == frame)
                .and_then(|owner| owner.variables.get(slot))
        };
        let Some(mut value) = storage else {
            return (frame, slot, projection);
        };
        // A handle stored directly in the root slot: adopt it and prepend its
        // own projection ahead of ours.
        if let Value::Ref {
            frame: handle_frame,
            slot: handle_slot,
            projection: handle_projection,
        } = value
        {
            let mut combined = handle_projection.clone();
            combined.extend(projection);
            return self.canonical_reference_parts(
                current,
                current_variables,
                *handle_frame,
                *handle_slot,
                combined,
            );
        }
        // Otherwise walk the projection and re-root at the first handle reached.
        for (position, segment) in projection.iter().enumerate() {
            match navigate_reference_step(value, segment) {
                Some(Value::Ref {
                    frame: handle_frame,
                    slot: handle_slot,
                    projection: handle_projection,
                }) => {
                    let mut combined = handle_projection.clone();
                    combined.extend_from_slice(&projection[position + 1..]);
                    return self.canonical_reference_parts(
                        current,
                        current_variables,
                        *handle_frame,
                        *handle_slot,
                        combined,
                    );
                }
                Some(next) => value = next,
                None => break,
            }
        }
        (frame, slot, projection)
    }

    /// Turn a closure environment into the leading reference arguments of its
    /// lifted function. Reference captures already are handles. Owned copy/move
    /// captures instead borrow the stable slot inside the closure value itself,
    /// which preserves mutations across repeated invocations.
    pub(super) fn closure_capture_arguments(
        frame_id: FrameId,
        registers: &[Value],
        variables: &[Value],
        callee_place: Option<&MirPlace>,
        captures: &[ClosureCapture],
    ) -> Result<Vec<Value>, RuntimeError> {
        captures
            .iter()
            .enumerate()
            .map(|(index, capture)| {
                if !capture.owned {
                    return Ok(capture.value.clone());
                }
                let place = callee_place.ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: an owned closure environment must be called from stable storage"
                            .to_string(),
                    )
                })?;
                let mut reference =
                    Self::reference_to_place_parts(frame_id, registers, variables, place)?;
                let Value::Ref { projection, .. } = &mut reference else {
                    unreachable!("reference_to_place_parts always returns a handle")
                };
                projection.push(RefProjection::Capture(index));
                Ok(reference)
            })
            .collect()
    }

    pub(super) fn read_reference(
        &self,
        reference: &Value,
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<Value, RuntimeError> {
        if let Value::Ref {
            frame,
            slot,
            projection,
        } = reference
            && *frame == current.0
        {
            return self.read_reference_projection(&current_variables[*slot], projection);
        }
        let Value::Ref {
            frame,
            slot,
            projection,
        } = reference
        else {
            return Err(RuntimeError::TypeError(
                "vm: expected reference handle".to_string(),
            ));
        };
        let owner = self
            .frames
            .iter()
            .find(|candidate| candidate.id.0 == *frame)
            .ok_or_else(|| {
                RuntimeError::TypeError(format!("vm: stale reference to frame {frame}"))
            })?;
        self.read_reference_projection(&owner.variables[*slot], projection)
    }

    pub(super) fn write_reference(
        &mut self,
        reference: &Value,
        current: FrameId,
        current_variables: &mut [Value],
        value: Value,
    ) -> Result<(), RuntimeError> {
        if let Value::Ref {
            frame,
            slot,
            projection,
        } = reference
            && *frame == current.0
        {
            return self.write_reference_projection(
                &mut current_variables[*slot],
                projection,
                value,
            );
        }
        let Value::Ref {
            frame,
            slot,
            projection,
        } = reference
        else {
            return Err(RuntimeError::TypeError(
                "vm: expected reference handle".to_string(),
            ));
        };
        let owner_index = self
            .frames
            .iter()
            .position(|candidate| candidate.id.0 == *frame)
            .ok_or_else(|| {
                RuntimeError::TypeError(format!("vm: stale reference to frame {frame}"))
            })?;
        // A pointer-index projection leaves frame-owned storage and enters the
        // VM heap. Locate that boundary from an immutable snapshot before
        // borrowing either storage arena mutably.
        if let Some(ReferencePointerBoundary {
            allocation,
            offset,
            index,
            suffix,
        }) =
            self.reference_pointer_boundary(&self.frames[owner_index].variables[*slot], projection)?
        {
            let (region, heap_slot) = self.heap_index(allocation, offset, index as i64)?;
            self.write_heap_projection(region, heap_slot, suffix, value)?;
            return Ok(());
        }
        if write_simd_reference_lane(
            &mut self.frames[owner_index].variables[*slot],
            projection,
            value.clone(),
        )? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(&self.frames[owner_index].variables[*slot]);
        *navigate_reference_mut(&mut self.frames[owner_index].variables[*slot], projection)
            .map_err(|error| {
                RuntimeError::TypeError(format!(
                    "{error}; reference root is {root_type} with projection {projection:?}"
                ))
            })? = value;
        Ok(())
    }

    /// Read a projection that may cross from frame storage into an
    /// `UnsafePointer` allocation. Reference handles deliberately retain a
    /// flattened projection, so this is the single runtime point that gives a
    /// typed MIR pointer-index projection its heap semantics.
    pub(super) fn read_reference_projection(
        &self,
        root: &Value,
        projection: &[RefProjection],
    ) -> Result<Value, RuntimeError> {
        let mut value = root.clone();
        for segment in projection {
            value = match (segment, value) {
                (RefProjection::Field(name), Value::Struct { fields, .. }) => fields
                    .into_iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| RuntimeError::TypeError(format!("no field '{name}'")))?,
                (RefProjection::Index(index), Value::Tuple(items)) => {
                    items.get(*index).cloned().ok_or_else(|| {
                        RuntimeError::TypeError("reference index out of bounds".to_string())
                    })?
                }
                (RefProjection::Index(index), Value::Simd { dtype, lanes }) => {
                    read_simd_lane(dtype, &lanes, *index as i64)?
                }
                (RefProjection::Index(index), Value::Pointer { allocation, offset }) => {
                    self.heap_read(allocation, offset, *index as i64)?
                }
                (RefProjection::Index(index), Value::Struct { name, fields, .. })
                    if name == "List" || name.ends_with("$List") =>
                {
                    let (allocation, offset) = fields
                        .into_iter()
                        .find_map(|(field, value)| match (field.as_str(), value) {
                            ("data", Value::Pointer { allocation, offset }) => {
                                Some((allocation, offset))
                            }
                            _ => None,
                        })
                        .ok_or_else(|| {
                            RuntimeError::TypeError(
                                "self-hosted List has no pointer storage".to_string(),
                            )
                        })?;
                    self.heap_read(allocation, offset, *index as i64)?
                }
                (
                    RefProjection::Variant(expected),
                    Value::Variant {
                        alternatives,
                        index,
                        value,
                    },
                ) => {
                    if index != *expected {
                        return Err(RuntimeError::TypeError(format!(
                            "Variant holds '{}', not '{}'",
                            alternatives
                                .get(index)
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "<invalid>".to_string()),
                            alternatives
                                .get(*expected)
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "<invalid>".to_string())
                        )));
                    }
                    *value
                }
                (RefProjection::Capture(index), Value::Closure { captures, .. }) => captures
                    .get(*index)
                    .map(|capture| capture.value.clone())
                    .ok_or_else(|| {
                        RuntimeError::TypeError("closure capture index out of bounds".to_string())
                    })?,
                // Offset-0 identity deref of an origin-erased single-pointee
                // `to=place` pointer. `UnsafePointer(to=v)` lowers to a `Value::Ref`
                // straight at `v`, so `self.p[0]` re-roots (across a return) at the
                // pointee itself with a residual `Index(0)`. That pointer designates
                // exactly one value — the checker rejects any other offset — so
                // index 0 is the value. A non-collection pointee (a scalar, or a
                // non-List struct falling through the guarded List arm) resolves here.
                (RefProjection::Index(0), value) => value,
                (segment, value) => {
                    return Err(RuntimeError::TypeError(format!(
                        "invalid reference projection {segment:?} on {}",
                        crate::runtime::type_name(&value)
                    )));
                }
            };
        }
        Ok(value)
    }

    /// Return the first pointer-index boundary in a flattened reference
    /// projection. `suffix` is applied to the selected heap slot after
    /// dereferencing it.
    pub(super) fn reference_pointer_boundary<'a>(
        &self,
        root: &Value,
        projection: &'a [RefProjection],
    ) -> Result<Option<ReferencePointerBoundary<'a>>, RuntimeError> {
        let mut value = root.clone();
        for (position, segment) in projection.iter().enumerate() {
            match (segment, value) {
                (RefProjection::Index(index), Value::Pointer { allocation, offset }) => {
                    return Ok(Some(ReferencePointerBoundary {
                        allocation,
                        offset,
                        index: *index,
                        suffix: &projection[position + 1..],
                    }));
                }
                (RefProjection::Index(index), Value::Struct { name, fields, .. })
                    if name == "List" || name.ends_with("$List") =>
                {
                    let (allocation, offset) = fields
                        .into_iter()
                        .find_map(|(field, value)| match (field.as_str(), value) {
                            ("data", Value::Pointer { allocation, offset }) => {
                                Some((allocation, offset))
                            }
                            _ => None,
                        })
                        .ok_or_else(|| {
                            RuntimeError::TypeError(
                                "self-hosted List has no pointer storage".to_string(),
                            )
                        })?;
                    return Ok(Some(ReferencePointerBoundary {
                        allocation,
                        offset,
                        index: *index,
                        suffix: &projection[position + 1..],
                    }));
                }
                (RefProjection::Field(name), Value::Struct { fields, .. }) => {
                    value = fields
                        .into_iter()
                        .find(|(field, _)| field == name)
                        .map(|(_, value)| value)
                        .ok_or_else(|| RuntimeError::TypeError(format!("no field '{name}'")))?;
                }
                (RefProjection::Index(index), Value::Tuple(items)) => {
                    value = items.get(*index).cloned().ok_or_else(|| {
                        RuntimeError::TypeError("reference index out of bounds".to_string())
                    })?;
                }
                (RefProjection::Capture(index), Value::Closure { captures, .. }) => {
                    value = captures
                        .get(*index)
                        .map(|capture| capture.value.clone())
                        .ok_or_else(|| {
                            RuntimeError::TypeError(
                                "closure capture index out of bounds".to_string(),
                            )
                        })?;
                }
                (
                    RefProjection::Variant(expected),
                    Value::Variant {
                        alternatives: _,
                        index,
                        value: inner,
                    },
                ) if index == *expected => value = *inner,
                (
                    RefProjection::Variant(expected),
                    Value::Variant {
                        alternatives,
                        index,
                        ..
                    },
                ) => {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{}', not '{}'",
                        alternatives
                            .get(index)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string()),
                        alternatives
                            .get(*expected)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string())
                    )));
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    pub(super) fn write_reference_projection(
        &mut self,
        root: &mut Value,
        projection: &[RefProjection],
        value: Value,
    ) -> Result<(), RuntimeError> {
        if let Some(ReferencePointerBoundary {
            allocation,
            offset,
            index,
            suffix,
        }) = self.reference_pointer_boundary(root, projection)?
        {
            let (region, heap_slot) = self.heap_index(allocation, offset, index as i64)?;
            self.write_heap_projection(region, heap_slot, suffix, value)?;
            return Ok(());
        }
        if write_simd_reference_lane(root, projection, value.clone())? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(root);
        *navigate_reference_mut(root, projection).map_err(|error| {
            RuntimeError::TypeError(format!(
                "{error}; reference root is {} with projection {projection:?}",
                root_type
            ))
        })? = value;
        Ok(())
    }

    /// Continue a reference write after it has entered heap storage. Nested
    /// self-hosted collections can cross another pointer boundary, so suffixes
    /// are resolved recursively instead of being handed to the frame-only
    /// projection navigator.
    pub(super) fn write_heap_projection(
        &mut self,
        region: usize,
        slot: usize,
        projection: &[RefProjection],
        value: Value,
    ) -> Result<(), RuntimeError> {
        if projection.is_empty() {
            self.heap[region].slots[slot] = value;
            return Ok(());
        }
        if let Some(ReferencePointerBoundary {
            allocation,
            offset,
            index,
            suffix,
        }) = self.reference_pointer_boundary(&self.heap[region].slots[slot], projection)?
        {
            let (next_region, next_slot) = self.heap_index(allocation, offset, index as i64)?;
            return self.write_heap_projection(next_region, next_slot, suffix, value);
        }
        if write_simd_reference_lane(
            &mut self.heap[region].slots[slot],
            projection,
            value.clone(),
        )? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(&self.heap[region].slots[slot]);
        *navigate_reference_mut(&mut self.heap[region].slots[slot], projection).map_err(
            |error| {
                RuntimeError::TypeError(format!(
                    "{error}; heap reference root is {root_type} with projection {projection:?}"
                ))
            },
        )? = value;
        Ok(())
    }

    pub(super) fn extend_reference(
        &self,
        root: &Value,
        projection_path: &[Proj],
        registers: &[Value],
    ) -> Result<Option<Value>, RuntimeError> {
        let Value::Ref {
            frame,
            slot,
            projection,
        } = root
        else {
            return Ok(None);
        };
        let mut projection = projection.clone();
        for segment in projection_path {
            projection.push(match segment {
                Proj::Field(name) => RefProjection::Field(name.clone()),
                Proj::Index(register) => {
                    RefProjection::Index(value_as_index(&registers[register.0 as usize])? as usize)
                }
                Proj::ConstIndex(index) => RefProjection::Index(*index),
                Proj::Variant(index) => RefProjection::Variant(*index),
            });
        }
        Ok(Some(Value::Ref {
            frame: *frame,
            slot: *slot,
            projection,
        }))
    }
}

/// Step one reference-projection segment into a concrete frame-stored value while
/// re-rooting a reference (`canonical_reference_parts`). Only frame-local storage
/// is navigated — struct fields and tuple/variant/closure payloads; crossing into
/// pointer or list-storage heap, or a SIMD lane, returns `None`, leaving the rest
/// of the projection to resolve lazily at read time against the surviving root.
fn navigate_reference_step<'a>(value: &'a Value, segment: &RefProjection) -> Option<&'a Value> {
    match (segment, value) {
        (RefProjection::Field(name), Value::Struct { fields, .. }) => fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value),
        (RefProjection::Index(index), Value::Tuple(items)) => items.get(*index),
        (
            RefProjection::Variant(expected),
            Value::Variant {
                index,
                value: payload,
                ..
            },
        ) if index == expected => Some(payload),
        (RefProjection::Capture(index), Value::Closure { captures, .. }) => {
            captures.get(*index).map(|capture| &capture.value)
        }
        _ => None,
    }
}
