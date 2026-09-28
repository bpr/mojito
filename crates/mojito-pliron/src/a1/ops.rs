//! The operations of `mojito_core`. Each is handwritten, printed in
//! Pliron's canonical syntax, and verified through the closed registry.

use pliron::basic_block::BasicBlock;
use pliron::builtin::op_interfaces::{
    IsTerminatorInterface, IsolatedFromAboveInterface, NOpdsInterface, NResultsInterface,
    OneRegionInterface, SymbolOpInterface,
};
use pliron::common_traits::Verify;
use pliron::context::{Context, Ptr};
use pliron::derive::pliron_op;
use pliron::dict_key;
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::result::Result;
use pliron::r#type::TypeHandle;
use pliron::value::Value;

use super::inventory::CoreOpKind;
use super::verify::verify_core_op;

dict_key!(
    /// The stable identity every core operation carries.
    KEY_IDENTITY, "mojito_core_identity"
);
dict_key!(
    /// The source provenance every core operation carries.
    KEY_PROVENANCE, "mojito_core_provenance"
);
dict_key!(
    /// The MIR register a value result defines.
    KEY_REG, "mojito_core_reg"
);
dict_key!(KEY_SLOT, "mojito_core_slot");
dict_key!(KEY_CONSTANT, "mojito_core_constant");
dict_key!(KEY_INFIX, "mojito_core_infix");
dict_key!(KEY_RESOLVED, "mojito_core_resolved");
dict_key!(KEY_USE_MODE, "mojito_core_use_mode");
dict_key!(KEY_STORE, "mojito_core_store");
dict_key!(KEY_PROJECTION, "mojito_core_projection");
dict_key!(KEY_CALL, "mojito_core_call");
dict_key!(KEY_LIFECYCLE, "mojito_core_lifecycle");
dict_key!(KEY_LOANS, "mojito_core_loans");
dict_key!(KEY_INVALIDATION, "mojito_core_invalidation");
dict_key!(KEY_TRY, "mojito_core_try");
dict_key!(KEY_EXIT, "mojito_core_exit");
dict_key!(KEY_SIGNATURE, "mojito_core_signature");
dict_key!(KEY_DECLARATION, "mojito_core_declaration");
dict_key!(KEY_ORPHANS, "mojito_core_orphans");
dict_key!(KEY_TABLES, "mojito_core_tables");
dict_key!(KEY_DEAD_TERM, "mojito_core_dead_term");
dict_key!(KEY_OUTCOME, "mojito_core_outcome");
dict_key!(KEY_LAYOUT, "mojito_core_layout");
dict_key!(KEY_CONTRACT, "mojito_core_contract");
dict_key!(KEY_PREFIX, "mojito_core_prefix");
dict_key!(KEY_SIMD_CONVERT, "mojito_core_simd_convert");
dict_key!(KEY_SUBSCRIPT, "mojito_core_subscript");
dict_key!(KEY_MULTI_SET, "mojito_core_multi_set");
dict_key!(KEY_POINTER_STORAGE, "mojito_core_pointer_storage");
dict_key!(KEY_ITER_INIT, "mojito_core_iter_init");
dict_key!(KEY_ITER_NEXT, "mojito_core_iter_next");
dict_key!(
    /// The count of cleanup slots a `return` carries out.
    KEY_CLEANUP, "mojito_core_cleanup"
);

macro_rules! core_op {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $kind:ident, [$($iface:ty),*]) => {
        $(#[$doc])*
        #[pliron_op(name = $name, format, interfaces = [$($iface),*])]
        pub struct $ty;

        impl Verify for $ty {
            fn verify(&self, ctx: &Context) -> Result<()> {
                verify_core_op(ctx, self.get_operation(), CoreOpKind::$kind)
            }
        }
    };
}

core_op!(
    /// A function: one region whose entry block takes the parameters and
    /// the effect token.
    FuncOp, "mojito_core.func", Func,
    [OneRegionInterface, SymbolOpInterface, IsolatedFromAboveInterface, NOpdsInterface<0>, NResultsInterface<0>]
);
core_op!(
    /// A variable or register slot, allocated once at function entry.
    SlotOp, "mojito_core.slot", Slot, []
);
core_op!(ConstOp, "mojito_core.const", Const, [NOpdsInterface<0>]);
core_op!(
    MaterializeOp,
    "mojito_core.materialize",
    Materialize,
    [NOpdsInterface<1>]
);
core_op!(BinaryOp, "mojito_core.binary", Binary, [NOpdsInterface<2>]);
core_op!(SimdMakeOp, "mojito_core.simd_make", SimdMake, []);
core_op!(UnaryOp, "mojito_core.unary", Unary, [NOpdsInterface<1>]);
core_op!(
    /// A lane-wise conversion or bit reinterpretation to the result type.
    SimdConvertOp, "mojito_core.simd_convert", SimdConvert, [NOpdsInterface<1>]
);
core_op!(UseOp, "mojito_core.use", Use, [NOpdsInterface<2>]);
core_op!(StoreOp, "mojito_core.store", Store, [NOpdsInterface<3>]);
core_op!(ProjectOp, "mojito_core.project", Project, []);
core_op!(LoadOp, "mojito_core.load", Load, [NOpdsInterface<2>]);
core_op!(
    RefMakeOp,
    "mojito_core.ref_make",
    RefMake,
    [NOpdsInterface<1>]
);
core_op!(
    RefStoreOp,
    "mojito_core.ref_store",
    RefStore,
    [NOpdsInterface<3>]
);
core_op!(
    RefReadOp,
    "mojito_core.ref_read",
    RefRead,
    [NOpdsInterface<2>]
);
core_op!(
    /// An owned lifecycle copy of a value.
    CopyOp, "mojito_core.copy", Copy, [NOpdsInterface<2>]
);
core_op!(MoveOp, "mojito_core.move", Move, [NOpdsInterface<2>]);
core_op!(
    /// An owner's live range extended to this point.
    KeepAliveOp, "mojito_core.keep_alive", KeepAlive, [NOpdsInterface<2>]
);
core_op!(IndexOp, "mojito_core.index", Index, []);
core_op!(MultiSetOp, "mojito_core.multi_set", MultiSet, []);
core_op!(
    /// An element taken from, or destroyed in, pointer collection storage.
    PointerStorageOp, "mojito_core.pointer_storage", PointerStorage, [NOpdsInterface<3>]
);
core_op!(
    IterInitOp,
    "mojito_core.iter_init",
    IterInit,
    [NOpdsInterface<3>]
);
core_op!(
    /// One advance of a nominal iterator: the element, and whether there
    /// was one.
    IterNextOp, "mojito_core.iter_next", IterNext, [NOpdsInterface<2>]
);
core_op!(CallOp, "mojito_core.call", Call, []);
core_op!(
    /// A raising call with explicit normal and error successors.
    InvokeOp, "mojito_core.invoke", Invoke, [IsTerminatorInterface, NResultsInterface<0>]
);
core_op!(DropOp, "mojito_core.drop", Drop, [NOpdsInterface<2>]);
core_op!(
    ConsumeOp,
    "mojito_core.consume",
    Consume,
    [NOpdsInterface<2>]
);
core_op!(LoansOp, "mojito_core.loans", Loans, []);
core_op!(
    InvalidateOp,
    "mojito_core.invalidate",
    Invalidate,
    [NOpdsInterface<1>]
);
core_op!(BrOp, "mojito_core.br", Br, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(CondBrOp, "mojito_core.cond_br", CondBr, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(ReturnOp, "mojito_core.return", Return, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(RaiseOp, "mojito_core.raise", Raise, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(
    /// The pending outcome a `finally` body runs under.
    OutcomeOp, "mojito_core.outcome", Outcome, []
);
core_op!(
    /// Dispatch on a pending outcome after its `finally` body.
    ResumeOp, "mojito_core.resume", Resume, [IsTerminatorInterface, NResultsInterface<0>]
);
core_op!(
    /// A structured `try`, legal only before outcome normalization.
    TryBridgeOp, "mojito_core.try_bridge", TryBridge, []
);
core_op!(
    /// The end of a structured region's block, legal only before outcome
    /// normalization.
    RegionExitOp, "mojito_core.region_exit", RegionExit, [IsTerminatorInterface, NResultsInterface<0>]
);

/// Create an unlinked operation of `kind`.
pub fn build(
    ctx: &mut Context,
    kind: CoreOpKind,
    results: Vec<TypeHandle>,
    operands: Vec<Value>,
    successors: Vec<Ptr<BasicBlock>>,
    regions: usize,
) -> Ptr<Operation> {
    Operation::new(
        ctx,
        kind.concrete_op_info(),
        results,
        operands,
        successors,
        regions,
    )
}
