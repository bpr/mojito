//! A hand-written rewriting pass: fold `llvm.add` of two integer constants
//! into a single `llvm.constant`. Exercises the [Pass] trait, IR mutation,
//! and use replacement; a follow-up built-in DCE run removes the dead inputs.

use pliron::{
    builtin::{attributes::IntegerAttr, op_interfaces::OneResultInterface},
    context::{Context, Ptr},
    irbuild::IRStatus,
    linked_list::ContainsLinkedList,
    op::Op,
    operation::Operation,
    pass::{AnalysisManager, Pass, PassResult},
    result::Result,
    value::Value,
};
use pliron_llvm::ops::{AddOp, ConstantOp};

/// Fold every `llvm.add` whose operands are both integer `llvm.constant`s.
#[derive(Default)]
pub struct FoldConstAdd;

impl Pass for FoldConstAdd {
    fn name(&self) -> &str {
        "fold_const_add"
    }

    fn run(
        &mut self,
        op: Ptr<Operation>,
        ctx: &mut Context,
        _analyses: &mut AnalysisManager,
    ) -> Result<PassResult> {
        let mut foldable = Vec::new();
        collect_foldable_adds(ctx, op, &mut foldable);

        let changed = !foldable.is_empty();
        for (add_ptr, lhs, rhs) in foldable {
            let sum = lhs.value().add(&rhs.value());
            let folded = ConstantOp::new(ctx, Box::new(IntegerAttr::new(lhs.get_type(), sum)));
            folded.get_operation().insert_after(ctx, add_ptr);
            let add_op = Operation::get_op::<AddOp>(add_ptr, ctx).expect("collected an llvm.add");
            add_op.get_result(ctx).replace_some_uses_with(
                ctx,
                |_, _| true,
                &folded.get_result(ctx),
            );
            Operation::erase(add_ptr, ctx);
        }

        let mut res = PassResult::default();
        res.ir_changed = if changed {
            IRStatus::Changed
        } else {
            IRStatus::Unchanged
        };
        Ok(res)
    }
}

/// Recursively collect `llvm.add` ops whose operands are both defined by
/// integer `llvm.constant`s of the same type.
fn collect_foldable_adds(
    ctx: &Context,
    root: Ptr<Operation>,
    out: &mut Vec<(Ptr<Operation>, IntegerAttr, IntegerAttr)>,
) {
    for region in root.deref(ctx).regions() {
        for block in region.deref(ctx).iter(ctx) {
            for op in block.deref(ctx).iter(ctx) {
                if Operation::get_op::<AddOp>(op, ctx).is_some() {
                    let lhs = as_integer_constant(ctx, op.deref(ctx).get_operand(0));
                    let rhs = as_integer_constant(ctx, op.deref(ctx).get_operand(1));
                    if let (Some(lhs), Some(rhs)) = (lhs, rhs)
                        && lhs.get_type() == rhs.get_type()
                    {
                        out.push((op, lhs, rhs));
                    }
                }
                collect_foldable_adds(ctx, op, out);
            }
        }
    }
}

/// If `value` is the result of an integer `llvm.constant`, return its attr.
fn as_integer_constant(ctx: &Context, value: Value) -> Option<IntegerAttr> {
    let const_op = Operation::get_op::<ConstantOp>(value.defining_op()?, ctx)?;
    const_op
        .get_value(ctx)
        .downcast_ref::<IntegerAttr>()
        .cloned()
}
