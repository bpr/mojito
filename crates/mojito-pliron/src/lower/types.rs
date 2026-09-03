//! Scalar/lowered type classification: [`Locator`] span resolution,
//! [`ScalarTy`]/[`LowerTy`] helpers, and `lower_ty`.

use super::*;

impl Locator {
    pub(crate) fn new(ctx: &mut Context, sources: &[(String, String)]) -> Locator {
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
        Locator { sources }
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
    /// The scalar lowering of a width-1 SIMD dtype.
    pub(super) fn of_dtype(dtype: Dtype) -> ScalarTy {
        match dtype {
            Dtype::Int => ScalarTy::Int,
            Dtype::Float64 => ScalarTy::Float64,
            Dtype::Bool => ScalarTy::Bool,
            sized => ScalarTy::Sized(sized),
        }
    }

    /// An integer scalar's `(bits, signed)` lane shape — `Int`/`UInt` are
    /// 64-bit signed/unsigned, sized integers use the VM's lane table — or
    /// `None` for floats, `Bool`, and `Ptr`.
    pub(super) fn int_shape(self) -> Option<(u32, bool)> {
        match self {
            ScalarTy::Int => Some((64, true)),
            ScalarTy::UInt => Some((64, false)),
            ScalarTy::Sized(dtype) => mojito_vm::runtime::integer_dtype_bits(dtype),
            _ => None,
        }
    }

    pub(super) fn handle(self, ctx: &mut Context) -> TypeHandle {
        match self {
            ScalarTy::Int | ScalarTy::UInt => {
                IntegerType::get(ctx, 64, Signedness::Signless).into()
            }
            ScalarTy::Float64 => FP64Type::get(ctx).into(),
            ScalarTy::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
            ScalarTy::Ptr => PointerType::get(ctx, 0).into(),
            ScalarTy::Sized(Dtype::Float32) => FP32Type::get(ctx).into(),
            ScalarTy::Sized(dtype) => {
                let (bits, _) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("of_dtype leaves only sized integers and Float32 in Sized");
                IntegerType::get(ctx, bits, Signedness::Signless).into()
            }
        }
    }

    pub(super) fn ret_kind(self) -> RetKind {
        match self {
            ScalarTy::Int => RetKind::I64,
            ScalarTy::UInt => RetKind::U64,
            ScalarTy::Float64 => RetKind::F64,
            ScalarTy::Bool => RetKind::Bool,
            ScalarTy::Ptr => RetKind::Ptr,
            ScalarTy::Sized(dtype) => RetKind::Sized(dtype),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            ScalarTy::Int => "Int",
            ScalarTy::UInt => "UInt",
            ScalarTy::Float64 => "Float64",
            ScalarTy::Bool => "Bool",
            ScalarTy::Ptr => "Pointer",
            ScalarTy::Sized(dtype) => dtype.scalar_alias().unwrap_or_else(|| dtype.name()),
        }
    }
}

/// Classify a checked type for lowering: scalars (including width-1 SIMD
/// aliases and the i64/f64 storage of `IntLiteral`/`FloatLiteral`-typed
/// registers) stay SSA; `None` is zero-sized; struct, tuple, and
/// `StringLiteral`-descriptor aggregates take their shared-engine layout, as
/// does the two-word `{ invoke, env }` retained-callable value. Multi-lane
/// SIMD values are deliberately memory-resident scalar aggregates; native
/// vector types belong to the later SIMD optimization stage.
pub(crate) fn lower_ty(
    function: &str,
    ty: &Ty,
    layout: &LayoutCx<'_>,
    location: Option<SourceSpan>,
) -> Result<LowerTy, PlironError> {
    match ty {
        Ty::Int => Ok(LowerTy::Scalar(ScalarTy::Int)),
        Ty::UInt => Ok(LowerTy::Scalar(ScalarTy::UInt)),
        Ty::Float64 => Ok(LowerTy::Scalar(ScalarTy::Float64)),
        Ty::Bool => Ok(LowerTy::Scalar(ScalarTy::Bool)),
        Ty::Simd { dtype, width: 1 } => Ok(LowerTy::Scalar(ScalarTy::of_dtype(*dtype))),
        // Literal-typed storage holds the default materialized value; a
        // constant that exceeds it rejects at the storage boundary rather
        // than wrapping (the VM keeps arbitrary precision).
        Ty::IntLiteral => Ok(LowerTy::Scalar(ScalarTy::Int)),
        Ty::FloatLiteral => Ok(LowerTy::Scalar(ScalarTy::Float64)),
        // Origins and ownership facts erase after validation; a pointer is
        // one opaque target pointer regardless of its element type.
        Ty::Pointer { .. } | Ty::Ref(_) => Ok(LowerTy::Scalar(ScalarTy::Ptr)),
        Ty::None => Ok(LowerTy::ZeroSized),
        // The built-in error value is `MjError { message: MjString }` storage;
        // its message buffer frees invisibly on drop (no user destructor).
        // `StringLiteral` storage is the borrowed `MjStrDesc` descriptor.
        // A retained callable is the two-word `{ invoke, env }` value.
        Ty::Error
        | Ty::Struct(..)
        | Ty::Tuple(_)
        | Ty::RuntimePack(_)
        | Ty::Variant(_)
        | Ty::Simd { .. }
        | Ty::StringLiteral
        | Ty::Func { .. } => match layout.layout_of(ty) {
            Ok(computed) => Ok(LowerTy::Aggregate {
                ty: Box::new(ty.clone()),
                layout: computed,
            }),
            Err(error) => Err(PlironError {
                function: Some(function.to_string()),
                kind: PlironErrorKind::Unsupported {
                    construct: format!("type `{ty:?}` ({error})"),
                },
                location,
            }),
        },
        other => Err(PlironError {
            function: Some(function.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("type `{other:?}`"),
            },
            location,
        }),
    }
}
