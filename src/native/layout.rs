//! The one native layout owner: sizes, alignments, offsets, and padding for
//! every runtime-representable checked type.
//!
//! Rules (normative in `docs/native-abi.md`): fields lay out in declaration
//! order with C-style padding and no reordering; a zero-sized type has size 0
//! and alignment 1; `Variant` is a `u32` tag at offset 0 with the payload
//! overlay at the first payload-aligned offset; pointers and references are
//! one target pointer with origins erased. Backends consume these results —
//! they never derive layout from Rust's unspecified `repr(Rust)` or from LLVM
//! defaults.
//!
//! Width-1 SIMD scalars store at their lane width ([`lane_layout`]); literal
//! value types store at their materialized width (`IntLiteral` as i64,
//! `FloatLiteral` as f64, `StringLiteral` as the borrowed descriptor). Types
//! with no (single) native representation — multi-lane SIMD until its lowering
//! stage, packs, callables — reject with [`LayoutError::Unsupported`] so
//! backends surface a contextual diagnostic instead of guessing.

use std::collections::HashMap;

use crate::ast::Dtype;
use crate::mir::MirDeclarations;
use crate::native::target::NativeTarget;
use crate::types::Ty;

/// Size and alignment in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

impl Layout {
    pub const fn new(size: u64, align: u64) -> Layout {
        Layout { size, align }
    }

    /// The layout of a zero-sized type.
    pub const ZERO: Layout = Layout::new(0, 1);
}

/// An aggregate layout with the byte offset of every field, in declaration
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    pub layout: Layout,
    pub offsets: Vec<u64>,
}

/// A tagged-union layout: `u32` tag at offset 0, every alternative's payload
/// overlaid at `payload_offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariantLayout {
    pub layout: Layout,
    pub payload_offset: u64,
}

/// Field types of every nominal struct in a `MirProgram`, keyed by the
/// specialized struct name (MIR-level struct identity is the name; generic
/// structs are already monomorphized below the waist).
#[derive(Debug, Clone, Default)]
pub struct StructFieldIndex {
    fields: HashMap<String, Vec<Ty>>,
}

impl StructFieldIndex {
    pub fn from_declarations(declarations: &MirDeclarations) -> StructFieldIndex {
        let mut index = StructFieldIndex::default();
        for decl in &declarations.structs {
            index.insert(
                decl.name.clone(),
                decl.fields.iter().map(|(_, ty)| ty.clone()).collect(),
            );
        }
        index
    }

    pub fn insert(&mut self, name: String, field_types: Vec<Ty>) {
        self.fields.insert(name, field_types);
    }

    pub fn get(&self, name: &str) -> Option<&[Ty]> {
        self.fields.get(name).map(Vec::as_slice)
    }
}

/// Layout computation context: the target (for pointer size) plus the struct
/// field index of the program being lowered.
#[derive(Debug, Clone, Copy)]
pub struct LayoutCx<'a> {
    pub target: &'a NativeTarget,
    pub structs: &'a StructFieldIndex,
}

impl LayoutCx<'_> {
    /// The layout of one checked type.
    pub fn layout_of(&self, ty: &Ty) -> Result<Layout, LayoutError> {
        // Compiler-private inline uninit storage is payload-only: no tag, no
        // init flag (verified programs never take or destroy an uninitialized
        // slot — misuse fixtures live off the exe gate). Deliberately no
        // synthesized MIR declaration either: a nominal struct without
        // declared fields drops as a no-op, which is exactly the VM's
        // leak-by-design drop of `Value::UninitStorage`.
        if let Some(element) = crate::types::uninit_storage_element(ty) {
            return self.layout_of(element);
        }
        match ty {
            Ty::Int | Ty::UInt | Ty::Float64 => Ok(Layout::new(8, 8)),
            Ty::Bool => Ok(Layout::new(1, 1)),
            Ty::None => Ok(Layout::ZERO),
            // The borrowed string-literal descriptor: `{ data: ptr, len: u64 }`
            // (`MjStrDesc` in the runtime contract).
            Ty::StringLiteral => Ok(compose(&[self.pointer(), Layout::new(8, 8)]).layout),
            // The built-in error value: `{ message: MjString }` where
            // `MjString` is `{ data: ptr, size: i64, cap: i64 }`.
            Ty::Error => Ok(self.mj_error()),
            Ty::Struct(name, _) => {
                let Some(fields) = self.structs.get(name) else {
                    return Err(LayoutError::Unsupported(format!(
                        "struct '{name}' has no MIR declaration to lay out"
                    )));
                };
                Ok(self.struct_layout(fields)?.layout)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                Ok(self.struct_layout(elements)?.layout)
            }
            Ty::Variant(alternatives) => Ok(self.variant_layout(alternatives)?.layout),
            Ty::Pointer { .. } | Ty::Ref(_) => Ok(self.pointer()),
            Ty::Simd { dtype, width: 1 } => Ok(lane_layout(*dtype)),
            // Literal-typed storage holds the value at its default
            // materialized width; a value that exceeds it rejects at lowering.
            Ty::IntLiteral | Ty::FloatLiteral => Ok(Layout::new(8, 8)),
            unsupported => Err(LayoutError::Unsupported(format!(
                "type has no native layout yet: {unsupported}"
            ))),
        }
    }

    /// C-style aggregate layout of `fields` in declaration order.
    pub fn struct_layout(&self, fields: &[Ty]) -> Result<StructLayout, LayoutError> {
        let laid = fields
            .iter()
            .map(|field| self.layout_of(field))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(compose(&laid))
    }

    /// Tagged success/error outcome of a raising function:
    /// `{ tag: u32, ok: T, err: MjError }` composed by the ordinary aggregate
    /// rules (normative in `docs/native-abi.md`). The returned offsets are
    /// `[tag, ok, err]`; a `None` return contributes a zero-sized ok payload.
    pub fn outcome_layout(&self, ok: &Ty) -> Result<StructLayout, LayoutError> {
        Ok(compose(&[
            Layout::new(4, 4),
            self.layout_of(ok)?,
            self.mj_error(),
        ]))
    }

    /// Tagged-union layout: `u32` tag at offset 0, payload overlay sized and
    /// aligned to the widest alternative.
    pub fn variant_layout(&self, alternatives: &[Ty]) -> Result<VariantLayout, LayoutError> {
        let mut payload = Layout::ZERO;
        for alternative in alternatives {
            let layout = self.layout_of(alternative)?;
            payload.size = payload.size.max(layout.size);
            payload.align = payload.align.max(layout.align);
        }
        let tag = Layout::new(4, 4);
        let align = tag.align.max(payload.align);
        let payload_offset = align_up(tag.size, payload.align);
        Ok(VariantLayout {
            layout: Layout::new(align_up(payload_offset + payload.size, align), align),
            payload_offset,
        })
    }

    /// The runtime's `MjString { data: ptr, size: i64, cap: i64 }`.
    pub fn mj_string(&self) -> Layout {
        compose(&[self.pointer(), Layout::new(8, 8), Layout::new(8, 8)]).layout
    }

    /// The runtime's `MjError { message: MjString }`.
    fn mj_error(&self) -> Layout {
        compose(&[self.mj_string()]).layout
    }

    fn pointer(&self) -> Layout {
        Layout::new(
            self.target.triple.pointer_size(),
            self.target.triple.pointer_align(),
        )
    }
}

/// A layout the native ABI does not define (yet). Backends must reject the
/// construct with a contextual diagnostic; they must not guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    Unsupported(String),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Unsupported(message) => write!(f, "{message}"),
        }
    }
}

/// The storage layout of one SIMD lane — equally, of the width-1 scalar alias
/// (`Int8`, `UInt32`, `Float32`, …): the lane's natural width and alignment.
pub fn lane_layout(dtype: Dtype) -> Layout {
    match dtype {
        Dtype::Int8 | Dtype::UInt8 | Dtype::Bool => Layout::new(1, 1),
        Dtype::Int16 | Dtype::UInt16 => Layout::new(2, 2),
        Dtype::Int32 | Dtype::UInt32 | Dtype::Float32 => Layout::new(4, 4),
        Dtype::Int | Dtype::Int64 | Dtype::UInt64 | Dtype::Float64 => Layout::new(8, 8),
    }
}

/// Compose already-computed field layouts into a C-style aggregate: each field
/// at the next offset aligned for it, total size padded to the aggregate
/// alignment (the max field alignment, at least 1).
pub fn compose(fields: &[Layout]) -> StructLayout {
    let mut offset = 0u64;
    let mut align = 1u64;
    let mut offsets = Vec::with_capacity(fields.len());
    for field in fields {
        offset = align_up(offset, field.align);
        offsets.push(offset);
        offset += field.size;
        align = align.max(field.align);
    }
    StructLayout {
        layout: Layout::new(align_up(offset, align), align),
        offsets,
    }
}

fn align_up(offset: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    offset.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::target::{NativeTarget, Triple};
    use crate::origin::PointerOrigin;

    fn cx(structs: &StructFieldIndex) -> (NativeTarget, &StructFieldIndex) {
        (NativeTarget::new(Triple::X86_64UnknownLinuxGnu), structs)
    }

    fn layout_of(ty: &Ty) -> Result<Layout, LayoutError> {
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        LayoutCx {
            target: &target,
            structs,
        }
        .layout_of(ty)
    }

    #[test]
    fn scalar_layouts() {
        assert_eq!(layout_of(&Ty::Int), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&Ty::UInt), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&Ty::Float64), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&Ty::Bool), Ok(Layout::new(1, 1)));
        assert_eq!(layout_of(&Ty::None), Ok(Layout::ZERO));
    }

    #[test]
    fn width_one_simd_scalars_store_at_lane_width() {
        let scalar = |dtype| Ty::Simd { dtype, width: 1 };
        assert_eq!(layout_of(&scalar(Dtype::Int8)), Ok(Layout::new(1, 1)));
        assert_eq!(layout_of(&scalar(Dtype::UInt8)), Ok(Layout::new(1, 1)));
        assert_eq!(layout_of(&scalar(Dtype::Int16)), Ok(Layout::new(2, 2)));
        assert_eq!(layout_of(&scalar(Dtype::UInt16)), Ok(Layout::new(2, 2)));
        assert_eq!(layout_of(&scalar(Dtype::Int32)), Ok(Layout::new(4, 4)));
        assert_eq!(layout_of(&scalar(Dtype::UInt32)), Ok(Layout::new(4, 4)));
        assert_eq!(layout_of(&scalar(Dtype::Float32)), Ok(Layout::new(4, 4)));
        assert_eq!(layout_of(&scalar(Dtype::Int64)), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&scalar(Dtype::UInt64)), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&scalar(Dtype::Int)), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&scalar(Dtype::Float64)), Ok(Layout::new(8, 8)));
        // Narrow fields pad C-style like any other member.
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        let mixed = cx
            .struct_layout(&[scalar(Dtype::UInt8), scalar(Dtype::Int32), Ty::Int])
            .unwrap();
        assert_eq!(mixed.offsets, vec![0, 4, 8]);
        assert_eq!(mixed.layout, Layout::new(16, 8));
    }

    #[test]
    fn literal_storage_uses_materialized_widths() {
        assert_eq!(layout_of(&Ty::IntLiteral), Ok(Layout::new(8, 8)));
        assert_eq!(layout_of(&Ty::FloatLiteral), Ok(Layout::new(8, 8)));
    }

    #[test]
    fn uninit_storage_is_payload_only() {
        let storage = |name: &str, element: Ty| {
            Ty::Struct(name.to_string(), vec![crate::types::TyArg::Ty(element)])
        };
        assert_eq!(
            layout_of(&storage("__UninitStorage", Ty::Int)),
            Ok(Layout::new(8, 8))
        );
        assert_eq!(
            layout_of(&storage("__UninitStorage", Ty::Bool)),
            Ok(Layout::new(1, 1))
        );
        // Backend-monomorphized instance names keep their argument list, so
        // the payload stays recoverable through the mangled spelling.
        assert_eq!(
            layout_of(&storage("__UninitStorage$mono$TInt", Ty::Int)),
            Ok(Layout::new(8, 8))
        );
        // Payload-only storage composes into an enclosing aggregate like a
        // bare field of the element type.
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        let nested = cx
            .struct_layout(&[Ty::Bool, storage("__UninitStorage", Ty::Int)])
            .unwrap();
        assert_eq!(nested.offsets, vec![0, 8]);
        assert_eq!(nested.layout, Layout::new(16, 8));
    }

    #[test]
    fn descriptor_error_and_pointer_layouts() {
        assert_eq!(layout_of(&Ty::StringLiteral), Ok(Layout::new(16, 8)));
        assert_eq!(layout_of(&Ty::Error), Ok(Layout::new(24, 8)));
        let pointer = Ty::Pointer {
            element: Box::new(Ty::Int),
            origin: PointerOrigin::Static,
        };
        assert_eq!(layout_of(&pointer), Ok(Layout::new(8, 8)));
    }

    #[test]
    fn tuples_pad_c_style_in_declaration_order() {
        // { i1, i64, i1 }: offsets 0, 8, 16; size pads to 24.
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        let layout = cx.struct_layout(&[Ty::Bool, Ty::Int, Ty::Bool]).unwrap();
        assert_eq!(layout.offsets, vec![0, 8, 16]);
        assert_eq!(layout.layout, Layout::new(24, 8));
        // Declaration order is preserved even when reordering would pack
        // tighter: { i1, i1, i64 } is 16 bytes, not 24.
        let packed = cx.struct_layout(&[Ty::Bool, Ty::Bool, Ty::Int]).unwrap();
        assert_eq!(packed.offsets, vec![0, 1, 8]);
        assert_eq!(packed.layout, Layout::new(16, 8));
        assert_eq!(
            layout_of(&Ty::Tuple(vec![Ty::Bool, Ty::Int, Ty::Bool])),
            Ok(Layout::new(24, 8))
        );
    }

    #[test]
    fn empty_and_zero_sized_aggregates() {
        assert_eq!(layout_of(&Ty::Tuple(vec![])), Ok(Layout::ZERO));
        assert_eq!(
            layout_of(&Ty::Tuple(vec![Ty::None, Ty::None])),
            Ok(Layout::ZERO)
        );
        // A ZST field takes no space and forces no padding.
        assert_eq!(
            layout_of(&Ty::Tuple(vec![Ty::None, Ty::Bool])),
            Ok(Layout::new(1, 1))
        );
    }

    #[test]
    fn nominal_structs_resolve_through_the_field_index() {
        let mut structs = StructFieldIndex::default();
        structs.insert("Pair".to_string(), vec![Ty::Int, Ty::Bool]);
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        assert_eq!(
            cx.layout_of(&Ty::Struct("Pair".to_string(), vec![])),
            Ok(Layout::new(16, 8))
        );
        let missing = cx.layout_of(&Ty::Struct("Absent".to_string(), vec![]));
        assert!(matches!(missing, Err(LayoutError::Unsupported(_))));
    }

    #[test]
    fn outcome_is_tag_ok_err_by_ordinary_aggregate_rules() {
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        // { tag: u32, ok: Int, err: MjError }: tag 0, ok 8, err 16; 40/8.
        let int = cx.outcome_layout(&Ty::Int).unwrap();
        assert_eq!(int.offsets, vec![0, 8, 16]);
        assert_eq!(int.layout, Layout::new(40, 8));
        // A None return is a zero-sized ok payload: err right after the tag's
        // padding — { tag, err } at offsets 0 and 8; 32/8.
        let none = cx.outcome_layout(&Ty::None).unwrap();
        assert_eq!(none.offsets, vec![0, 4, 8]);
        assert_eq!(none.layout, Layout::new(32, 8));
        // Bool packs into the tag's padding.
        let boolean = cx.outcome_layout(&Ty::Bool).unwrap();
        assert_eq!(boolean.offsets, vec![0, 4, 8]);
        assert_eq!(boolean.layout, Layout::new(32, 8));
        // The ok payload of an unrepresentable type rejects like any layout.
        assert!(matches!(
            cx.outcome_layout(&Ty::Never),
            Err(LayoutError::Unsupported(_))
        ));
    }

    #[test]
    fn variant_is_tag_plus_aligned_payload_overlay() {
        let structs = StructFieldIndex::default();
        let (target, structs) = cx(&structs);
        let cx = LayoutCx {
            target: &target,
            structs,
        };
        // Variant[Int, Bool]: tag at 0, payload at 8, size 16, align 8.
        let variant = cx.variant_layout(&[Ty::Int, Ty::Bool]).unwrap();
        assert_eq!(variant.payload_offset, 8);
        assert_eq!(variant.layout, Layout::new(16, 8));
        // Variant[Bool]: payload aligns 1, so it sits right after the tag.
        let small = cx.variant_layout(&[Ty::Bool]).unwrap();
        assert_eq!(small.payload_offset, 4);
        assert_eq!(small.layout, Layout::new(8, 4));
    }

    #[test]
    fn unrepresentable_types_reject_loudly() {
        for ty in [
            Ty::Never,
            Ty::Infer,
            Ty::Dtype,
            Ty::SelfType,
            Ty::VariadicPack(Box::new(Ty::Int)),
            Ty::ComptimeList(Box::new(Ty::Int)),
            // Multi-lane SIMD stays unrepresentable until its lowering stage.
            Ty::Simd {
                dtype: Dtype::Float64,
                width: 4,
            },
        ] {
            assert!(
                matches!(layout_of(&ty), Err(LayoutError::Unsupported(_))),
                "{ty} should have no layout"
            );
        }
    }
}
