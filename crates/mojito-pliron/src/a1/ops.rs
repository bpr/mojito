//! The operations of `mojito`. Each is handwritten, printed in
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
    KEY_IDENTITY, "identity"
);
dict_key!(
    /// The source provenance every core operation carries.
    KEY_PROVENANCE, "provenance"
);
dict_key!(
    /// The MIR register a value result defines.
    KEY_REG, "reg"
);
dict_key!(KEY_SLOT, "slot");
dict_key!(KEY_CONSTANT, "constant");
dict_key!(KEY_INFIX, "infix");
dict_key!(KEY_RESOLVED, "resolved");
dict_key!(KEY_USE_MODE, "use_mode");
dict_key!(KEY_STORE, "store");
dict_key!(KEY_PROJECTION, "projection");
dict_key!(KEY_CALL, "call");
dict_key!(KEY_LIFECYCLE, "lifecycle");
dict_key!(KEY_LOANS, "loans");
dict_key!(KEY_INVALIDATION, "invalidation");
dict_key!(KEY_TRY, "try");
dict_key!(KEY_EXIT, "exit");
dict_key!(KEY_SIGNATURE, "signature");
dict_key!(KEY_DECLARATION, "declaration");
dict_key!(KEY_ORPHANS, "orphans");
dict_key!(KEY_TABLES, "tables");
dict_key!(KEY_DEAD_TERM, "dead_term");
dict_key!(KEY_OUTCOME, "outcome");
dict_key!(KEY_LAYOUT, "layout");
dict_key!(KEY_CONTRACT, "contract");
dict_key!(KEY_PREFIX, "prefix");
dict_key!(KEY_SIMD_CONVERT, "simd_convert");
dict_key!(KEY_SIMD_MAKE, "simd_make");
dict_key!(KEY_CLOSURE, "closure");
dict_key!(KEY_FIELD, "field");
dict_key!(KEY_SIZE_OF, "size_of");
dict_key!(KEY_MULTI_INDEX, "multi_index");
dict_key!(KEY_SLICE, "slice");
dict_key!(KEY_SHUFFLE, "shuffle");
dict_key!(KEY_UNINIT, "uninit");
dict_key!(KEY_VARIANT, "variant");
dict_key!(KEY_ESCAPE, "escape");
dict_key!(KEY_RESUME, "resume");
dict_key!(KEY_EXIT_SITE, "exit_site");
dict_key!(KEY_SUBSCRIPT, "subscript");
dict_key!(KEY_MULTI_SET, "multi_set");
dict_key!(KEY_POINTER_STORAGE, "pointer_storage");
dict_key!(KEY_ITER_INIT, "iter_init");
dict_key!(KEY_ITER_NEXT, "iter_next");
dict_key!(
    /// The count of cleanup slots a `return` carries out.
    KEY_CLEANUP, "cleanup"
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
    FuncOp, "mojito.func", Func,
    [OneRegionInterface, SymbolOpInterface, IsolatedFromAboveInterface, NOpdsInterface<0>, NResultsInterface<0>]
);
core_op!(
    /// A variable or register slot, allocated once at function entry.
    SlotOp, "mojito.slot", Slot, []
);
core_op!(ConstOp, "mojito.const", Const, [NOpdsInterface<0>]);
core_op!(
    MaterializeOp,
    "mojito.materialize",
    Materialize,
    [NOpdsInterface<1>]
);
core_op!(BinaryOp, "mojito.binary", Binary, [NOpdsInterface<2>]);
core_op!(SimdMakeOp, "mojito.simd_make", SimdMake, []);
core_op!(UnaryOp, "mojito.unary", Unary, [NOpdsInterface<1>]);
core_op!(
    /// A lane-wise conversion or bit reinterpretation to the result type.
    SimdConvertOp, "mojito.simd_convert", SimdConvert, [NOpdsInterface<1>]
);
core_op!(UseOp, "mojito.use", Use, [NOpdsInterface<2>]);
core_op!(StoreOp, "mojito.store", Store, [NOpdsInterface<3>]);
core_op!(ProjectOp, "mojito.project", Project, []);
core_op!(LoadOp, "mojito.load", Load, [NOpdsInterface<2>]);
core_op!(RefMakeOp, "mojito.ref_make", RefMake, [NOpdsInterface<1>]);
core_op!(
    RefStoreOp,
    "mojito.ref_store",
    RefStore,
    [NOpdsInterface<3>]
);
core_op!(RefReadOp, "mojito.ref_read", RefRead, [NOpdsInterface<2>]);
core_op!(
    /// An owned lifecycle copy of a value.
    CopyOp, "mojito.copy", Copy, [NOpdsInterface<2>]
);
core_op!(MoveOp, "mojito.move", Move, [NOpdsInterface<2>]);
core_op!(
    /// An owner's live range extended to this point.
    KeepAliveOp, "mojito.keep_alive", KeepAlive, [NOpdsInterface<2>]
);
core_op!(IndexOp, "mojito.index", Index, []);
core_op!(MultiSetOp, "mojito.multi_set", MultiSet, []);
core_op!(
    /// An element taken from, or destroyed in, pointer collection storage.
    PointerStorageOp, "mojito.pointer_storage", PointerStorage, [NOpdsInterface<3>]
);
core_op!(
    IterInitOp,
    "mojito.iter_init",
    IterInit,
    [NOpdsInterface<3>]
);
core_op!(
    /// One advance of a nominal iterator: the element, and whether there
    /// was one.
    IterNextOp, "mojito.iter_next", IterNext, [NOpdsInterface<2>]
);
core_op!(CallOp, "mojito.call", Call, []);
core_op!(
    /// A closure over captured places: the lifted body and how each
    /// capture is taken.
    ClosureMakeOp, "mojito.closure_make", ClosureMake, []
);
core_op!(
    /// A field read of a value, through a reference when the value is one.
    FieldGetOp, "mojito.field_get", FieldGet, [NOpdsInterface<1>]
);
core_op!(
    /// The target-layout byte size of a type.
    SizeOfOp, "mojito.size_of", SizeOf, [NOpdsInterface<0>]
);
core_op!(
    /// A write through a reference handle.
    RefWriteOp, "mojito.ref_write", RefWrite, [NOpdsInterface<3>]
);
core_op!(
    /// A subscript read with several arguments, indices or slices.
    MultiIndexOp, "mojito.multi_index", MultiIndex, []
);
core_op!(
    /// A subscript read by one slice.
    SliceOp, "mojito.slice", Slice, []
);
core_op!(
    /// A lane selection over one vector or two.
    SimdShuffleOp, "mojito.simd_shuffle", SimdShuffle, []
);
core_op!(
    /// Whether a runtime pack held in a variable has an element left.
    PackHasNextOp, "mojito.pack_has_next", PackHasNext, [NOpdsInterface<2>]
);
core_op!(
    /// The next element of a runtime pack held in a variable, removed.
    PackNextOp, "mojito.pack_next", PackNext, [NOpdsInterface<2>]
);
core_op!(
    /// Uninitialized storage built, or its payload taken or destroyed.
    UninitStorageOp, "mojito.uninit_storage", UninitStorage, []
);
core_op!(
    /// A variant built from a payload at one alternative.
    VariantMakeOp, "mojito.variant_make", VariantMake, [NOpdsInterface<2>]
);
core_op!(
    /// Whether a variant holds one alternative.
    VariantTestOp, "mojito.variant_test", VariantTest, [NOpdsInterface<1>]
);
core_op!(
    /// A variant's payload read, or taken out.
    VariantGetOp, "mojito.variant_get", VariantGet, [NOpdsInterface<2>]
);
core_op!(
    /// A variant place written with a payload, a factory's result, or a
    /// replacement returning the old payload.
    VariantSetOp, "mojito.variant_set", VariantSet, [NOpdsInterface<3>]
);
core_op!(
    /// A variant's payload handed to a consuming handler.
    VariantDeinitWithOp, "mojito.variant_deinit_with", VariantDeinitWith, [NOpdsInterface<3>]
);
core_op!(
    /// A raising call with explicit normal and error successors.
    InvokeOp, "mojito.invoke", Invoke, [IsTerminatorInterface, NResultsInterface<0>]
);
core_op!(DropOp, "mojito.drop", Drop, [NOpdsInterface<2>]);
core_op!(
    /// A consumption of a variable, or of a projected place beside the
    /// marker register of the destructor call that took it.
    ConsumeOp, "mojito.consume", Consume, []
);
core_op!(LoansOp, "mojito.loans", Loans, []);
core_op!(
    InvalidateOp,
    "mojito.invalidate",
    Invalidate,
    [NOpdsInterface<1>]
);
core_op!(BrOp, "mojito.br", Br, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(CondBrOp, "mojito.cond_br", CondBr, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(ReturnOp, "mojito.return", Return, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(RaiseOp, "mojito.raise", Raise, [IsTerminatorInterface, NResultsInterface<0>]);
core_op!(
    /// The pending outcome a `finally` body runs under.
    OutcomeOp, "mojito.outcome", Outcome, []
);
core_op!(
    /// Dispatch on a pending outcome after its `finally` body.
    ResumeOp, "mojito.resume", Resume, [IsTerminatorInterface, NResultsInterface<0>]
);
core_op!(
    /// A structured `try`, legal only before outcome normalization.
    TryBridgeOp, "mojito.try_bridge", TryBridge, []
);
core_op!(
    /// The end of a structured region's block, legal only before outcome
    /// normalization.
    RegionExitOp, "mojito.region_exit", RegionExit, [IsTerminatorInterface, NResultsInterface<0>]
);
core_op!(
    /// A `break` or `continue` leaving a structured try region for a block
    /// of the function, with the slots it drops on the way; legal only
    /// before outcome normalization.
    EscapeOp, "mojito.escape", Escape, [IsTerminatorInterface, NResultsInterface<0>]
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
