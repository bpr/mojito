//! Runtime reference handle read/write/projection, the reference-pointer
//! boundary, closure capture arguments, and reference extension.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// The storage a handle designates: a frame (or compile-time memory, or
/// the interned heap pointers), a slot of it, and a projection within.
pub(super) type HandleParts = (u64, usize, Vec<RefProjection>);

/// How many handle-to-handle hops a projection walk will chase before
/// treating the chain as cyclic. Reborrows nest as deep as the source does,
/// which is a handful of frames; this only has to be past that.
const MAX_REF_HOPS: usize = 64;

impl VmBackend {
    /// The variable `slot` of the live frame `frame`, or of compile-time
    /// memory when `frame` is [`COMPTIME_MEMORY_FRAME`], or of the interned
    /// heap pointers when it is [`POINTER_ROOT_FRAME`]: the storage a
    /// reference handle rooted outside the current frame designates.
    pub(super) fn frame_variable(&self, frame: u64, slot: usize) -> Option<&Value> {
        if frame == COMPTIME_MEMORY_FRAME {
            return self.comptime_memory.get(slot);
        }
        if frame == POINTER_ROOT_FRAME {
            return self.pointer_roots.get(slot);
        }
        self.frames
            .iter()
            .find(|candidate| candidate.id.0 == frame)
            .and_then(|owner| owner.variables.get(slot))
    }

    /// [`Self::frame_variable`], mutably.
    pub(super) fn frame_variable_mut(&mut self, frame: u64, slot: usize) -> Option<&mut Value> {
        if frame == COMPTIME_MEMORY_FRAME {
            return self.comptime_memory.get_mut(slot);
        }
        if frame == POINTER_ROOT_FRAME {
            return self.pointer_roots.get_mut(slot);
        }
        self.frames
            .iter_mut()
            .find(|candidate| candidate.id.0 == frame)
            .and_then(|owner| owner.variables.get_mut(slot))
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
        projection.extend(place_projection_segments(place, registers)?);
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
    /// A call result, or a reference made in place, bound to a reference
    /// register. When it was reached through a heap pointer held in a slot
    /// — a temporary `Span` over a list's elements or over static ones,
    /// which the body drops before it returns, or before the caller it was
    /// inlined into reads the reference — it designates the heap element,
    /// never that slot. A static-origin element is forwarded as the
    /// element's pointer; any other is rooted at the interned pointer
    /// reaching it ([`Self::pointer_root`]), so it still projects into the
    /// element.
    pub(super) fn heap_reference_result(
        &mut self,
        value: Value,
        target: Option<&Ty>,
        current: FrameId,
        current_variables: &[Value],
    ) -> Value {
        let Some(reference) = target.and_then(mojito_mir::mir::verify::reference_record) else {
            return value;
        };
        let static_origin = reference.origin == mojito_types::origin::Origin::Static;
        let Value::Ref {
            frame,
            slot,
            projection,
        } = &value
        else {
            return value;
        };
        let root = if *frame == current.0 {
            current_variables.get(*slot)
        } else {
            self.frame_variable(*frame, *slot)
        };
        let boundary = root
            .map(|root| self.reference_pointer_boundary(root, projection))
            .and_then(Result::ok)
            .flatten()
            .map(|boundary| {
                (
                    boundary.allocation,
                    boundary.offset,
                    boundary.index,
                    boundary.suffix.to_vec(),
                )
            });
        match boundary {
            Some((allocation, offset, index, suffix)) if static_origin && suffix.is_empty() => {
                Value::Pointer {
                    allocation,
                    offset: offset + index as i64,
                }
            }
            Some((allocation, offset, index, suffix)) if !static_origin => Value::Ref {
                frame: POINTER_ROOT_FRAME,
                slot: self.pointer_root(allocation, offset),
                projection: std::iter::once(RefProjection::Index(index))
                    .chain(suffix)
                    .collect(),
            },
            _ => value,
        }
    }

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
            }
            | Value::PlacePointer {
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
            Value::Struct { fields, .. } => {
                for (_, field) in fields {
                    self.canonicalize_value_references(current, current_variables, field);
                }
            }
            Value::ComptimeList(values)
            | Value::Tuple(values)
            | Value::Closure {
                captures: values, ..
            } => {
                for element in values {
                    self.canonicalize_value_references(current, current_variables, element);
                }
            }
            Value::Variant { value, .. } => {
                self.canonicalize_value_references(current, current_variables, value);
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
            self.frame_variable(frame, slot)
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
        // Otherwise walk the projection and re-root at the first handle that
        // segments still address through: a reference reached before the
        // final segment, or a place pointer a segment dereferences. A handle
        // reached at the final segment is itself the designated value (a
        // stored `Pointer` or `ref` element) and stays where it is.
        for (position, segment) in projection.iter().enumerate() {
            if let Value::PlacePointer {
                frame: pointer_frame,
                slot: pointer_slot,
                projection: pointer_projection,
            } = value
                && let Some(Ok((target_frame, target_slot, mut combined))) =
                    place_pointer_target(*pointer_frame, *pointer_slot, pointer_projection, segment)
            {
                combined.extend_from_slice(&projection[position + 1..]);
                return self.canonical_reference_parts(
                    current,
                    current_variables,
                    target_frame,
                    target_slot,
                    combined,
                );
            }
            match navigate_reference_step(value, segment) {
                Some(Value::Ref {
                    frame: handle_frame,
                    slot: handle_slot,
                    projection: handle_projection,
                }) if position + 1 < projection.len() => {
                    // A dereference of the stored handle is the handle's own
                    // referent, as a place pointer's is.
                    let rest = &projection[position + 1..];
                    let rest = match rest {
                        [RefProjection::Deref, rest @ ..] => rest,
                        rest => rest,
                    };
                    let mut combined = handle_projection.clone();
                    combined.extend_from_slice(rest);
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

    pub(super) fn read_reference(
        &self,
        reference: &Value,
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<Value, RuntimeError> {
        // A heap pointer forwarded as a handle (see `MakeRef`) reads its
        // pointee from the arena.
        if let Value::Pointer { allocation, offset } = reference {
            return self.heap_read(*allocation, *offset, 0);
        }
        let reference = &*address_reference(reference);
        if let Value::Ref {
            frame,
            slot,
            projection,
        } = reference
            && *frame == current.0
        {
            return self
                .read_reference_projection(
                    current,
                    current_variables,
                    &current_variables[*slot],
                    projection,
                )
                .and_then(defined);
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
        let root = self.frame_variable(*frame, *slot).ok_or_else(|| {
            RuntimeError::TypeError(format!("vm: stale reference to frame {frame}"))
        })?;
        self.read_reference_projection(current, current_variables, root, projection)
            .and_then(defined)
    }

    pub(super) fn write_reference(
        &mut self,
        reference: &Value,
        current: FrameId,
        current_variables: &mut [Value],
        value: Value,
    ) -> Result<(), RuntimeError> {
        // A heap pointer forwarded as a handle (see `MakeRef`) writes its
        // pointee in the arena.
        if let Value::Pointer { allocation, offset } = reference {
            let (region, slot) = self.heap_index(*allocation, *offset, 0)?;
            return self.write_heap_projection(region, slot, &[], value);
        }
        let reference = &*address_reference(reference);
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
        // A projection crossing a `ref`-typed field addresses the inner
        // handle's referent: chase stored handles to the ultimate storage
        // before navigating, symmetrically with `read_reference_projection`.
        let (frame, slot, projection) = self.canonical_reference_parts(
            current,
            current_variables,
            *frame,
            *slot,
            projection.clone(),
        );
        // A byte view's root holds no value to navigate: the write rebuilds
        // the held values its bytes cover.
        if let Some(view) = self.view_at(frame, slot) {
            let view = self.view_projection(view, &projection)?;
            return self.view_write(&view, &value, current, current_variables);
        }
        let (frame, slot, projection) = (&frame, &slot, &projection);
        if *frame == current.0 {
            return self.write_reference_projection(
                &mut current_variables[*slot],
                projection,
                value,
            );
        }
        let stale = || RuntimeError::TypeError(format!("vm: stale reference to frame {frame}"));
        // A pointer-index projection leaves frame-owned storage and enters the
        // VM heap. Locate that boundary from an immutable snapshot before
        // borrowing either storage arena mutably.
        if let Some(ReferencePointerBoundary {
            allocation,
            offset,
            index,
            suffix,
        }) = self.reference_pointer_boundary(
            self.frame_variable(*frame, *slot).ok_or_else(stale)?,
            projection,
        )? {
            let (region, heap_slot) = self.heap_index(allocation, offset, index as i64)?;
            self.write_heap_projection(region, heap_slot, suffix, value)?;
            return Ok(());
        }
        let root = self.frame_variable_mut(*frame, *slot).ok_or_else(stale)?;
        if write_uninit_payload(root, projection, value.clone())? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(root);
        let leaf = navigate_reference_mut(root, projection).map_err(|error| {
            RuntimeError::TypeError(format!(
                "{error}; reference root is {root_type} with projection {projection:?}"
            ))
        })?;
        *leaf = crate::runtime::keep_held(leaf, value);
        Ok(())
    }

    /// Read a projection that may cross from frame storage into an
    /// `Pointer` allocation. Reference handles deliberately retain a
    /// flattened projection, so this is the single runtime point that gives a
    /// typed MIR pointer-index projection its heap semantics.
    pub(super) fn read_reference_projection(
        &self,
        current: FrameId,
        current_variables: &[Value],
        root: &Value,
        projection: &[RefProjection],
    ) -> Result<Value, RuntimeError> {
        // A byte view's root reads the bytes its projection selects.
        if let Value::ByteView(view) = root {
            return self.view_read(
                &self.view_projection(view, projection)?,
                current,
                current_variables,
            );
        }
        let mut value = root.clone();
        for segment in projection {
            // A `ref`-typed field mid-projection stores another handle; the
            // remaining segments address its referent — chase the handle
            // exactly like reference canonicalization does. A reborrow makes
            // that a *chain*: a handle whose target slot holds another handle
            // reads back as a handle, since this walk returns an
            // empty-projection root untouched. Chase to the referent, or a
            // field read of the receiver behind a `ref` field (`self.size` in
            // `List.__getitem__`) meets a handle where it expects a struct.
            let mut hops = 0;
            while let Value::Ref { .. } = &value {
                if hops == MAX_REF_HOPS {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: reference handle chain exceeded {MAX_REF_HOPS} hops \
                         resolving projection {projection:?}"
                    )));
                }
                value = self.read_reference(&value, current, current_variables)?;
                hops += 1;
            }
            value = match (segment, value) {
                (RefProjection::Field(name), Value::Struct { fields, .. }) => fields
                    .into_iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| RuntimeError::TypeError(format!("no field '{name}'")))?,
                // A checked program indexes a tuple past its count only
                // through `VariadicList`'s untracked element pointer (`elems[i]`
                // past a pack's arguments), which native code reports as a
                // dangling dereference.
                (RefProjection::Index(index), Value::Tuple(items)) => {
                    items.get(*index).cloned().ok_or_else(|| {
                        RuntimeError::TypeError("vm: dereference of dangling Pointer".to_string())
                    })?
                }
                (RefProjection::Index(index), Value::Pointer { allocation, offset }) => {
                    self.heap_read(allocation, offset, *index as i64)?
                }
                (
                    segment @ (RefProjection::Index(_) | RefProjection::Deref),
                    Value::PlacePointer {
                        frame,
                        slot,
                        projection,
                    },
                ) => {
                    let (frame, slot, projection) =
                        place_pointer_target(frame, slot, &projection, segment).ok_or_else(
                            || {
                                RuntimeError::TypeError(format!(
                                    "vm: {segment:?} does not dereference a place pointer"
                                ))
                            },
                        )??;
                    self.read_reference(
                        &Value::Ref {
                            frame,
                            slot,
                            projection,
                        },
                        current,
                        current_variables,
                    )?
                }
                // A single-pointee `Pointer` whose value is a heap pointer
                // (an `OwnedPointer`'s `unsafe_ptr()`) dereferences its one
                // element.
                (RefProjection::Deref, Value::Pointer { allocation, offset }) => {
                    self.heap_read(allocation, offset, 0)?
                }
                (RefProjection::Index(index), Value::Struct { name, fields, .. })
                    if name == "List" || name.ends_with("$List") =>
                {
                    let (allocation, offset) = list_storage_pointer(&fields)?;
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
                                .map_or_else(|| "<invalid>".to_string(), ToString::to_string),
                            alternatives
                                .get(*expected)
                                .map_or_else(|| "<invalid>".to_string(), ToString::to_string)
                        )));
                    }
                    *value
                }
                (RefProjection::UninitPayload, Value::UninitStorage(payload)) => *payload
                    .ok_or_else(|| {
                        RuntimeError::TypeError(
                            "vm: read of uninitialized MaybeUninit storage".to_string(),
                        )
                    })?,
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
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn reference_pointer_boundary<'a>(
        &self,
        root: &Value,
        projection: &'a [RefProjection],
    ) -> Result<Option<ReferencePointerBoundary<'a>>, RuntimeError> {
        let mut value = root.clone();
        for (position, segment) in projection.iter().enumerate() {
            match (segment, value) {
                (
                    segment @ (RefProjection::Index(_) | RefProjection::Deref),
                    Value::Pointer { allocation, offset },
                ) => {
                    return Ok(Some(ReferencePointerBoundary {
                        allocation,
                        offset,
                        index: match segment {
                            RefProjection::Index(index) => *index,
                            _ => 0,
                        },
                        suffix: &projection[position + 1..],
                    }));
                }
                (RefProjection::Index(index), Value::Struct { name, fields, .. })
                    if name == "List" || name.ends_with("$List") =>
                {
                    let (allocation, offset) = list_storage_pointer(&fields)?;
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
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string),
                        alternatives
                            .get(*expected)
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string)
                    )));
                }
                // An initialized payload is ordinary frame-local storage to
                // scan through; an uninitialized one has no heap boundary
                // beyond it — the frame-local write/read logic decides whether
                // the access initializes (a final raw write) or traps.
                (RefProjection::UninitPayload, Value::UninitStorage(Some(payload))) => {
                    value = *payload;
                }
                (RefProjection::UninitPayload, Value::UninitStorage(None)) => return Ok(None),
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
        if write_uninit_payload(root, projection, value.clone())? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(root);
        let leaf = navigate_reference_mut(root, projection).map_err(|error| {
            RuntimeError::TypeError(format!(
                "{error}; reference root is {root_type} with projection {projection:?}"
            ))
        })?;
        *leaf = crate::runtime::keep_held(leaf, value);
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
            self.heap_store(region, slot, value);
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
        if write_uninit_payload(
            &mut self.heap[region].slots[slot],
            projection,
            value.clone(),
        )? {
            return Ok(());
        }
        let root_type = crate::runtime::type_name(&self.heap[region].slots[slot]);
        let leaf = navigate_reference_mut(&mut self.heap[region].slots[slot], projection).map_err(
            |error| {
                RuntimeError::TypeError(format!(
                    "{error}; heap reference root is {root_type} with projection {projection:?}"
                ))
            },
        )?;
        *leaf = crate::runtime::keep_held(leaf, value);
        Ok(())
    }

    /// The handle a place rooted at a reference binding that holds a heap
    /// pointer names (`ref r = p.__getitem__()` over an allocation): a
    /// reference to its pointee, projected by the place. `None` for any other place. The write and projection twin of
    /// the load's static-origin read through such a binding. A place
    /// projected from the binding (`e.items`) names the pointee whether or
    /// not it is spelled as read through the root.
    pub(super) fn heap_handle_place(
        &mut self,
        (function, declarations): (&MirFunction, &MirDeclarations),
        place: &MirPlace,
        registers: &[Value],
        variables: &[Value],
    ) -> Result<Option<Value>, RuntimeError> {
        let reference_root = function.binds_reference(place.root);
        let projected_from_reference = reference_root
            && !place.proj.is_empty()
            && !place_crosses_reference(place, declarations);
        if place.through != Some(place.root) && !projected_from_reference {
            return Ok(None);
        }
        let Value::Pointer { allocation, offset } = variables[place.root as usize] else {
            return Ok(None);
        };
        let segments = place_projection_segments(place, registers)?;
        // A `ref` field on the way (a closure's `mut` capture) stores a
        // handle of its own: the rest of the place lies behind it.
        let storage = place.storage();
        let declares_ref_field = |ty: &Ty, field: &str| declarations.declares_ref_field(ty, field);
        let wants_slot = place
            .ty
            .as_ref()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Ref(_)))
            && mojito_mir::mir::place_holds_reference(
                place,
                function.binds_reference(place.root),
                declares_ref_field,
            );
        let last = segments.len().saturating_sub(1);
        let mut handle = self.pointee_reference(allocation, offset);
        for (position, segment) in segments.into_iter().enumerate() {
            if let Value::Ref { projection, .. } = &mut handle {
                projection.push(segment);
            }
            let stores_handle =
                mojito_mir::mir::place_step_holds_reference(&storage, position, declares_ref_field);
            if stores_handle && !(position == last && wants_slot) {
                let roots = self.pointer_roots.clone();
                handle = match self.read_reference(&handle, FrameId(POINTER_ROOT_FRAME), &roots)? {
                    stored @ Value::Ref { .. } => stored,
                    Value::Pointer { allocation, offset } => {
                        self.pointee_reference(allocation, offset)
                    }
                    _ => return Ok(None),
                };
            }
        }
        Ok(Some(handle))
    }

    /// The reference to the pointee of the heap pointer
    /// `allocation`+`offset`.
    fn pointee_reference(&mut self, allocation: u64, offset: i64) -> Value {
        Value::Ref {
            frame: POINTER_ROOT_FRAME,
            slot: self.pointer_root(allocation, offset),
            projection: vec![RefProjection::Index(0)],
        }
    }

    /// The handle `place` designates through the handle its root slot
    /// holds, if any. A root whose declaration makes its handle an address
    /// (`addresses`: a binding, or a parameter
    /// [`mojito_mir::mir::address_parameters`] passes by address)
    /// extends that handle with the place's projections. A handle that is
    /// the slot's own value (a place pointer, or a reference held by a slot
    /// declared of reference type) is stepped through only by a
    /// dereferencing first segment; any other place is the slot's own.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn extend_reference(
        &self,
        root: &Value,
        addresses: bool,
        place: &MirPlace,
        registers: &[Value],
    ) -> Result<Option<Value>, RuntimeError> {
        match root {
            Value::Ref {
                frame,
                slot,
                projection,
            } if addresses => {
                let mut projection = projection.clone();
                projection.extend(place_projection_segments(place, registers)?);
                Ok(Some(Value::Ref {
                    frame: *frame,
                    slot: *slot,
                    projection,
                }))
            }
            Value::Ref {
                frame,
                slot,
                projection,
            }
            | Value::PlacePointer {
                frame,
                slot,
                projection,
            } => {
                let segments = place_projection_segments(place, registers)?;
                let Some((first, suffix)) = segments.split_first() else {
                    return Ok(None);
                };
                let Some(target) = place_pointer_target(*frame, *slot, projection, first) else {
                    return Ok(None);
                };
                let (frame, slot, mut projection) = target?;
                projection.extend_from_slice(suffix);
                Ok(Some(Value::Ref {
                    frame,
                    slot,
                    projection,
                }))
            }
            _ => Ok(None),
        }
    }

    /// The reference handle a place designates, if any: the root slot's own
    /// handle extended with the place's projections, or — for a place that
    /// only reaches a handle below its root ([`place_crosses_reference`]) — a
    /// handle composed at the root slot. `None` leaves the place to ordinary
    /// frame-storage navigation, which cannot interpret a [`Value::Ref`],
    /// as it does for the whole of a slot holding a pointer value.
    pub(super) fn place_handle(
        &self,
        (declarations, addresses): (&MirDeclarations, bool),
        frame_id: FrameId,
        place: &MirPlace,
        registers: &[Value],
        variables: &[Value],
    ) -> Result<Option<Value>, RuntimeError> {
        if let Some(reference) =
            self.extend_reference(&variables[place.root as usize], addresses, place, registers)?
        {
            return Ok(Some(reference));
        }
        if !place_crosses_reference(place, declarations) {
            return Ok(None);
        }
        Self::reference_to_place_parts(frame_id, registers, variables, place).map(Some)
    }

    /// Take the value `place` holds, leaving it moved: through the handle
    /// the place designates ([`Self::place_handle`]), a `mut` receiver's
    /// or parameter's field included, or in frame storage.
    pub(super) fn take_place(
        &mut self,
        root: (&MirDeclarations, bool),
        frame_id: FrameId,
        place: &MirPlace,
        registers: &[Value],
        variables: &mut [Value],
    ) -> Result<Value, RuntimeError> {
        match self.place_handle(root, frame_id, place, registers, variables)? {
            Some(reference) => {
                let old = self.read_reference(&reference, frame_id, variables)?;
                self.write_reference(&reference, frame_id, variables, Value::Moved)?;
                Ok(old)
            }
            None => Ok(std::mem::replace(
                nav_mut(variables, registers, place)?,
                Value::Moved,
            )),
        }
    }
}

/// The place pointer `handle` moved `delta` elements along the aggregate
/// its last index projection selects within (the call's array of a
/// `VariadicList`'s element pointers), as pointer arithmetic within one
/// array moves a native address. A handle selecting no element designates
/// one value, which only offset 0 reaches.
pub(super) fn offset_place_handle(handle: Value, delta: i64) -> Result<Value, RuntimeError> {
    let Value::PlacePointer {
        frame,
        slot,
        mut projection,
    } = handle
    else {
        return Err(RuntimeError::TypeError(
            "vm: pointer arithmetic expects a place pointer".to_string(),
        ));
    };
    offset_place_projection(&mut projection, delta)?;
    Ok(Value::PlacePointer {
        frame,
        slot,
        projection,
    })
}

/// The storage the place pointer `(frame, slot, projection)` designates
/// after the dereferencing `segment`: its pointee for a `Deref`, and the
/// element `i` past it for an `Index(i)`, as `lit.ref.from_pointer` of the
/// offset pointer addresses it. `None` for a segment that does not
/// dereference a pointer.
pub(super) fn place_pointer_target(
    frame: u64,
    slot: usize,
    projection: &[RefProjection],
    segment: &RefProjection,
) -> Option<Result<HandleParts, RuntimeError>> {
    let delta = match segment {
        RefProjection::Deref => 0,
        RefProjection::Index(index) => *index as i64,
        _ => return None,
    };
    let mut projection = projection.to_vec();
    Some(offset_place_projection(&mut projection, delta).map(|()| (frame, slot, projection)))
}

/// The element a place rooted at a slot holding the heap pointer `pointer`
/// projects (`q[i]._size`, a `Span` member's `self._size` inlined at the
/// pointee place `q[i]`): the pointer offset by the leading element index
/// or dereference, as `ref.make` of such a place forwards it, beside the
/// segments that project the element. `None` when the place does not
/// begin by selecting an element, or selects nothing within it.
pub(super) fn pointer_element(
    pointer: &Value,
    place: &MirPlace,
    registers: &[Value],
) -> Result<Option<(Value, Vec<RefProjection>)>, RuntimeError> {
    let Value::Pointer { allocation, offset } = pointer else {
        return Ok(None);
    };
    if !place.root_ty.as_ref().is_some_and(Ty::is_pointer) {
        return Ok(None);
    }
    let segments = place_projection_segments(place, registers)?;
    let Some((first, suffix)) = segments.split_first() else {
        return Ok(None);
    };
    let offset = match first {
        RefProjection::Deref => *offset,
        RefProjection::Index(index) => *offset + *index as i64,
        _ => return Ok(None),
    };
    if suffix.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        Value::Pointer {
            allocation: *allocation,
            offset,
        },
        suffix.to_vec(),
    )))
}

/// Whether a checked storage type is a single-pointee tracked pointer — a
/// `to=place` pointer designating exactly one value
/// ([`mojito_types::origin::Origin::single_place`]). Indexing such storage
/// is the offset-0 dereference, not element selection.
pub(super) fn single_pointee_pointer(ty: Option<&Ty>) -> bool {
    ty.and_then(Ty::pointer_lit_ref)
        .is_some_and(|reference| reference.origin.single_place())
}

/// Whether a place reaches a stored handle *below* its root: a field
/// declared `ref` or a pointer field along the way (`self.value.items`
/// through a `ref` field, `p.src[].v` through a `Pointer[Box, o]` field,
/// `self.data[i]._scalar` through a `List`'s element pointer), by
/// [`mojito_mir::mir::place_crosses_declared_reference`]. Such a place is
/// not raw frame storage — the handle designates storage this frame may
/// not own — so every access to it routes through the reference walk, which
/// chases stored handles mid-projection. The final step is excluded: it
/// names the slot the access lands on, not a step through it.
pub(super) fn place_crosses_reference(place: &MirPlace, declarations: &MirDeclarations) -> bool {
    mojito_mir::mir::place_crosses_declared_reference(place, |ty, field| {
        declarations.declares_ref_field(ty, field)
    })
}

/// Whether a place designates one element slot of multi-element raw pointer
/// storage (`self.data[i]`). A store there fills the slot itself even when
/// the element type is a reference: the slot holds the handle, and may still
/// be uninitialized, so there is no referent to write through.
pub(super) fn pointer_element_slot(place: &MirPlace) -> bool {
    let place = &*place.storage();
    let base_ty = match place.proj.len() {
        0 => return false,
        1 => place.root_ty.as_ref(),
        length => place.projection_tys.get(length - 2),
    };
    matches!(place.proj.last(), Some(Proj::Index(_)))
        && base_ty.is_some_and(Ty::is_pointer)
        && !single_pointee_pointer(base_ty)
}

/// The reference-projection segments of a MIR place's projections. An index
/// step whose base storage is a single-pointee pointer becomes the identity
/// [`RefProjection::Deref`] — the stored handle already designates the
/// pointee — rather than an element index into it.
pub(super) fn place_projection_segments(
    place: &MirPlace,
    registers: &[Value],
) -> Result<Vec<RefProjection>, RuntimeError> {
    let place = &*place.storage();
    let mut segments = Vec::with_capacity(place.proj.len());
    for (position, segment) in place.proj.iter().enumerate() {
        let base_ty = if position == 0 {
            place.root_ty.as_ref()
        } else {
            place.projection_tys.get(position - 1)
        };
        segments.push(match segment {
            Proj::Field(name) => RefProjection::Field(name.clone()),
            Proj::Index(_) | Proj::ConstIndex(_) if single_pointee_pointer(base_ty) => {
                RefProjection::Deref
            }
            Proj::Index(register) => {
                RefProjection::Index(value_as_index(&registers[register.0 as usize])? as usize)
            }
            Proj::ConstIndex(index) => RefProjection::Index(*index),
            Proj::Variant(index) => RefProjection::Variant(known_variant_index(index)?),
            Proj::UninitPayload => RefProjection::UninitPayload,
            Proj::Rebind => continue,
            Proj::FieldAt(_) => return Err(super::symbolic_field_step()),
        });
    }
    Ok(segments)
}

/// Step one reference-projection segment into a concrete frame-stored value while
/// re-rooting a reference (`canonical_reference_parts`). Only frame-local storage
/// is navigated — struct fields and tuple/variant/closure payloads; crossing into
/// pointer or list-storage heap, or a SIMD lane, returns `None`, leaving the rest
/// of the projection to resolve lazily at read time against the surviving root.
pub(super) fn navigate_reference_step<'a>(
    value: &'a Value,
    segment: &RefProjection,
) -> Option<&'a Value> {
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
        (RefProjection::UninitPayload, Value::UninitStorage(payload)) => payload.as_deref(),
        _ => None,
    }
}

/// The buffer a self-hosted `List` indexes: its `_storage` `Allocation`'s
/// `ThinAllocation` pointer.
fn list_storage_pointer(fields: &[(String, Value)]) -> Result<(u64, i64), RuntimeError> {
    let field = |fields: &[(String, Value)], wanted: &str| {
        fields
            .iter()
            .find(|(field, _)| field == wanted)
            .map(|(_, value)| value.clone())
    };
    let nested = |value: Option<Value>, wanted: &str| match value {
        Some(Value::Struct { fields, .. }) => field(&fields, wanted),
        _ => None,
    };
    match nested(nested(field(fields, "_storage"), "_alloc"), "_ptr") {
        Some(Value::Pointer { allocation, offset }) => Ok((allocation, offset)),
        _ => Err(RuntimeError::TypeError(
            "self-hosted List has no pointer storage".into(),
        )),
    }
}

/// Move a place pointer's projection `delta` elements along the aggregate
/// its last index selects within ([`offset_place_handle`]).
fn offset_place_projection(
    projection: &mut [RefProjection],
    delta: i64,
) -> Result<(), RuntimeError> {
    if delta == 0 {
        return Ok(());
    }
    let Some(RefProjection::Index(index)) = projection.last_mut() else {
        return Err(RuntimeError::TypeError(
            "vm: a place pointer designates a single value; only offset 0 can be \
             dereferenced"
                .to_string(),
        ));
    };
    *index = usize::try_from(*index as i64 + delta).map_err(|_| {
        RuntimeError::TypeError("vm: place pointer offset before its array".to_string())
    })?;
    Ok(())
}

/// The reference to the storage `address` designates: a place pointer's
/// pointee, as `lit.ref.from_pointer` addresses it, and a reference itself.
fn address_reference(address: &Value) -> std::borrow::Cow<'_, Value> {
    match address {
        Value::PlacePointer {
            frame,
            slot,
            projection,
        } => std::borrow::Cow::Owned(Value::Ref {
            frame: *frame,
            slot: *slot,
            projection: projection.clone(),
        }),
        reference => std::borrow::Cow::Borrowed(reference),
    }
}

/// `value`, read through a reference, unless it is [`Value::Undef`], whose
/// read traps.
fn defined(value: Value) -> Result<Value, RuntimeError> {
    match value {
        Value::Undef => Err(undef_read()),
        value => Ok(value),
    }
}
