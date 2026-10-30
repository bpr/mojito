//! The `__mlir_op` forms Mojito admits: upstream's
//! `lit.ownership.mark_initialized` and `lit.ownership.mark_destroyed` over a
//! place, which the bundled standard library spells before writing a value's
//! storage through pointers or lane by lane and after moving it out through
//! them,
//! `kgen.struct.extract` of an element of a literal struct,
//! `pop.string.size` and `pop.string.address` over a `!kgen.string`, the
//! `pop.array` ops of `pop_array_ops.rs`, the
//! `pop` and `index` ops of `pop_ops.rs` with its `pop.simd.shuffle`, and the LLVM
//! intrinsic call of `llvm_intrinsic.rs`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::ParamOp;

impl Checker {
    /// Type the `__mlir_op` statement `lit.ownership.mark_initialized` or
    /// `lit.ownership.mark_destroyed` over `__get_mvalue_as_litref(place)`.
    /// The first marks `place` initialized, so a constructor may write its
    /// storage through pointers (`Tuple.__init__`); the second ends its
    /// value without destroying it, after its storage was moved out through
    /// pointers (`Tuple.consume_elements`). Each is accepted only in the
    /// bundled modules that reach compiler-private storage or, as `SIMD`'s
    /// element list and `StaticTuple`'s initializers do, initialize `out
    /// self` element by element. Every other
    /// spelling is an LLVM intrinsic call ([`Self::infer_llvm_intrinsic_op`]),
    /// a struct element read ([`Self::infer_kgen_struct_extract_op`]), a
    /// KGEN string read ([`Self::infer_string_op`]), a lane gather
    /// ([`Self::infer_simd_shuffle_op`]), or a
    /// `pop` or `index` op ([`Self::infer_pop_op`]). The
    /// statement's checked record is `SemanticAdjustment::MarkInitialized`
    /// or `SemanticAdjustment::MarkDestroyed`.
    pub(super) fn infer_mlir_op(
        &self,
        span: &SourceSpan,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        const INITIALIZED: &str = "lit.ownership.mark_initialized";
        const DESTROYED: &str = "lit.ownership.mark_destroyed";
        const LLVM_INTRINSIC: &str = "pop.call_llvm_intrinsic";
        const STRUCT_EXTRACT: &str = "kgen.struct.extract";
        const SIMD_SHUFFLE: &str = "pop.simd.shuffle";
        const STRING_SIZE: &str = "pop.string.size";
        const STRING_ADDRESS: &str = "pop.string.address";
        if method == LLVM_INTRINSIC {
            return self.infer_llvm_intrinsic_op(span, method, call);
        }
        if method == STRUCT_EXTRACT {
            return self.infer_kgen_struct_extract_op(span, method, call);
        }
        if method == SIMD_SHUFFLE {
            return self.infer_simd_shuffle_op(span, method, call);
        }
        if matches!(
            method,
            super::pop_array_ops::POP_ARRAY_REPEAT
                | super::pop_array_ops::POP_ARRAY_GET
                | super::pop_array_ops::POP_ARRAY_REPLACE
        ) {
            return self.infer_pop_array_op(span, method, call);
        }
        if method == super::pop_array_ops::POP_ARRAY_GEP {
            return self.infer_pop_array_gep(span, call);
        }
        if method == STRING_SIZE {
            return self.infer_string_op(span, method, ParamOp::StringSize, call);
        }
        if method == STRING_ADDRESS
            && super::super::overload_support::is_bundled_kgen_string_source(span.source.as_deref())
        {
            return self.infer_string_op(span, method, ParamOp::StringAddress, call);
        }
        if !matches!(method, INITIALIZED | DESTROYED) {
            return self.infer_pop_op(span, method, call);
        }
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let rejected = || {
            TypeError::Unsupported(format!(
                "'__mlir_op' is accepted only as '{INITIALIZED}' or '{DESTROYED}' over \
                 '__get_mvalue_as_litref(place)' in the bundled standard library"
            ))
        };
        if !param_args.is_empty()
            || !kwargs.is_empty()
            || !super::super::overload_support::is_bundled_ownership_marker_source(
                span.source.as_deref(),
            )
        {
            return Err(rejected());
        }
        let [
            Expr {
                kind:
                    ExprKind::Call {
                        name,
                        param_args,
                        args,
                        kwargs,
                    },
                ..
            },
        ] = args
        else {
            return Err(rejected());
        };
        let [place] = args.as_slice() else {
            return Err(rejected());
        };
        if name != "__get_mvalue_as_litref" || !param_args.is_empty() || !kwargs.is_empty() {
            return Err(rejected());
        }
        self.origin_place(place).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => rejected(),
        })?;
        self.infer(place)?;
        self.record_at(span.clone()).adjustment = Some(Box::new(if method == INITIALIZED {
            mojito_checked::checked::SemanticAdjustment::MarkInitialized
        } else {
            mojito_checked::checked::SemanticAdjustment::MarkDestroyed
        }));
        Ok(Ty::none())
    }

    /// Type ``__mlir_op.`kgen.struct.extract`[index=i.__mlir_index__()](s)``,
    /// recording it as `SemanticAdjustment::KgenStructExtract`: element `i`
    /// of the literal struct `s` (`!kgen.struct<…>`, a `TyKind::Tuple`),
    /// typed as a compile-time subscript of that storage types it. The
    /// `index` attribute is a compile-time `Int`'s `__mlir_index__()`, as
    /// upstream's `_RegisterPackType.__getitem_param__` spells it.
    fn infer_kgen_struct_extract_op(
        &self,
        span: &SourceSpan,
        name: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let invalid = |reason: &str| TypeError::BadCall {
            func: name.to_string(),
            reason: reason.to_string(),
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional"));
        }
        let mut index = None;
        for argument in param_args {
            match argument {
                mojito_ast::ast::ParamArg::Named {
                    name: attribute,
                    value,
                } if attribute == "index" => index = Some(value.as_ref()),
                mojito_ast::ast::ParamArg::Named {
                    name: attribute, ..
                } => {
                    return Err(invalid(&format!(
                        "attribute '{attribute}' is not an inherent attribute of '{name}'"
                    )));
                }
                _ => return Err(invalid("op attributes are named")),
            }
        }
        let index = match index {
            Some(mojito_ast::ast::ParamArg::Value(Expr {
                kind:
                    ExprKind::MethodCall {
                        object,
                        method,
                        args,
                        kwargs,
                    },
                ..
            })) if method == "__mlir_index__" && args.is_empty() && kwargs.is_empty() => object,
            Some(_) => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' takes its 'index' attribute as a compile-time \
                     'Int.__mlir_index__()'"
                )));
            }
            None => return Err(invalid("op requires attribute 'index'")),
        };
        let [operand] = args else {
            return Err(invalid(&format!(
                "op takes 1 operand(s), got {}",
                args.len()
            )));
        };
        let operand_ty = self.infer(operand)?;
        let TyKind::Tuple(elements) = operand_ty.kind() else {
            return Err(invalid(&format!(
                "operand #0 must be a '!kgen.struct', but got '{operand_ty}'"
            )));
        };
        let built = self.compile_dependent_ct_expr(index)?;
        if !matches!(built.meta().as_value().map(Ty::kind), Some(TyKind::INT)) {
            return Err(invalid("attribute 'index' takes a compile-time 'Int'"));
        }
        let result = self.tuple_storage_element(&elements.iter().collect::<Vec<_>>(), index)?;
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::KgenStructExtract { index: built },
        ));
        Ok(result)
    }

    /// Type ``__mlir_op.`pop.string.size`(s)`` or
    /// ``__mlir_op.`pop.string.address`(s)`` over a `!kgen.string`: the
    /// string's byte length, an `index`, or the address of its static
    /// bytes. A compile-time operand (`Self.value` in
    /// `StringLiteral.byte_length`) records the parameter value `op` folds
    /// once the string is bound; a run-time one (`StaticString(_kgen)`)
    /// records the `pop` op that reads it. The address is a `Pointer[Byte]`, accepted only in the bundled
    /// library, where upstream's is the `!kgen.pointer` its `Pointer` wraps.
    fn infer_string_op(
        &self,
        span: &SourceSpan,
        name: &str,
        op: ParamOp,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let invalid = |reason: String| TypeError::BadCall {
            func: name.to_string(),
            reason,
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional".to_string()));
        }
        if let Some(argument) = param_args.first() {
            let attribute = match argument {
                mojito_ast::ast::ParamArg::Named { name, .. } => name.as_str(),
                _ => "",
            };
            return Err(invalid(format!(
                "attribute '{attribute}' is not an inherent attribute of '{name}'"
            )));
        }
        let [operand] = args else {
            return Err(invalid(format!(
                "op takes 1 operand(s), got {}",
                args.len()
            )));
        };
        let operand_ty = self.infer(operand)?;
        if !matches!(operand_ty.kind(), TyKind::KgenString) {
            return Err(invalid(format!(
                "operand #0 must be a '{}', but got '{operand_ty}'",
                mojito_types::types::KGEN_STRING_TYPE_SPELLING
            )));
        }
        let adjustment = match self.compile_dependent_ct_expr(operand) {
            Ok(string) => mojito_checked::checked::SemanticAdjustment::ParamValue {
                value: self
                    .param_context
                    .op(op, &[string])
                    .map_err(|error| TypeError::Unsupported(error.to_string()))?,
                materialized: None,
            },
            Err(_) => {
                mojito_checked::checked::SemanticAdjustment::MlirOp(if op == ParamOp::StringSize {
                    mojito_types::mlir::MlirOp::PopStringSize
                } else {
                    mojito_types::mlir::MlirOp::PopStringAddress
                })
            }
        };
        self.record_at(span.clone()).adjustment = Some(Box::new(adjustment));
        Ok(if op == ParamOp::StringSize {
            TyKind::Index.into()
        } else {
            mojito_types::types::static_string_address_ty()
        })
    }
}
