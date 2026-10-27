//! The `pop` and `index` `__mlir_op` expressions, and the `SIMD` methods
//! whose upstream bodies are one `pop.simd` lane op, lowered to the
//! instruction that computes each (`mojito_types::mlir::MlirOp::lowering`),
//! and `pop.call_llvm_intrinsic`, lowered to `llvm.intrinsic`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::{MlirLowering, MlirOp};

impl<'a> Flatten<'a> {
    /// The op the checker recorded on `call`, if it is a `pop` or `index`
    /// `__mlir_op`.
    pub(super) fn checked_mlir_op(call: Node<'a>) -> Option<MlirOp> {
        call.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::MlirOp(op) => Some(*op),
                _ => None,
            })
    }

    /// The LLVM intrinsic call the checker recorded on `call`, if it is a
    /// `pop.call_llvm_intrinsic` `__mlir_op`: the intrinsic's name, its
    /// side-effect flag, and the position of a spread collector.
    pub(super) fn checked_llvm_intrinsic(call: Node<'a>) -> Option<CheckedLlvmIntrinsic> {
        call.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::LlvmIntrinsic {
                    intrinsic,
                    has_side_effect,
                    spread,
                } => Some(CheckedLlvmIntrinsic {
                    intrinsic: intrinsic.clone(),
                    has_side_effect: has_side_effect.clone(),
                    spread: *spread,
                }),
                _ => None,
            })
    }

    /// Lower the checked LLVM intrinsic call `call` over `args` to
    /// `llvm.intrinsic`, its destination typed by the call's `_type`. A
    /// closed name or flag is a value constant; a template's stays a
    /// parameter constant the elaborator closes.
    pub(super) fn lower_llvm_intrinsic(
        &mut self,
        call: Node<'a>,
        called: CheckedLlvmIntrinsic,
        args: &[Node<'a>],
    ) -> Reg {
        let operands: Vec<Reg> = args.iter().map(|argument| self.expr(*argument)).collect();
        let ty = call.ty().expect("a checked `__mlir_op` has a type");
        let dest = self.fresh_typed(call.location(), None, ty);
        let constant = |value: ParamExpr| match value.as_constant() {
            Some(closed) => Const::Value(closed.clone()),
            None => Const::Param(value),
        };
        self.emit(MirInstr::CallLlvmIntrinsic {
            dest,
            intrinsic: constant(called.intrinsic),
            has_side_effect: constant(called.has_side_effect),
            args: operands,
            spread: called.spread,
        });
        dest
    }

    /// The lane op the checker recorded on `call`, if it is a `SIMD` method
    /// whose operands are its receiver and arguments.
    pub(super) fn checked_simd_receiver_op(call: Node<'a>) -> Option<MlirOp> {
        call.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdReceiverOp(op) => Some(*op),
                _ => None,
            })
    }

    /// Lower the checked `op` over `args`: a built-in operator whose lanes
    /// compute the op (`binary`, `unary`), a `simd.make` splat or
    /// `simd.cast` at the result type's slots, a `pop` instruction, or a
    /// lane instruction. The destination is typed by the checked result.
    pub(super) fn lower_mlir_op(&mut self, call: Node<'a>, op: MlirOp, args: &[Node<'a>]) -> Reg {
        let operands: Vec<Reg> = args.iter().map(|argument| self.expr(*argument)).collect();
        let ty = call.ty().expect("a checked `__mlir_op` has a type");
        self.emit_mlir_op(call.location(), op, operands, &ty)
    }

    /// Lower the `SIMD` method `call` the checker resolved to the lane `op`
    /// over `object` and `args`. A `select` case of another type than the
    /// result (a scalar or a literal) splats to it first, and a `bool`
    /// reduction the checker typed `Bool` casts its one lane to it.
    pub(super) fn lower_simd_receiver_op(
        &mut self,
        call: Node<'a>,
        op: MlirOp,
        object: Node<'a>,
        args: &[Node<'a>],
    ) -> Reg {
        let ty = call.ty().expect("a checked SIMD method call has a type");
        let mut operands = vec![self.expr(object)];
        for argument in args {
            let value = self.expr(*argument);
            operands.push(match argument.ty() {
                Some(found) if op == MlirOp::PopSimdSelect && found != ty => {
                    self.f.reg_types.entry(value.0).or_insert(found);
                    self.splat_scalar(value, &ty, argument.location())
                }
                _ => value,
            });
        }
        let lane = match (op.lowering(), object.ty().as_ref().map(Ty::kind)) {
            (MlirLowering::Reduce(_), Some(TyKind::Simd { dtype, .. })) => {
                mojito_types::types::simd_ty_from_slots(
                    dtype.clone(),
                    mojito_types::types::SimdWidth::Known(1),
                )
                .ok()
            }
            _ => None,
        };
        let Some(lane) = lane.filter(|lane| *lane != ty) else {
            return self.emit_mlir_op(call.location(), op, operands, &ty);
        };
        let value = self.emit_mlir_op(call.location(), op, operands, &lane);
        let dest = self.fresh_typed(call.location(), None, Ty::bool());
        self.emit(MirInstr::ScalarCast {
            dest,
            value,
            target: ScalarConversion::Bool,
        });
        dest
    }

    /// Emit the instruction computing `op` over `operands` at `location`,
    /// its destination of type `ty`.
    fn emit_mlir_op(
        &mut self,
        location: SourceSpan,
        op: MlirOp,
        operands: Vec<Reg>,
        ty: &Ty,
    ) -> Reg {
        let dest = self.fresh_typed(location, None, ty.clone());
        let slots = || {
            mojito_types::types::simd_slots(ty.kind())
                .expect("a checked splat or cast names a SIMD type")
        };
        self.emit(match (op.lowering(), operands.as_slice()) {
            (MlirLowering::Binary(infix), [a, b]) => MirInstr::BinOp {
                op: infix,
                dest,
                a: *a,
                b: *b,
                resolved: None,
                raises: None,
                refined_bound: None,
            },
            (MlirLowering::Negate, [a]) => MirInstr::UnOp {
                op: PrefixOp::Neg,
                dest,
                a: *a,
                raises: None,
            },
            (MlirLowering::Splat, [value]) => {
                let (dtype, width) = slots();
                MirInstr::MakeSimd {
                    dest,
                    dtype,
                    width,
                    elems: vec![*value],
                }
            }
            (MlirLowering::Cast, [value]) => {
                let (dtype, width) = slots();
                MirInstr::SimdCast {
                    dest,
                    value: *value,
                    dtype,
                    width,
                }
            }
            (MlirLowering::Select, [mask, on_true, on_false]) => MirInstr::SimdSelect {
                dest,
                mask: *mask,
                on_true: *on_true,
                on_false: *on_false,
            },
            (MlirLowering::Extract, [value, index]) => MirInstr::SimdExtract {
                dest,
                value: *value,
                index: *index,
            },
            (MlirLowering::Insert, [value, element, index]) => MirInstr::SimdInsert {
                dest,
                value: *value,
                element: *element,
                index: *index,
            },
            (MlirLowering::Reduce(op), [value]) => MirInstr::SimdReduce {
                dest,
                value: *value,
                op,
            },
            (MlirLowering::Pop(op), _) => MirInstr::Pop { dest, op, operands },
            _ => unreachable!("the checker admits `{}` with its arity", op.name()),
        });
        dest
    }
}

/// A `pop.call_llvm_intrinsic` the checker recorded: the name and
/// side-effect flag, closed or parameter expressions, and the position of
/// the collector whose elements the call passes.
pub(super) struct CheckedLlvmIntrinsic {
    intrinsic: ParamExpr,
    has_side_effect: ParamExpr,
    spread: Option<usize>,
}
