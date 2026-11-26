//! Byte views of typed storage: a `pop.pointer.bitcast` whose two pointees
//! lay out differently, and the typed reads and writes through one.
//!
//! The VM holds one typed value per heap or frame slot. A view reads the
//! bytes `mojito-native-core`'s layout gives that storage at an address, as
//! an LLVM `load` does, and a write through it rebuilds each value whose
//! bytes it covers. Extracted from `backend/vm.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::runtime::{ByteView, Retype, SimdLanes};
use mojito_ast::ast::Dtype;
use mojito_native_core::layout::{LayoutCx, StructFieldIndex};
use mojito_types::types::TyKind;

/// The struct table and field layouts of the program a VM runs
/// ([`VmBackend::configure`]), which a byte view lays its storage out by:
/// the table the native layout reads.
#[derive(Clone)]
pub(super) struct ProgramLayout {
    pub(super) structs: Rc<HashMap<String, StructDef>>,
    pub(super) fields: Rc<StructFieldIndex>,
}

/// A byte view's identity among the VM's pointer roots
/// ([`VmBackend::view_root`]).
#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct ViewKey {
    storage: ViewStorage,
    held: Ty,
    view: Ty,
    byte: i64,
}

impl VmBackend {
    /// `pop.pointer.bitcast` of the address `source` from a `from` pointer
    /// to a `to` pointer.
    ///
    /// A [`Retype::Plain`] retype keeps the pointer. Any other is a byte
    /// view of the storage `source` addresses, interned as a pointer root
    /// and addressed by an ordinary place pointer, so pointer arithmetic,
    /// `Int(p)`, and reference formation handle it as they handle any
    /// other. A view of a view flattens to a view of the first one's
    /// storage.
    pub(super) fn pointer_bitcast(
        &mut self,
        prog: &Prog,
        source: Value,
        from: Option<&Ty>,
        to: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        if crate::runtime::pointer_retype(from, to)? == Retype::Plain {
            return Ok(address_as(&prog.structs, source, to));
        }
        let pointee = |ty: Option<&Ty>| {
            ty.and_then(crate::runtime::bitcast_pointee).ok_or_else(|| {
                RuntimeError::TypeError("vm: pointer.bitcast requires Pointer operands".to_string())
            })
        };
        let (held, view) = (pointee(from)?, pointee(to)?);
        let mut bytes = match handle_parts(&source) {
            Some((frame, slot, projection)) if let Some(root) = self.view_at(frame, slot) => {
                self.view_projection(root, projection)?
            }
            _ => ByteView {
                storage: self.view_storage(source)?,
                held,
                view: view.clone(),
                byte: 0,
            },
        };
        bytes.view = view;
        self.layout_size(&bytes.held)?;
        self.layout_size(&bytes.view)?;
        let slot = self.view_root(bytes)?;
        Ok(address_as(
            &prog.structs,
            Value::PlacePointer {
                frame: POINTER_ROOT_FRAME,
                slot,
                projection: vec![RefProjection::Index(0)],
            },
            to,
        ))
    }

    /// The byte view the pointer root `(frame, slot)` holds, if it is one.
    pub(super) fn view_at(&self, frame: u64, slot: usize) -> Option<&ByteView> {
        match self.pointer_roots.get(slot) {
            Some(Value::ByteView(view)) if frame == POINTER_ROOT_FRAME => Some(view),
            _ => None,
        }
    }

    /// The view `projection` narrows `view` to: its first segment steps by
    /// whole view elements, as a pointer's index does, and the rest select
    /// a field or an array element within one.
    pub(super) fn view_projection(
        &self,
        view: &ByteView,
        projection: &[RefProjection],
    ) -> Result<ByteView, RuntimeError> {
        let mut narrowed = view.clone();
        let Some((step, rest)) = projection.split_first() else {
            return Ok(narrowed);
        };
        match step {
            RefProjection::Index(index) => {
                narrowed.byte += *index as i64 * self.layout_size(&narrowed.view)?;
            }
            RefProjection::Deref => {}
            segment => return Err(view_segment(segment, &narrowed.view)),
        }
        for segment in rest {
            let (offset, ty) = match (segment, narrowed.view.kind()) {
                (RefProjection::Index(index), TyKind::PopArray { element, .. }) => {
                    (*index as i64 * self.layout_size(element)?, element.clone())
                }
                (RefProjection::Field(name), _) => self.field_slot(&narrowed.view, name)?,
                (segment, _) => return Err(view_segment(segment, &narrowed.view)),
            };
            narrowed.byte += offset;
            narrowed.view = ty;
        }
        Ok(narrowed)
    }

    /// The integer address of `view`: its storage's address, at the held
    /// element's stride, plus the byte it views.
    pub(super) fn view_address(&self, view: &ByteView) -> Result<i64, RuntimeError> {
        let stride = self.layout_size(&view.held)?;
        let base = match &view.storage {
            Value::Pointer { allocation, offset } => {
                crate::runtime::pointer_address(*allocation, *offset, stride)
            }
            Value::PlacePointer {
                frame,
                slot,
                projection,
            } => self.place_address(*frame, *slot, projection, stride)?,
            other => return Err(view_storage_error(other)),
        };
        Ok(base.wrapping_add(view.byte))
    }

    /// The value a typed read at `view.view` reads: the bytes of every held
    /// value it covers, decoded at the view's type. Padding reads as zero.
    pub(super) fn view_read(
        &self,
        view: &ByteView,
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<Value, RuntimeError> {
        let size = self.access_size(&view.view)?;
        let mut bytes = vec![0; size as usize];
        for (index, leaf) in self.covered_leaves(view, size)? {
            let value =
                self.read_leaf(&view.storage, index, &leaf.path, current, current_variables)?;
            let mut encoded = vec![0; leaf.size as usize];
            self.encode_bytes(&value, &leaf.ty, &mut encoded)?;
            for (position, byte) in encoded.into_iter().enumerate() {
                if let Some(slot) = view_byte(&mut bytes, leaf.offset + position as i64) {
                    *slot = byte;
                }
            }
        }
        self.decode_bytes(&bytes, &view.view)
    }

    /// Write `value` at `view.view` through `view`: each held value whose
    /// bytes it covers is rebuilt from its bytes with the written ones
    /// spliced in, and stored back at its own type.
    pub(super) fn view_write(
        &mut self,
        view: &ByteView,
        value: &Value,
        current: FrameId,
        current_variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let size = self.access_size(&view.view)?;
        let mut bytes = vec![0; size as usize];
        self.encode_bytes(value, &view.view, &mut bytes)?;
        for (index, leaf) in self.covered_leaves(view, size)? {
            let mut encoded = vec![0; leaf.size as usize];
            // A value the write covers in part keeps its other bytes; one
            // never written starts as zero bytes.
            if (leaf.offset < 0 || leaf.offset + leaf.size > size)
                && let Ok(held) =
                    self.read_leaf(&view.storage, index, &leaf.path, current, current_variables)
            {
                self.encode_bytes(&held, &leaf.ty, &mut encoded)?;
            }
            for (position, byte) in encoded.iter_mut().enumerate() {
                if let Some(written) = view_byte(&mut bytes, leaf.offset + position as i64) {
                    *byte = *written;
                }
            }
            let rebuilt = self.decode_bytes(&encoded, &leaf.ty)?;
            self.write_leaf(
                &view.storage,
                index,
                &leaf.path,
                rebuilt,
                current,
                current_variables,
            )?;
        }
        Ok(())
    }

    /// The `count` bytes at `pointer`, as a libc callee reads a buffer
    /// ([`Self::read_byte`]).
    pub(super) fn read_byte_run(
        &self,
        pointer: &Value,
        count: usize,
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<Vec<u8>, RuntimeError> {
        (0..count as i64)
            .map(|index| self.read_byte(pointer, index, current, current_variables))
            .collect()
    }

    /// The byte `index` bytes past `pointer`: a heap byte buffer's slot, or
    /// the byte any other address reads through a typed `Byte` access, a
    /// byte view's included.
    pub(super) fn read_byte(
        &self,
        pointer: &Value,
        index: i64,
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<u8, RuntimeError> {
        if let Value::Pointer { allocation, offset } = pointer {
            let (region, slot) = self.heap_index(*allocation, *offset, index)?;
            return held_byte(self.heap[region].slots.get(slot));
        }
        let element = byte_element(pointer, index)?;
        held_byte(Some(&self.read_reference(
            &element,
            current,
            current_variables,
        )?))
    }

    /// Write `bytes` at `pointer`, as a libc callee fills a buffer: each a
    /// `UInt8` lane, through any address [`Self::read_byte`] reads.
    pub(super) fn write_byte_run(
        &mut self,
        pointer: &Value,
        bytes: &[u8],
        current: FrameId,
        current_variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let byte = |byte: u8| Value::Simd {
            dtype: Dtype::UInt8,
            lanes: SimdLanes::Int(vec![i128::from(byte)]),
        };
        if let Value::Pointer { allocation, offset } = pointer {
            for (index, value) in bytes.iter().enumerate() {
                let (region, slot) = self.heap_index(*allocation, *offset, index as i64)?;
                self.heap_store(region, slot, byte(*value));
            }
            return Ok(());
        }
        for (index, value) in bytes.iter().enumerate() {
            let element = byte_element(pointer, index as i64)?;
            self.write_reference(&element, current, current_variables, byte(*value))?;
        }
        Ok(())
    }

    /// The bytes of `value`, held at `ty`, into `out` (`ty`'s size).
    pub(super) fn encode_bytes(
        &self,
        value: &Value,
        ty: &Ty,
        out: &mut [u8],
    ) -> Result<(), RuntimeError> {
        if out.is_empty() {
            return Ok(());
        }
        if crate::runtime::lane_shape(ty).is_some() {
            return encode_lanes(value, ty, out);
        }
        if let Some(pointee) = address_type_pointee(ty) {
            let address = self.value_address(value, pointee)?;
            for (slot, byte) in out.iter_mut().zip(address.to_le_bytes()) {
                *slot = byte;
            }
            return Ok(());
        }
        match (self.aggregate(ty)?, value) {
            (Aggregate::Held(field), value) => self.encode_bytes(value, &field, out),
            (Aggregate::Array(element, count), Value::Tuple(items)) if items.len() == count => {
                let stride = self.layout_size(&element)? as usize;
                for (index, item) in items.iter().enumerate() {
                    self.encode_bytes(item, &element, &mut out[index * stride..][..stride])?;
                }
                Ok(())
            }
            (Aggregate::Struct(fields), Value::Struct { fields: held, .. }) => {
                for (name, field, offset) in fields {
                    let size = self.layout_size(&field)? as usize;
                    let value = held
                        .iter()
                        .find(|(held, _)| *held == name)
                        .map(|(_, value)| value)
                        .ok_or_else(|| unsupported(&format!("`{ty}` holds no field `{name}`")))?;
                    self.encode_bytes(value, &field, &mut out[offset as usize..][..size])?;
                }
                Ok(())
            }
            (_, value) => Err(unsupported(&format!(
                "a {} held as `{ty}` has no bytes",
                crate::runtime::type_name(value)
            ))),
        }
    }

    /// The value of `ty` whose bytes are `bytes` (`ty`'s size).
    pub(super) fn decode_bytes(&self, bytes: &[u8], ty: &Ty) -> Result<Value, RuntimeError> {
        if let Some((dtype, width)) = crate::runtime::lane_shape(ty) {
            return decode_lanes(bytes, dtype, width);
        }
        if let Some(pointee) = address_type_pointee(ty) {
            let mut word = [0; 8];
            for (slot, byte) in word.iter_mut().zip(bytes) {
                *slot = *byte;
            }
            return Ok(self.address_value(i64::from_le_bytes(word), ty, pointee));
        }
        if matches!(ty.kind(), TyKind::None) {
            return Ok(Value::None);
        }
        match self.aggregate(ty)? {
            Aggregate::Held(field) => self.decode_bytes(bytes, &field),
            Aggregate::Array(element, count) => {
                let stride = self.layout_size(&element)? as usize;
                (0..count)
                    .map(|index| self.decode_bytes(&bytes[index * stride..][..stride], &element))
                    .collect::<Result<_, _>>()
                    .map(Value::Tuple)
            }
            Aggregate::Struct(fields) => {
                let name = ty
                    .kind()
                    .as_non_simd_struct()
                    .map(|(name, _)| name.clone())
                    .ok_or_else(|| no_bytes(ty))?;
                let fields = fields
                    .into_iter()
                    .map(|(field, field_ty, offset)| {
                        let size = self.layout_size(&field_ty)? as usize;
                        self.decode_bytes(&bytes[offset as usize..][..size], &field_ty)
                            .map(|value| (field, value))
                    })
                    .collect::<Result<_, _>>()?;
                Ok(Value::Struct { name, fields })
            }
        }
    }

    /// The size of `ty` under the native layout of the program running.
    fn layout_size(&self, ty: &Ty) -> Result<i64, RuntimeError> {
        self.layout_cx()?
            .layout_of(ty)
            .map(|layout| layout.size as i64)
            .map_err(|error| unsupported(&format!("`{ty}` has no layout: {error}")))
    }

    /// The bytes a typed read or write at `ty` accesses, LLVM's store size
    /// of the type the pin loads and stores: a `bool` vector's `<N x i1>`
    /// packs into its leading `ceil(N / 8)` bytes and leaves the rest of
    /// its layout untouched, and every other type accesses its whole size.
    fn access_size(&self, ty: &Ty) -> Result<i64, RuntimeError> {
        match crate::runtime::lane_shape(ty) {
            Some((Dtype::Bool, width)) => Ok(width.div_ceil(8) as i64),
            _ => self.layout_size(ty),
        }
    }

    /// The layout context of the program running.
    fn layout_cx(&self) -> Result<LayoutCx<'_>, RuntimeError> {
        let layout = self
            .layout
            .as_ref()
            .ok_or_else(|| unsupported("no program layout is configured"))?;
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| unsupported("no native target lays storage out"))?;
        Ok(LayoutCx {
            target,
            structs: &layout.fields,
        })
    }

    /// How a composite `ty` holds its bytes.
    fn aggregate(&self, ty: &Ty) -> Result<Aggregate, RuntimeError> {
        if let TyKind::PopArray { size, element } = ty.kind() {
            let count = size
                .known()
                .and_then(|count| usize::try_from(count).ok())
                .ok_or_else(|| no_bytes(ty))?;
            return Ok(Aggregate::Array(element.clone(), count));
        }
        let (name, _) = ty.kind().as_non_simd_struct().ok_or_else(|| no_bytes(ty))?;
        let layout = self
            .layout
            .as_ref()
            .ok_or_else(|| unsupported("no program layout is configured"))?;
        let declaration = layout.structs.get(name).ok_or_else(|| no_bytes(ty))?;
        if let Some(held) = &declaration.register_field
            && let Some((_, field)) = declaration.fields.iter().find(|(name, _)| name == held)
        {
            return Ok(Aggregate::Held(field.clone()));
        }
        let types: Vec<Ty> = declaration
            .fields
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect();
        let offsets = self
            .layout_cx()?
            .struct_layout(&types)
            .map_err(|error| unsupported(&format!("`{ty}` has no layout: {error}")))?
            .offsets;
        Ok(Aggregate::Struct(
            declaration
                .fields
                .iter()
                .zip(offsets)
                .map(|((name, ty), offset)| (name.clone(), ty.clone(), offset as i64))
                .collect(),
        ))
    }

    /// The byte offset and type of the field `name` of a `ty` value.
    fn field_slot(&self, ty: &Ty, name: &str) -> Result<(i64, Ty), RuntimeError> {
        if let Some(storage) = mojito_types::types::simd_storage(ty) {
            return Ok((0, storage));
        }
        match self.aggregate(ty)? {
            Aggregate::Held(field) => Ok((0, field)),
            Aggregate::Struct(fields) => fields
                .into_iter()
                .find(|(field, _, _)| field == name)
                .map(|(_, ty, offset)| (offset, ty))
                .ok_or_else(|| unsupported(&format!("`{ty}` has no field `{name}`"))),
            Aggregate::Array(..) => Err(unsupported(&format!("`{ty}` has no field `{name}`"))),
        }
    }

    /// Every held value a `size`-byte access through `view` covers: the
    /// element of the storage holding it, and where it sits.
    fn covered_leaves(&self, view: &ByteView, size: i64) -> Result<Vec<(i64, Leaf)>, RuntimeError> {
        let stride = self.layout_size(&view.held)?;
        if stride == 0 {
            return Err(unsupported(&format!(
                "a byte view of zero-sized `{}` storage",
                view.held
            )));
        }
        let mut covered = Vec::new();
        if size == 0 {
            return Ok(covered);
        }
        let first = view.byte.div_euclid(stride);
        let last = (view.byte + size - 1).div_euclid(stride);
        for index in first..=last {
            let mut leaves = Vec::new();
            self.leaves(
                &view.held,
                index * stride - view.byte,
                size,
                &mut Vec::new(),
                &mut leaves,
            )?;
            covered.extend(leaves.into_iter().map(|leaf| (index, leaf)));
        }
        Ok(covered)
    }

    /// The held values of a `ty` value at byte `base` of a `size`-byte
    /// access that overlap it, reached from the value by `path`. A part the
    /// access does not overlap is never asked for its bytes.
    fn leaves(
        &self,
        ty: &Ty,
        base: i64,
        size: i64,
        path: &mut Vec<RefProjection>,
        leaves: &mut Vec<Leaf>,
    ) -> Result<(), RuntimeError> {
        let ty_size = self.layout_size(ty)?;
        if ty_size == 0 || base + ty_size <= 0 || base >= size {
            return Ok(());
        }
        if crate::runtime::lane_shape(ty).is_some() || address_type_pointee(ty).is_some() {
            leaves.push(Leaf {
                path: path.clone(),
                ty: ty.clone(),
                offset: base,
                size: ty_size,
            });
            return Ok(());
        }
        match self.aggregate(ty)? {
            Aggregate::Held(field) => self.leaves(&field, base, size, path, leaves),
            Aggregate::Array(element, count) => {
                let stride = self.layout_size(&element)?;
                for index in 0..count {
                    path.push(RefProjection::Index(index));
                    self.leaves(&element, base + index as i64 * stride, size, path, leaves)?;
                    path.pop();
                }
                Ok(())
            }
            Aggregate::Struct(fields) => {
                for (name, field, offset) in fields {
                    path.push(RefProjection::Field(name));
                    self.leaves(&field, base + offset, size, path, leaves)?;
                    path.pop();
                }
                Ok(())
            }
        }
    }

    /// The held value `path` reaches in element `index` of `storage`.
    fn read_leaf(
        &self,
        storage: &Value,
        index: i64,
        path: &[RefProjection],
        current: FrameId,
        current_variables: &[Value],
    ) -> Result<Value, RuntimeError> {
        match storage {
            Value::Pointer { allocation, offset } => {
                let element = self.heap_read(*allocation, *offset, index)?;
                self.read_reference_projection(current, current_variables, &element, path)
            }
            place => self.read_reference(
                &place_element(place, index, path)?,
                current,
                current_variables,
            ),
        }
    }

    /// Store `value` as the held value `path` reaches in element `index`
    /// of `storage`.
    fn write_leaf(
        &mut self,
        storage: &Value,
        index: i64,
        path: &[RefProjection],
        value: Value,
        current: FrameId,
        current_variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        match storage {
            Value::Pointer { allocation, offset } => {
                let (region, slot) = self.heap_index(*allocation, *offset, index)?;
                self.write_heap_projection(region, slot, path, value)
            }
            place => self.write_reference(
                &place_element(place, index, path)?,
                current,
                current_variables,
                value,
            ),
        }
    }

    /// The integer address of the pointer `value` to `pointee` elements:
    /// a heap pointer's, a place's, or a raw address held as an integer.
    fn value_address(&self, value: &Value, pointee: &Ty) -> Result<i64, RuntimeError> {
        match value {
            Value::Pointer { allocation, offset } => Ok(crate::runtime::pointer_address(
                *allocation,
                *offset,
                self.layout_size(pointee)?,
            )),
            Value::PlacePointer {
                frame,
                slot,
                projection,
            }
            | Value::Ref {
                frame,
                slot,
                projection,
            } => self.place_address(*frame, *slot, projection, self.layout_size(pointee)?),
            Value::Int(address) => Ok(*address),
            Value::UInt(address) => Ok(*address as i64),
            other => Err(unsupported(&format!(
                "a {} held as a pointer has no address",
                crate::runtime::type_name(other)
            ))),
        }
    }

    /// The pointer of type `ty` whose integer address is `address`, or the
    /// integer itself when it names no storage the VM holds: the raw
    /// address a typed read of `ty` decodes later
    /// ([`VmBackend::read_typed`]).
    fn address_value(&self, address: i64, ty: &Ty, pointee: &Ty) -> Value {
        let Some(layout) = &self.layout else {
            return Value::Int(address);
        };
        self.layout_size(pointee)
            .and_then(|stride| self.address_pointer_at(address, stride, pointee))
            .map_or(Value::Int(address), |pointer| {
                address_as(&layout.structs, pointer, Some(ty))
            })
    }

    /// The storage a view of the address `source` reads: a heap pointer,
    /// an element of an interned heap pointer included, or a place pointer.
    fn view_storage(&self, source: Value) -> Result<Value, RuntimeError> {
        match source {
            Value::Pointer { .. } => Ok(source),
            Value::PlacePointer {
                frame,
                slot,
                projection,
            }
            | Value::Ref {
                frame,
                slot,
                projection,
            } => Ok(
                match (self.frame_variable(frame, slot), projection.as_slice()) {
                    (
                        Some(Value::Pointer { allocation, offset }),
                        [RefProjection::Index(index)],
                    ) if frame == POINTER_ROOT_FRAME => Value::Pointer {
                        allocation: *allocation,
                        offset: offset + *index as i64,
                    },
                    _ => Value::PlacePointer {
                        frame,
                        slot,
                        projection,
                    },
                },
            ),
            other => Err(view_storage_error(&other)),
        }
    }

    /// The slot of [`POINTER_ROOT_FRAME`] holding `view`, interned on
    /// first use.
    fn view_root(&mut self, view: ByteView) -> Result<usize, RuntimeError> {
        let storage = match &view.storage {
            Value::Pointer { allocation, offset } => ViewStorage::Heap(*allocation, *offset),
            Value::PlacePointer {
                frame,
                slot,
                projection,
            } => ViewStorage::Place(*frame, *slot, projection.clone()),
            other => return Err(view_storage_error(other)),
        };
        let key = ViewKey {
            storage,
            held: view.held.clone(),
            view: view.view.clone(),
            byte: view.byte,
        };
        Ok(*self.view_root_slots.entry(key).or_insert_with(|| {
            self.pointer_roots.push(Value::ByteView(Box::new(view)));
            self.pointer_roots.len() - 1
        }))
    }
}

/// The storage a [`ViewKey`] names.
#[derive(Debug, PartialEq, Eq, Hash)]
enum ViewStorage {
    Heap(u64, i64),
    Place(u64, usize, Vec<RefProjection>),
}

/// A held value a byte access covers: the projection reaching it within
/// its storage element, its type, and its byte offset and size relative to
/// the access.
struct Leaf {
    path: Vec<RefProjection>,
    ty: Ty,
    offset: i64,
    size: i64,
}

/// How a composite type holds its bytes ([`VmBackend::aggregate`]).
enum Aggregate {
    /// A struct held as its one field ([`StructDef::register_field`]).
    Held(Ty),
    /// A `!pop.array` of `count` elements, held as a [`Value::Tuple`].
    Array(Ty, usize),
    /// A struct's fields, with their byte offsets.
    Struct(Vec<(String, Ty, i64)>),
}

fn unsupported(message: &str) -> RuntimeError {
    RuntimeError::Unsupported(format!("vm: {message}"))
}

/// A byte view of `ty`, which the VM holds no bytes of: a type with no
/// byte form (a `Variant`, a callable, a lane dtype the VM holds no lanes
/// of).
fn no_bytes(ty: &Ty) -> RuntimeError {
    unsupported(&format!(
        "a byte view of `{ty}`, which the VM holds no bytes of"
    ))
}

/// The pointee of a type held as an address: a pointer, its
/// `!kgen.pointer`, or a reference.
fn address_type_pointee(ty: &Ty) -> Option<&Ty> {
    ty.kind().address_parts().map(|(pointee, _)| pointee)
}

/// The byte of a view's `bytes` at `position`, when the view covers it.
fn view_byte(bytes: &mut [u8], position: i64) -> Option<&mut u8> {
    usize::try_from(position)
        .ok()
        .and_then(|position| bytes.get_mut(position))
}

/// The frame, slot, and projection of a place pointer or reference.
fn handle_parts(handle: &Value) -> Option<(u64, usize, &[RefProjection])> {
    match handle {
        Value::PlacePointer {
            frame,
            slot,
            projection,
        }
        | Value::Ref {
            frame,
            slot,
            projection,
        } => Some((*frame, *slot, projection)),
        _ => None,
    }
}

/// The reference to the value `path` reaches in element `index` of the
/// place pointer `place`.
fn place_element(place: &Value, index: i64, path: &[RefProjection]) -> Result<Value, RuntimeError> {
    let Value::PlacePointer {
        frame,
        slot,
        mut projection,
    } = super::references::offset_place_handle(place.clone(), index)?
    else {
        return Err(view_storage_error(place));
    };
    projection.extend_from_slice(path);
    Ok(Value::Ref {
        frame,
        slot,
        projection,
    })
}

/// The handle of byte `index` past the place pointer or reference
/// `pointer`.
fn byte_element(pointer: &Value, index: i64) -> Result<Value, RuntimeError> {
    let (frame, slot, projection) = handle_parts(pointer).ok_or_else(|| {
        RuntimeError::TypeError(format!(
            "vm: a byte buffer argument is a {}, not a pointer",
            crate::runtime::type_name(pointer)
        ))
    })?;
    super::references::offset_place_handle(
        Value::PlacePointer {
            frame,
            slot,
            projection: projection.to_vec(),
        },
        index,
    )
}

/// The byte a buffer slot holds: a width-1 integer lane, `Byte`'s value.
fn held_byte(slot: Option<&Value>) -> Result<u8, RuntimeError> {
    match slot {
        Some(Value::Simd {
            lanes: SimdLanes::Int(lanes),
            ..
        }) if lanes.len() == 1 => Ok(lanes[0] as u8),
        other => Err(RuntimeError::TypeError(format!(
            "vm: byte buffer slot is {other:?}, not a byte"
        ))),
    }
}

fn view_segment(segment: &RefProjection, view: &Ty) -> RuntimeError {
    unsupported(&format!(
        "a {segment:?} step through a byte view of `{view}`"
    ))
}

fn view_storage_error(storage: &Value) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "vm: a byte view addresses a {}, not storage",
        crate::runtime::type_name(storage)
    ))
}

/// The bytes of the register scalar or vector `value`, held at `ty`: its
/// lanes little-endian at their own dtype's width, upstream's `pop.bitcast`
/// to bytes, and a `bool` vector's lanes one bit each, lane 0 lowest, as
/// the pin packs them in memory.
fn encode_lanes(value: &Value, ty: &Ty, out: &mut [u8]) -> Result<(), RuntimeError> {
    let (dtype, lanes) = crate::runtime::simd_parts(value).ok_or_else(|| {
        unsupported(&format!(
            "a {} held as `{ty}` has no bytes",
            crate::runtime::type_name(value)
        ))
    })?;
    if let SimdLanes::Bool(lanes) = &lanes {
        for (index, lane) in lanes.iter().enumerate() {
            if let Some(byte) = out.get_mut(index / 8)
                && *lane
            {
                *byte |= 1 << (index % 8);
            }
        }
        return Ok(());
    }
    let count = lanes.width() * dtype.bit_width() as usize / 8;
    let Value::Simd {
        lanes: SimdLanes::Int(bytes),
        ..
    } = crate::runtime::simd_bitcast(Dtype::UInt8, count, value)?
    else {
        return Err(unsupported(&format!("the bytes of `{ty}`")));
    };
    for (slot, byte) in out.iter_mut().zip(bytes) {
        *slot = byte as u8;
    }
    Ok(())
}

/// The register scalar or vector of `width` `dtype` lanes whose bytes are
/// `bytes` ([`encode_lanes`]'s inverse). A `bool` lane reads its one bit,
/// so a byte of `2` reads `False`, as at the pin.
fn decode_lanes(bytes: &[u8], dtype: Dtype, width: usize) -> Result<Value, RuntimeError> {
    if dtype == Dtype::Bool {
        let lanes: Vec<bool> = (0..width)
            .map(|index| {
                bytes
                    .get(index / 8)
                    .is_some_and(|byte| byte >> (index % 8) & 1 == 1)
            })
            .collect();
        return Ok(match lanes.as_slice() {
            [lane] => Value::Bool(*lane),
            _ => Value::Simd {
                dtype,
                lanes: SimdLanes::Bool(lanes),
            },
        });
    }
    let count = width * dtype.bit_width() as usize / 8;
    let source = Value::Simd {
        dtype: Dtype::UInt8,
        lanes: SimdLanes::Int(
            bytes
                .iter()
                .take(count)
                .map(|byte| i128::from(*byte))
                .collect(),
        ),
    };
    crate::runtime::simd_bitcast(dtype, width, &source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_types::origin::{Mutability, Origin};
    use mojito_types::types::{AddressSpaceSlot, SimdDtype, SimdWidth};

    fn vector(dtype: Dtype, width: i64) -> Ty {
        Ty::simd(SimdDtype::Known(dtype), SimdWidth::Known(width))
    }

    /// A VM laying out the structs `declared`, each by its fields.
    fn vm(declared: &[(&str, Vec<(&str, Ty)>)]) -> VmBackend {
        let mut fields = StructFieldIndex::default();
        let mut structs = HashMap::new();
        for (name, declaration) in declared {
            fields.insert(
                (*name).to_string(),
                declaration.iter().map(|(_, ty)| ty.clone()).collect(),
            );
            structs.insert(
                (*name).to_string(),
                StructDef {
                    fields: declaration
                        .iter()
                        .map(|(field, ty)| ((*field).to_string(), ty.clone()))
                        .collect(),
                    members: Vec::new(),
                    register_field: None,
                },
            );
        }
        let mut vm = VmBackend::new();
        vm.layout = Some(ProgramLayout {
            structs: Rc::new(structs),
            fields: Rc::new(fields),
        });
        vm
    }

    fn encode(vm: &VmBackend, value: &Value, ty: &Ty) -> Vec<u8> {
        let mut bytes = vec![0; vm.layout_size(ty).unwrap() as usize];
        vm.encode_bytes(value, ty, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_register_scalar_or_vector_holds_its_bits_little_endian() {
        let vm = vm(&[]);
        let int = Ty::int();
        assert_eq!(
            encode(&vm, &Value::Int(0x0102), &int),
            [2, 1, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            vm.decode_bytes(&[2, 1, 0, 0, 0, 0, 0, 0], &int).unwrap(),
            Value::Int(0x0102)
        );
        let bytes = Value::Simd {
            dtype: Dtype::UInt8,
            lanes: SimdLanes::Int(vec![1, 2, 255, 4]),
        };
        assert_eq!(
            encode(&vm, &bytes, &vector(Dtype::UInt8, 4)),
            [1, 2, 255, 4]
        );
        assert_eq!(
            encode(&vm, &Value::Float64(1.0), &Ty::float64()),
            1.0_f64.to_le_bytes()
        );
    }

    #[test]
    fn a_bool_lane_is_one_bit_of_memory() {
        let vm = vm(&[]);
        let bool = vector(Dtype::Bool, 1);
        assert_eq!(encode(&vm, &Value::Bool(true), &bool), [1]);
        let lanes = Value::Simd {
            dtype: Dtype::Bool,
            lanes: SimdLanes::Bool(vec![true, false, true, true]),
        };
        assert_eq!(encode(&vm, &lanes, &vector(Dtype::Bool, 4)), [13, 0, 0, 0]);
        assert_eq!(vm.decode_bytes(&[2], &bool).unwrap(), Value::Bool(false));
    }

    #[test]
    fn a_struct_holds_its_fields_at_their_offsets() {
        let byte = vector(Dtype::UInt8, 1);
        let pointer = Ty::pointer(
            byte,
            Origin::Static,
            Mutability::Mutable,
            AddressSpaceSlot::GENERIC,
        );
        let vm = vm(&[
            (
                "Text",
                vec![("ptr", pointer), ("len", Ty::int()), ("cap", Ty::int())],
            ),
            (
                "Padded",
                vec![("a", vector(Dtype::Int8, 1)), ("b", Ty::int())],
            ),
        ]);
        let text = Ty::new(TyKind::Struct("Text".to_string(), Vec::new()));
        let bytes = *b"efgh\xff\xff\xff\x7fhello, world!!!!";
        let decoded = vm.decode_bytes(&bytes, &text).unwrap();
        assert_eq!(encode(&vm, &decoded, &text), bytes);
        let padded = Ty::new(TyKind::Struct("Padded".to_string(), Vec::new()));
        let value = Value::Struct {
            name: "Padded".to_string(),
            fields: vec![
                (
                    "a".to_string(),
                    Value::Simd {
                        dtype: Dtype::Int8,
                        lanes: SimdLanes::Int(vec![-1]),
                    },
                ),
                ("b".to_string(), Value::Int(2)),
            ],
        };
        assert_eq!(
            encode(&vm, &value, &padded),
            [255, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_type_with_no_byte_form_is_refused() {
        let vm = vm(&[]);
        let variant = Ty::new(TyKind::Variant(vec![Ty::int(), Ty::float64()]));
        assert!(matches!(
            vm.decode_bytes(&[0; 16], &variant),
            Err(RuntimeError::Unsupported(_))
        ));
    }
}
