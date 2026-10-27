//! The `pop` and `index` `__mlir_op` expressions, lowered to the
//! instruction that computes each (`mojito_types::mlir::MlirOp::lowering`).

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

    /// Lower the checked `op` over `args`: a built-in operator whose lanes
    /// compute the op (`binary`, `unary`), a `simd.make` splat or
    /// `simd.cast` at the result type's slots, or a `pop` instruction. The
    /// destination is typed by the checked result.
    pub(super) fn lower_mlir_op(&mut self, call: Node<'a>, op: MlirOp, args: &[Node<'a>]) -> Reg {
        let operands: Vec<Reg> = args.iter().map(|argument| self.expr(*argument)).collect();
        let ty = call.ty().expect("a checked `__mlir_op` has a type");
        let dest = self.fresh_typed(call.location(), None, ty.clone());
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
            (MlirLowering::Pop(op), _) => MirInstr::Pop { dest, op, operands },
            _ => unreachable!("the checker admits `{}` with its arity", op.name()),
        });
        dest
    }
}
