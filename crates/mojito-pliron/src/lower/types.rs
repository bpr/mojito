//! Scalar/lowered type classification: [`Locator`] span resolution,
//! [`ScalarTy`]/[`LowerTy`] helpers, and `lower_ty`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{AddressSpaceSlot, SimdDtype, SimdWidth, TyKind};

impl Locator {
    pub(crate) fn new(ctx: &Context, sources: &[(String, String)]) -> Self {
        let sources = sources
            .iter()
            .map(|(name, text)| {
                let source = pliron::location::Source::new_from_file(ctx, name.clone());
                let mut line_starts = vec![0];
                for (offset, byte) in text.bytes().enumerate() {
                    if byte == b'\n' {
                        line_starts.push(offset + 1);
                    }
                }
                (name.clone(), source, line_starts)
            })
            .collect();
        Self { sources }
    }

    /// The registered source labels, in registration order — the debug
    /// table's file-id space.
    pub(crate) fn source_labels(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|(name, _, _)| name.as_str())
    }

    /// The registered pliron [`pliron::location::Source`] handles, in the
    /// same order as [`Locator::source_labels`].
    pub(crate) fn sources(&self) -> impl Iterator<Item = pliron::location::Source> + '_ {
        self.sources.iter().map(|(_, source, _)| *source)
    }

    pub(super) fn locate(&self, span: &SourceSpan) -> Option<Location> {
        let name = span.source.as_deref()?;
        let (_, source, line_starts) = self.sources.iter().find(|(n, _, _)| n == name)?;
        let byte = span.span.0;
        let line = line_starts.partition_point(|start| *start <= byte);
        let column = byte - line_starts[line - 1] + 1;
        Some(Location::SrcPos {
            src: *source,
            pos: pliron::combine::stream::position::SourcePosition {
                line: line as i32,
                column: column as i32,
            },
        })
    }
}

impl ScalarTy {
    /// A pointer in the generic address space: every runtime-owned address
    /// (allocas, runtime buffers, the C ABI table's `ptr`).
    pub(super) const PTR: Self = Self::Ptr(0);

    /// The scalar lowering of a width-1 SIMD dtype.
    pub(super) const fn of_dtype(dtype: Dtype) -> Self {
        match dtype {
            Dtype::Int => Self::Int,
            Dtype::UInt => Self::UInt,
            Dtype::Float64 => Self::Float64,
            Dtype::Bool => Self::Bool,
            sized => Self::Sized(sized),
        }
    }

    /// An integer scalar's `(bits, signed)` lane shape — `Int`/`UInt` are
    /// 64-bit signed/unsigned, sized integers use the VM's lane table — or
    /// `None` for floats, `Bool`, and `Ptr`.
    pub(super) const fn int_shape(self) -> Option<(u32, bool)> {
        match self {
            Self::Int => Some((64, true)),
            Self::UInt => Some((64, false)),
            Self::Sized(dtype) => mojito_vm::runtime::integer_dtype_bits(dtype),
            _ => None,
        }
    }

    pub(super) fn handle(self, ctx: &Context) -> TypeHandle {
        match self {
            Self::Int | Self::UInt => IntegerType::get(ctx, 64, Signedness::Signless).into(),
            Self::Float64 => FP64Type::get(ctx).into(),
            Self::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
            Self::Ptr(space) => PointerType::get(ctx, space).into(),
            Self::Dtype => IntegerType::get(ctx, 8, Signedness::Signless).into(),
            Self::Sized(Dtype::Float16) => FP16Type::get(ctx).into(),
            Self::Sized(Dtype::Float32) => FP32Type::get(ctx).into(),
            Self::Sized(dtype) => {
                let (bits, _) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("of_dtype leaves only sized integers and narrow floats in Sized");
                IntegerType::get(ctx, bits, Signedness::Signless).into()
            }
        }
    }

    pub(super) const fn ret_kind(self) -> RetKind {
        match self {
            Self::Int => RetKind::I64,
            Self::UInt => RetKind::U64,
            Self::Float64 => RetKind::F64,
            Self::Bool => RetKind::Bool,
            Self::Ptr(_) => RetKind::Ptr,
            // The typed JIT harness reads a `DType` return as its code byte.
            Self::Dtype => RetKind::Sized(Dtype::UInt8),
            Self::Sized(dtype) => RetKind::Sized(dtype),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Int => "Int",
            Self::UInt => "UInt",
            Self::Float64 => "Float64",
            Self::Bool => "Bool",
            Self::Ptr(_) => "Pointer",
            Self::Dtype => "DType",
            Self::Sized(dtype) => dtype.scalar_alias().unwrap_or_else(|| dtype.name()),
        }
    }
}

/// Classify a checked type for lowering, through the type LLVM translation
/// sees ([`MirDeclarations::translated_type`]): a vector classifies as its
/// `!kgen.simd` storage, and a one-field `TrivialRegisterPassable` struct as
/// its field, as upstream's `LowerKGENToLLVM` type converter unwraps both;
/// scalars (a one-lane storage) stay SSA; `None`, the field-less literals,
/// and a register-passable struct or tuple of size zero are zero-sized, as
/// the converter lowers an empty register-passable `!kgen.struct` to a type
/// holding no bits; struct, tuple, and `!kgen.string`-descriptor aggregates take
/// their shared-engine layout, as does the two-word `{ invoke, env }`
/// retained-callable value. A multi-lane storage keeps the lane-aligned
/// aggregate as its storage and ABI form; `lower/simd.rs` computes on it as
/// an LLVM fixed vector between the loads and stores.
pub fn lower_ty(
    function: &str,
    ty: &Ty,
    layout: &LayoutCx<'_>,
    declarations: &MirDeclarations,
    location: Option<SourceSpan>,
) -> Result<LowerTy, PlironError> {
    // A literal is upstream's field-less `IntLiteral[v]`,
    // `FloatLiteral[v]`, or `StringLiteral[v]`: its readers take the
    // number or text from the type.
    if ty.is_numeric_literal() || ty.is_string_literal() {
        return Ok(LowerTy::ZeroSized);
    }
    if matches!(ty.kind(), TyKind::Struct(..)) {
        let translated = declarations.translated_type(ty);
        if translated != *ty {
            return lower_ty(function, &translated, layout, declarations, location);
        }
    }
    match ty.kind() {
        // The MLIR builtin `i1` is `Bool`'s held `!kgen.scalar<bool>`.
        TyKind::I1 => Ok(LowerTy::Scalar(ScalarTy::Bool)),
        // A `!kgen.dtype` is the one-byte code a `DType` holds.
        TyKind::KgenDtype => Ok(LowerTy::Scalar(ScalarTy::Dtype)),
        // The MLIR builtin `ui8` is held as the `UInt8` it casts from.
        TyKind::Ui8 => Ok(LowerTy::Scalar(ScalarTy::of_dtype(Dtype::UInt8))),
        // The MLIR builtin `index` is held as the `Int` it casts from.
        TyKind::Index => Ok(LowerTy::Scalar(ScalarTy::Int)),
        // A one-lane storage is the scalar, so `Int` stays an SSA `i64`.
        TyKind::KgenSimd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(1),
        } => Ok(LowerTy::Scalar(ScalarTy::of_dtype(*dtype))),
        // Origins and ownership facts erase after validation; a pointer is
        // one opaque target pointer in its address space, upstream's
        // `ptr addrspace(N)`, regardless of its element type.
        _ if let Some(space) = address_space_slot(ty) => Ok(LowerTy::Scalar(pointer_scalar(
            function, ty, space, location,
        )?)),
        // A `!kgen.generator` is a trivial register type that holds nothing,
        // and a `Never` register is never written: its call ends the block.
        TyKind::None | TyKind::Never | TyKind::Generator(_) => Ok(LowerTy::ZeroSized),
        // A `!kgen.string`'s storage is the borrowed `MjStrDesc` descriptor.
        // A retained callable is the two-word `{ invoke, env }` value. A
        // vector of addresses is its `width` pointers.
        TyKind::KgenSimd { .. }
        | TyKind::KgenAddressSimd { .. }
        | TyKind::Struct(..)
        | TyKind::Tuple(_)
        | TyKind::PopArray { .. }
        | TyKind::RuntimePack(_)
        | TyKind::LitRefPack { .. }
        | TyKind::Variant(_)
        | TyKind::KgenString
        | TyKind::Func(_)
            if !ty.is_simd() =>
        {
            match layout.layout_of(ty) {
                // An empty register-passable `!kgen.struct` lowers to a type
                // that holds nothing, so it has no runtime representation.
                Ok(computed)
                    if computed.size == 0
                        && matches!(ty.kind(), TyKind::Struct(..) | TyKind::Tuple(_))
                        && declarations.register_passable(ty) =>
                {
                    Ok(LowerTy::ZeroSized)
                }
                Ok(computed) => Ok(LowerTy::Aggregate {
                    ty: ty.clone(),
                    layout: computed,
                }),
                Err(error) => Err(PlironError {
                    function: Some(function.to_string()),
                    kind: PlironErrorKind::Unsupported {
                        construct: format!("type `{ty:?}` ({error})"),
                    },
                    location,
                }),
            }
        }
        other => Err(PlironError {
            function: Some(function.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("type `{other:?}`"),
            },
            location,
        }),
    }
}

/// The address-space slot of a `Pointer`, a `!kgen.pointer`, a `!lit.ref`,
/// or a reference.
pub(super) fn address_space_slot(ty: &Ty) -> Option<&AddressSpaceSlot> {
    match ty.kind() {
        TyKind::Ref(reference) => Some(&reference.address_space),
        kind => kind.pointer_address_space(),
    }
}

/// The `Ptr` scalar of the pointer or reference type `ty` in `space`. A
/// space elaboration left symbolic is a broken invariant, reported rather
/// than guessed.
pub(super) fn pointer_scalar(
    function: &str,
    ty: &Ty,
    space: &AddressSpaceSlot,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    space.known().map(ScalarTy::Ptr).ok_or_else(|| PlironError {
        function: Some(function.to_string()),
        kind: PlironErrorKind::Unsupported {
            construct: format!("a pointer in the unresolved address space {space} (`{ty}`)"),
        },
        location,
    })
}

/// The address space of the pointer value `value`.
pub(super) fn value_address_space(ctx: &Context, value: Value) -> u32 {
    value
        .get_type(ctx)
        .deref(ctx)
        .downcast_ref::<PointerType>()
        .map_or(0, PointerType::address_space)
}
