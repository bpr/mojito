//! A minimal `spike` dialect with one op, `spike.answer : () -> i32`, plus a
//! dialect-conversion lowering into the LLVM dialect. This rehearses the
//! Stage 1 shape: custom op with textual syntax, verifier, negative coverage,
//! and a total conversion with a legality check.

use pliron::{
    builtin::{
        attributes::IntegerAttr,
        op_interfaces::{NOpdsInterface, OneResultInterface},
        ops::ModuleOp,
        types::{IntegerType, Signedness},
    },
    context::{Context, Ptr},
    derive::{op_interface_impl, pliron_op},
    irbuild::dialect_conversion::{
        DialectConversion, DialectConversionRewriter, OperandsInfo, PassWrapper,
    },
    irbuild::{inserter::Inserter, rewriter::Rewriter},
    linked_list::ContainsLinkedList,
    op::{Op, op_cast},
    operation::Operation,
    pass::{GuardedPass, OpGuard, OpPass},
    result::Result,
    r#type::{TypeHandle, Typed, TypedHandle},
    utils::apint::{APInt, bw},
    verify_err,
};
use pliron_llvm::{ToLLVMDialect, ops::ConstantOp as LLVMConstantOp};

/// The number the spike dialect is about.
pub const ANSWER: u64 = 42;

/// `spike.answer`: no operands, one result, which must be a signless i32.
#[pliron_op(
    name = "spike.answer",
    format = "`: ` type($0)",
    interfaces = [NOpdsInterface<0>, OneResultInterface],
)]
pub struct AnswerOp;

impl AnswerOp {
    /// Create a well-typed `spike.answer` (result type i32).
    pub fn new(ctx: &mut Context) -> Self {
        let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
        Self::new_with_result_type(ctx, i32_ty.into())
    }

    /// Create a `spike.answer` with an arbitrary result type; anything but a
    /// signless i32 must be rejected by the verifier (negative coverage).
    pub fn new_with_result_type(ctx: &mut Context, result_ty: TypeHandle) -> Self {
        let op = Operation::new(
            ctx,
            Self::get_concrete_op_info(),
            vec![result_ty],
            vec![],
            vec![],
            0,
        );
        AnswerOp { op }
    }
}

impl pliron::common_traits::Verify for AnswerOp {
    fn verify(&self, ctx: &Context) -> Result<()> {
        let loc = self.loc(ctx);
        let ty = self.get_result(ctx).get_type(ctx);
        let is_i32 = TypedHandle::<IntegerType>::from_handle(ty, ctx)
            .map(|handle| {
                let int_ty = handle.deref(ctx);
                int_ty.width() == 32 && matches!(int_ty.signedness(), Signedness::Signless)
            })
            .unwrap_or(false);
        if !is_i32 {
            verify_err!(loc, "spike.answer result must be a signless i32")?;
        }
        Ok(())
    }
}

/// Lower `spike.answer` to `llvm.constant <42: i32>`.
#[op_interface_impl]
impl ToLLVMDialect for AnswerOp {
    fn rewrite(
        &self,
        ctx: &mut Context,
        rewriter: &mut DialectConversionRewriter,
        _operands_info: &OperandsInfo,
    ) -> Result<()> {
        let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
        let c42 = LLVMConstantOp::new(
            ctx,
            Box::new(IntegerAttr::new(i32_ty, APInt::from_u64(ANSWER, bw(32)))),
        );
        rewriter.insert_operation(ctx, c42.get_operation());
        rewriter.replace_operation(ctx, self.get_operation(), c42.get_operation());
        Ok(())
    }
}

/// Dialect conversion that rewrites every `spike.*` op via [ToLLVMDialect].
#[derive(Default)]
pub struct SpikeToLLVMConversion;

impl DialectConversion for SpikeToLLVMConversion {
    fn can_convert_op(&self, ctx: &Context, op: Ptr<Operation>) -> bool {
        Operation::get_opid(op, ctx) == AnswerOp::get_opid_static()
    }

    fn rewrite(
        &mut self,
        ctx: &mut Context,
        rewriter: &mut DialectConversionRewriter,
        op: Ptr<Operation>,
        operands_info: &OperandsInfo,
    ) -> Result<()> {
        let op_dyn = Operation::get_op_dyn(op, ctx);
        if let Some(to_llvm) = op_cast::<dyn ToLLVMDialect>(op_dyn.op_ref()) {
            to_llvm.rewrite(ctx, rewriter, operands_info)?;
        }
        Ok(())
    }
}

/// A [ModuleOp] pass applying [SpikeToLLVMConversion] across the module.
pub fn spike_to_llvm_pass() -> OpPass<ModuleOp, PassWrapper<SpikeToLLVMConversion>> {
    let pass = PassWrapper::new("spike_to_llvm", SpikeToLLVMConversion);
    GuardedPass::new(OpGuard::<ModuleOp>::default(), pass)
}

/// Count ops under `root` (inclusive) whose dialect matches `dialect`.
/// Conversion legality: after lowering, the `spike` count must be zero.
pub fn count_ops_in_dialect(ctx: &Context, root: Ptr<Operation>, dialect: &str) -> usize {
    let mut count = usize::from(Operation::get_opid(root, ctx).dialect.to_string() == dialect);
    for region in root.deref(ctx).regions() {
        for block in region.deref(ctx).iter(ctx) {
            for op in block.deref(ctx).iter(ctx) {
                count += count_ops_in_dialect(ctx, op, dialect);
            }
        }
    }
    count
}
