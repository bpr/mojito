//! Verification of `mojito`: the per-operation rules the registry
//! dispatches to, the effect chain of each block, and the legality of a
//! whole module at a conversion boundary.

use std::collections::HashMap;

use pliron::attribute::Attribute;
use pliron::basic_block::BasicBlock;
use pliron::builtin::ops::ModuleOp;
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::linked_list::ContainsLinkedList;
use pliron::location::Located;
use pliron::operation::Operation;
use pliron::region::Region;
use pliron::result::Result;
use pliron::r#type::{TypeHandle, Typed};
use pliron::value::Value;
use pliron::verify_err;

use super::attrs::{
    CallAttr, CleanupAttr, ClosureAttr, ConstAttr, CoreCallKind, CoreLifecycle, CoreRole,
    CoreStepKind, CoreStorage, CoreStruct, CoreSubscriptCall, CoreUninitAccess, CoreVariantAccess,
    DeadTermAttr, EscapeAttr, ExitAttr, FieldAttr, IdentityAttr, IndexAttr, InfixAttr,
    InvalidateAttr, IterInitAttr, IterNextAttr, LifecycleAttr, LoansAttr, ModuleAttr,
    MultiIndexAttr, MultiSetAttr, OrphansAttr, OutcomeAttr, OutcomeKind, PointerStorageAttr,
    PrefixAttr, ProjectionAttr, ProvenanceAttr, RegAttr, ResolvedAttr, ResumeAttr, ShuffleAttr,
    SignatureAttr, SimdConvertAttr, SimdMakeAttr, SizeOfAttr, SliceAttr, SlotAttr, StoreAttr, Text,
    TryAttr, UninitAttr, UseModeAttr, VariantAttr,
};
use super::inventory::{CoreOpKind, EffectClass, Stage};
use super::ops::{
    KEY_CALL, KEY_CLEANUP, KEY_CLOSURE, KEY_CONSTANT, KEY_DEAD_TERM, KEY_ESCAPE, KEY_EXIT,
    KEY_FIELD, KEY_IDENTITY, KEY_INFIX, KEY_INVALIDATION, KEY_ITER_INIT, KEY_ITER_NEXT,
    KEY_LIFECYCLE, KEY_LOANS, KEY_MULTI_INDEX, KEY_MULTI_SET, KEY_ORPHANS, KEY_OUTCOME,
    KEY_POINTER_STORAGE, KEY_PREFIX, KEY_PROJECTION, KEY_PROVENANCE, KEY_REG, KEY_RESOLVED,
    KEY_RESUME, KEY_SHUFFLE, KEY_SIGNATURE, KEY_SIMD_CONVERT, KEY_SIMD_MAKE, KEY_SIZE_OF,
    KEY_SLICE, KEY_SLOT, KEY_STORE, KEY_SUBSCRIPT, KEY_TABLES, KEY_TRY, KEY_UNINIT, KEY_USE_MODE,
    KEY_VARIANT,
};
use super::types::{
    BoolType, CoreDtype, CorePointerOrigin, DtypeType, EffectType, ErrorType, Float64Type,
    FloatLiteralType, FuncType, IntLiteralType, IntType, NominalType, NoneType, OutcomeType,
    PlaceType, PointerType, RefType, SimdType, StringLiteralType, UIntType, VariantType,
};
use super::{A1Error, A1ErrorKind};

/// The builtin callees a direct call may name without a declaration, and
/// the methods of compiler-private values a method call may name without
/// a checker-selected symbol: the VM's own dispatch tables.
pub use mojito_vm::builtins::{
    BUILTIN_CALLEES, INTRINSIC_METHODS as BUILTIN_METHODS, TRAIT_DISPATCH_PREFIX,
};

/// Verify `op` against the registry rule of `kind`.
pub fn verify_core_op(ctx: &Context, op: Ptr<Operation>, kind: CoreOpKind) -> Result<()> {
    let shape = Shape::of(ctx, op);
    shape.require_attr::<IdentityAttr>(ctx, &KEY_IDENTITY, "identity")?;
    shape.require_attr::<ProvenanceAttr>(ctx, &KEY_PROVENANCE, "provenance")?;
    if kind.effect() != EffectClass::Terminator {
        shape.successors(0)?;
    }
    match kind.effect() {
        EffectClass::Pure | EffectClass::Structural => shape.forbid_effects(ctx)?,
        EffectClass::Effectful => {
            shape.effect_operand_last(ctx)?;
            shape.effect_result_last(ctx)?;
        }
        EffectClass::Terminator => {
            shape.effect_operand_last(ctx)?;
            shape.results(0)?;
        }
    }
    match kind {
        CoreOpKind::Func => verify_func(ctx, &shape),
        CoreOpKind::Slot => verify_slot(ctx, &shape),
        CoreOpKind::Const => verify_const(ctx, &shape),
        CoreOpKind::Materialize => {
            shape.operands(1)?;
            shape.value_result(ctx)?;
            shape.require(
                is_exact_literal(ctx, shape.operand_types[0]),
                "materialize reads an exact literal",
            )?;
            shape.require(
                !is_exact_literal(ctx, shape.result_types[0]),
                "materialize yields a machine scalar, not a literal",
            )
        }
        CoreOpKind::Binary => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..2)?;
            shape.value_result(ctx)?;
            shape.require_attr::<InfixAttr>(ctx, &KEY_INFIX, "infix")?;
            shape
                .require_attr::<ResolvedAttr>(ctx, &KEY_RESOLVED, "resolved")
                .map(drop)
        }
        CoreOpKind::SimdMake => verify_simd_make(ctx, &shape),
        CoreOpKind::Unary => {
            shape.operands(1)?;
            shape.value_operands(ctx, 0..1)?;
            shape.value_result(ctx)?;
            shape
                .require_attr::<PrefixAttr>(ctx, &KEY_PREFIX, "prefix")
                .map(drop)
        }
        CoreOpKind::SimdConvert => {
            shape.operands(1)?;
            shape.value_result(ctx)?;
            let declared: SimdConvertAttr =
                shape.require_attr(ctx, &KEY_SIMD_CONVERT, "conversion")?;
            shape.value_operands(ctx, 0..1)?;
            let result = shape.result_types[0];
            let vector = result
                .deref(ctx)
                .downcast_ref::<SimdType>()
                .is_some_and(|simd| simd.dtype == declared.dtype && simd.width == declared.width);
            let alias = declared.width == 1 && is_scalar_alias(ctx, result, declared.dtype);
            shape.require(
                vector || alias,
                "simd_convert yields the vector it declares, or the scalar alias of one lane",
            )
        }
        CoreOpKind::Use => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape
                .require_attr::<UseModeAttr>(ctx, &KEY_USE_MODE, "use mode")
                .map(drop)
        }
        CoreOpKind::Store => {
            shape.operands(3)?;
            shape.place_operand(ctx, 0)?;
            shape.value_operands(ctx, 1..2)?;
            shape.results(1)?;
            shape
                .require_attr::<StoreAttr>(ctx, &KEY_STORE, "store kind")
                .map(drop)
        }
        CoreOpKind::Project => verify_project(ctx, &shape),
        CoreOpKind::Load => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::RefMake => {
            shape.operands(1)?;
            shape.place_operand(ctx, 0)?;
            shape.value_result(ctx)?;
            shape.require(
                is_capability(ctx, shape.result_types[0]),
                "ref_make yields a reference capability",
            )
        }
        CoreOpKind::RefStore => {
            shape.operands(3)?;
            shape.place_operand(ctx, 0)?;
            shape.results(1)?;
            shape.require(
                is_capability(ctx, shape.operand_types[1]),
                "ref_store initializes storage with a reference capability",
            )
        }
        CoreOpKind::RefRead | CoreOpKind::Copy => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..1)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::Move => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::KeepAlive => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(1)?;
            shape.require(
                variable_slot(ctx, shape.operand_values[0]),
                "keep_alive names a variable slot",
            )
        }
        CoreOpKind::Index => verify_index(ctx, &shape),
        CoreOpKind::MultiSet => verify_multi_set(ctx, &shape),
        CoreOpKind::PointerStorage => {
            shape.operands(3)?;
            shape.value_operands(ctx, 0..2)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.require_attr::<PointerStorageAttr>(ctx, &KEY_POINTER_STORAGE, "access")?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<PointerType>(),
                "pointer_storage reads through a pointer",
            )
        }
        CoreOpKind::IterInit => {
            shape.operands(3)?;
            shape.results(1)?;
            shape.require_attr::<IterInitAttr>(ctx, &KEY_ITER_INIT, "iteration")?;
            shape.require(
                (0..2).all(|index| variable_slot(ctx, shape.operand_values[index])),
                "iter_init names the source and iterator variable slots",
            )
        }
        CoreOpKind::IterNext => {
            verify_iter_next_operands(ctx, &shape)?;
            shape.results(3)?;
            shape.value_result_with_effect(ctx)?;
            shape.require(
                shape.result_types[1].deref(ctx).is::<BoolType>(),
                "iter_next yields the element, then whether there was one",
            )
        }
        CoreOpKind::Call => {
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.successors(0)?;
            verify_call_operands(ctx, &shape).map(drop)
        }
        CoreOpKind::Invoke => verify_invoke(ctx, &shape),
        CoreOpKind::Drop => verify_lifecycle(
            ctx,
            &shape,
            &[
                CoreLifecycle::DropVar,
                CoreLifecycle::DropPlace,
                CoreLifecycle::DropReg,
            ],
        ),
        CoreOpKind::Consume => verify_lifecycle(
            ctx,
            &shape,
            &[CoreLifecycle::ConsumeVar, CoreLifecycle::ConsumePlace],
        ),
        CoreOpKind::Loans => verify_loans(ctx, &shape),
        CoreOpKind::Invalidate => {
            shape.operands(1)?;
            shape.results(2)?;
            shape.marker_result(ctx)?;
            shape
                .require_attr::<InvalidateAttr>(ctx, &KEY_INVALIDATION, "invalidation")
                .map(drop)
        }
        CoreOpKind::Br => {
            shape.successors(1)?;
            shape.forwards(ctx, 0, &shape.operand_types)
        }
        CoreOpKind::CondBr => {
            shape.operands(2)?;
            shape.successors(2)?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<BoolType>(),
                "cond_br tests a Bool",
            )?;
            let forwarded = &shape.operand_types[1..];
            shape.forwards(ctx, 0, forwarded)?;
            shape.forwards(ctx, 1, forwarded)
        }
        CoreOpKind::Return => {
            shape.successors(0)?;
            let cleanup = attr::<CleanupAttr>(ctx, op, &KEY_CLEANUP)
                .and_then(|cleanup| usize::try_from(cleanup.0).ok())
                .unwrap_or(0);
            let values = (shape.operand_types.len() - 1).checked_sub(cleanup);
            let Some(values @ (0 | 1)) = values else {
                return shape.fail(
                    ctx,
                    "return takes an optional value, its cleanup slots, and the effect token",
                );
            };
            shape.value_operands(ctx, 0..values)?;
            shape.require(
                (values..values + cleanup)
                    .all(|index| variable_slot(ctx, shape.operand_values[index])),
                "return carries variable slots as cleanup",
            )
        }
        CoreOpKind::Raise => verify_raise(ctx, &shape),
        CoreOpKind::Outcome => verify_outcome(ctx, &shape),
        CoreOpKind::Resume => verify_resume(ctx, &shape),
        CoreOpKind::TryBridge => verify_try_bridge(ctx, &shape),
        CoreOpKind::RegionExit => {
            shape.operands(1)?;
            shape.successors(0)?;
            shape.require_attr::<ExitAttr>(ctx, &KEY_EXIT, "exit kind")?;
            shape.require(
                enclosing_kind(ctx, op) == Some(CoreOpKind::TryBridge),
                "region_exit ends a block of a structured try region",
            )
        }
        CoreOpKind::ClosureMake
        | CoreOpKind::FieldGet
        | CoreOpKind::SizeOf
        | CoreOpKind::RefWrite
        | CoreOpKind::MultiIndex
        | CoreOpKind::Slice
        | CoreOpKind::SimdShuffle
        | CoreOpKind::PackHasNext
        | CoreOpKind::PackNext
        | CoreOpKind::UninitStorage
        | CoreOpKind::VariantMake
        | CoreOpKind::VariantTest
        | CoreOpKind::VariantGet
        | CoreOpKind::VariantSet
        | CoreOpKind::VariantDeinitWith
        | CoreOpKind::Escape => verify_corpus_op(ctx, op, &shape, kind),
    }
}

/// The rules of the operations the decision corpus added.
fn verify_corpus_op(
    ctx: &Context,
    op: Ptr<Operation>,
    shape: &Shape,
    kind: CoreOpKind,
) -> Result<()> {
    match kind {
        CoreOpKind::ClosureMake => {
            let closure: ClosureAttr = shape.require_attr(ctx, &KEY_CLOSURE, "closure")?;
            shape.operands(closure.modes.len() + 1)?;
            for index in 0..closure.modes.len() {
                shape.place_operand(ctx, index)?;
            }
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<FuncType>(),
                "closure_make yields a function value",
            )
        }
        CoreOpKind::FieldGet => {
            shape.operands(1)?;
            shape.value_operands(ctx, 0..1)?;
            shape.value_result(ctx)?;
            shape
                .require_attr::<FieldAttr>(ctx, &KEY_FIELD, "field")
                .map(drop)
        }
        CoreOpKind::SizeOf => {
            shape.operands(0)?;
            shape.value_result(ctx)?;
            shape.require_attr::<SizeOfAttr>(ctx, &KEY_SIZE_OF, "measured type")?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<IntType>(),
                "size_of yields an Int",
            )
        }
        CoreOpKind::RefWrite => {
            shape.operands(3)?;
            shape.value_operands(ctx, 0..2)?;
            shape.results(1)?;
            shape.require(
                is_capability(ctx, shape.operand_types[0]),
                "ref_write writes through a reference capability",
            )
        }
        CoreOpKind::MultiIndex => {
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            verify_multi_index_operands(ctx, shape)
        }
        CoreOpKind::Slice => {
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            verify_slice_operands(ctx, shape)
        }
        CoreOpKind::SimdShuffle => {
            let shuffle: ShuffleAttr = shape.require_attr(ctx, &KEY_SHUFFLE, "lane selection")?;
            shape.operands(1 + usize::from(shuffle.other))?;
            shape.value_operands(ctx, 0..shape.operand_types.len())?;
            shape.value_result(ctx)?;
            let width = shape.result_types[0]
                .deref(ctx)
                .downcast_ref::<SimdType>()
                .map(|simd| simd.width);
            shape.require(
                width == Some(shuffle.mask.len() as u64),
                "simd_shuffle yields one lane per mask entry",
            )
        }
        CoreOpKind::PackHasNext | CoreOpKind::PackNext => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.require(
                variable_slot(ctx, shape.operand_values[0]),
                "a pack step names the pack's variable slot",
            )?;
            shape.require(
                kind == CoreOpKind::PackNext || shape.result_types[0].deref(ctx).is::<BoolType>(),
                "pack_has_next yields a Bool",
            )
        }
        CoreOpKind::UninitStorage => {
            let uninit: UninitAttr = shape.require_attr(ctx, &KEY_UNINIT, "storage access")?;
            let values = match uninit.access {
                CoreUninitAccess::Make => usize::from(uninit.init),
                CoreUninitAccess::Take | CoreUninitAccess::Destroy => 1,
            };
            shape.operands(values + 1)?;
            shape.value_operands(ctx, 0..values)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.require(
                (uninit.access == CoreUninitAccess::Make) == uninit.element.is_none(),
                "taking or destroying names the element type, and building does not",
            )
        }
        CoreOpKind::VariantMake => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..1)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            let access: VariantAttr = shape.require_attr(ctx, &KEY_VARIANT, "variant access")?;
            shape.require(
                matches!(access.access, CoreVariantAccess::Make(_)),
                "variant_make constructs",
            )?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<VariantType>(),
                "variant_make yields a variant",
            )
        }
        CoreOpKind::VariantTest => {
            shape.operands(1)?;
            shape.value_operands(ctx, 0..1)?;
            shape.value_result(ctx)?;
            let access: VariantAttr = shape.require_attr(ctx, &KEY_VARIANT, "variant access")?;
            shape.require(
                matches!(access.access, CoreVariantAccess::Test(_)),
                "variant_test tests a tag",
            )?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<BoolType>(),
                "variant_test yields a Bool",
            )
        }
        CoreOpKind::VariantGet => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..1)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            let access: VariantAttr = shape.require_attr(ctx, &KEY_VARIANT, "variant access")?;
            shape.require(
                matches!(
                    access.access,
                    CoreVariantAccess::Get(_) | CoreVariantAccess::Take(..)
                ),
                "variant_get reads or takes a payload",
            )
        }
        CoreOpKind::VariantSet => {
            shape.operands(3)?;
            shape.place_operand(ctx, 0)?;
            shape.value_operands(ctx, 1..2)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            let access: VariantAttr = shape.require_attr(ctx, &KEY_VARIANT, "variant access")?;
            shape.require(
                matches!(
                    access.access,
                    CoreVariantAccess::Set(_)
                        | CoreVariantAccess::Replace(..)
                        | CoreVariantAccess::SetInitWith(_)
                ),
                "variant_set writes a place",
            )
        }
        CoreOpKind::VariantDeinitWith => {
            shape.operands(3)?;
            shape.value_operands(ctx, 0..2)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            let access: VariantAttr = shape.require_attr(ctx, &KEY_VARIANT, "variant access")?;
            shape.require(
                matches!(access.access, CoreVariantAccess::DeinitWith(_)),
                "variant_deinit_with hands the payload to a handler",
            )
        }
        CoreOpKind::Escape => {
            shape.successors(0)?;
            shape.require_attr::<EscapeAttr>(ctx, &KEY_ESCAPE, "escape target")?;
            let cleanup: CleanupAttr = shape.require_attr(ctx, &KEY_CLEANUP, "cleanup count")?;
            shape.require(
                shape.operand_types.len() as u64 == cleanup.0 + 1,
                "escape takes its cleanup slots, then the effect token",
            )?;
            shape.require(
                (0..shape.operand_values.len() - 1)
                    .all(|index| variable_slot(ctx, shape.operand_values[index])),
                "escape carries variable slots as cleanup",
            )?;
            shape.require(
                enclosing_kind(ctx, op) == Some(CoreOpKind::TryBridge),
                "escape ends a block of a structured try region",
            )
        }

        _ => shape.fail(ctx, "an operation the corpus rules do not cover"),
    }
}

/// The attribute of type `T` under `key`, cloned out of `op`.
pub fn attr<T: Attribute + Clone>(
    ctx: &Context,
    op: Ptr<Operation>,
    key: &Identifier,
) -> Option<T> {
    op.deref(ctx).attributes.get::<T>(key).cloned()
}

/// Every operation of `module` in nesting order: an operation, then the
/// operations of its regions.
pub fn walk(ctx: &Context, root: Ptr<Operation>) -> Vec<Ptr<Operation>> {
    let mut out = Vec::new();
    let mut pending = vec![root];
    while let Some(op) = pending.pop() {
        out.push(op);
        let mut nested = Vec::new();
        for region in op.deref(ctx).regions() {
            for block in region.deref(ctx).iter(ctx) {
                nested.extend(block.deref(ctx).iter(ctx));
            }
        }
        pending.extend(nested.into_iter().rev());
    }
    out
}

/// Run Pliron's verifier and the core legality inspection at `stage`.
pub fn verify_module(
    ctx: &Context,
    module: Ptr<Operation>,
    stage: Stage,
) -> std::result::Result<(), A1Error> {
    verify_tree(ctx, module)?;
    check_legality(ctx, module, stage)
}

/// Pliron's verification of `root` and everything under it, with the
/// dominance of every value's uses checked by [`verify_dominance`] in
/// place of Pliron's own walk.
pub fn verify_tree(ctx: &Context, root: Ptr<Operation>) -> std::result::Result<(), A1Error> {
    pliron::common_traits::Verify::verify(&*root.deref(ctx), ctx).map_err(|error| {
        A1Error::new(
            A1ErrorKind::Verification,
            pliron::printable::Printable::disp(&error, ctx).to_string(),
        )
    })?;
    verify_dominance(ctx, root)
}

/// Every use of a value under `root` is dominated by the value's
/// definition, as Pliron's own check demands.
///
/// The one difference: a block nothing reaches is dead code, and the uses
/// in it are not judged. The pinned Pliron panics on such a block, which
/// normalization leaves behind a `try` whose body cannot raise: its error
/// path.
pub fn verify_dominance(ctx: &Context, root: Ptr<Operation>) -> std::result::Result<(), A1Error> {
    let mut regions: HashMap<Ptr<Region>, Dominance> = HashMap::new();
    for op in walk(ctx, root) {
        let results: Vec<Value> = op.deref(ctx).results().collect();
        for value in results {
            check_uses(ctx, value, &mut regions)?;
        }
        let blocks: Vec<Ptr<BasicBlock>> = op
            .deref(ctx)
            .regions()
            .flat_map(|region| region.deref(ctx).iter(ctx).collect::<Vec<_>>())
            .collect();
        for block in blocks {
            let arguments: Vec<Value> = block.deref(ctx).arguments().collect();
            for value in arguments {
                check_uses(ctx, value, &mut regions)?;
            }
        }
    }
    Ok(())
}

/// The reachable blocks of one region and their dominators.
struct Dominance {
    reachable: Vec<Ptr<BasicBlock>>,
    dominators: HashMap<Ptr<BasicBlock>, Vec<Ptr<BasicBlock>>>,
}

impl Dominance {
    fn of(ctx: &Context, region: Ptr<Region>) -> Self {
        let successors = |block: Ptr<BasicBlock>| -> Vec<Ptr<BasicBlock>> {
            block
                .deref(ctx)
                .get_tail()
                .map(|term| term.deref(ctx).successors().collect())
                .unwrap_or_default()
        };
        let mut reachable = Vec::new();
        let mut pending: Vec<Ptr<BasicBlock>> =
            region.deref(ctx).get_entry_block().into_iter().collect();
        while let Some(block) = pending.pop() {
            if reachable.contains(&block) {
                continue;
            }
            reachable.push(block);
            pending.extend(successors(block));
        }
        let mut predecessors: HashMap<Ptr<BasicBlock>, Vec<Ptr<BasicBlock>>> = HashMap::new();
        for block in &reachable {
            for successor in successors(*block) {
                predecessors.entry(successor).or_default().push(*block);
            }
        }
        let mut dominators: HashMap<Ptr<BasicBlock>, Vec<Ptr<BasicBlock>>> = reachable
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let all = if index == 0 {
                    vec![*block]
                } else {
                    reachable.clone()
                };
                (*block, all)
            })
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for block in reachable.iter().skip(1) {
                let mut next: Vec<Ptr<BasicBlock>> = reachable
                    .iter()
                    .filter(|candidate| {
                        predecessors.get(block).is_some_and(|preds| {
                            preds
                                .iter()
                                .all(|pred| dominators[pred].contains(candidate))
                        })
                    })
                    .copied()
                    .collect();
                if !next.contains(block) {
                    next.push(*block);
                }
                if dominators[block] != next {
                    dominators.insert(*block, next);
                    changed = true;
                }
            }
        }
        Self {
            reachable,
            dominators,
        }
    }

    fn dominates(&self, dominator: Ptr<BasicBlock>, block: Ptr<BasicBlock>) -> bool {
        self.dominators
            .get(&block)
            .is_some_and(|dominators| dominators.contains(&dominator))
    }
}

fn check_uses(
    ctx: &Context,
    value: Value,
    regions: &mut HashMap<Ptr<Region>, Dominance>,
) -> std::result::Result<(), A1Error> {
    let undominated = |user: Ptr<Operation>| {
        A1Error::new(
            A1ErrorKind::Verification,
            format!(
                "a use is not dominated by its definition ({})",
                describe(ctx, user)
            ),
        )
    };
    let (definition, defining_op) = match value.defining_entity() {
        pliron::value::DefiningEntity::Op(op) => (op.deref(ctx).get_parent_block(), Some(op)),
        pliron::value::DefiningEntity::Block(block) => (Some(block), None),
    };
    let Some(definition) = definition else {
        return Err(A1Error::new(
            A1ErrorKind::Verification,
            "a value defined by an operation outside every block",
        ));
    };
    let Some(region) = definition.deref(ctx).get_parent_region() else {
        return Err(A1Error::new(
            A1ErrorKind::Verification,
            "a value defined in a block outside every region",
        ));
    };
    let dominance = regions
        .entry(region)
        .or_insert_with(|| Dominance::of(ctx, region));
    for r#use in value.uses(ctx) {
        let user = r#use.user_op();
        let Some(ancestor) = ancestor_in_region(ctx, user, region) else {
            return Err(undominated(user));
        };
        let Some(block) = ancestor.deref(ctx).get_parent_block() else {
            return Err(undominated(user));
        };
        if !dominance.reachable.contains(&block) {
            continue;
        }
        let dominated = if block == definition {
            defining_op.is_none_or(|definer| precedes(ctx, block, definer, ancestor))
        } else {
            dominance.dominates(definition, block)
        };
        if !dominated {
            return Err(undominated(user));
        }
    }
    Ok(())
}

/// The operation of `region` that holds `op`, or `op` itself when it is
/// directly in `region`.
fn ancestor_in_region(
    ctx: &Context,
    mut op: Ptr<Operation>,
    region: Ptr<Region>,
) -> Option<Ptr<Operation>> {
    loop {
        let block = op.deref(ctx).get_parent_block()?;
        if block.deref(ctx).get_parent_region() == Some(region) {
            return Some(op);
        }
        op = block.deref(ctx).get_parent_op(ctx)?;
    }
}

/// Whether `first` comes strictly before `second` in `block`.
fn precedes(
    ctx: &Context,
    block: Ptr<BasicBlock>,
    first: Ptr<Operation>,
    second: Ptr<Operation>,
) -> bool {
    for op in block.deref(ctx).iter(ctx) {
        if op == second {
            return false;
        }
        if op == first {
            return true;
        }
    }
    false
}

/// The legality of `module` at `stage`: only the approved container and
/// registered operations legal there remain, every type is a core type,
/// and every symbol resolves.
pub fn check_legality(
    ctx: &Context,
    module: Ptr<Operation>,
    stage: Stage,
) -> std::result::Result<(), A1Error> {
    match legality_violations(ctx, module, stage).into_iter().next() {
        Some(violation) => Err(violation),
        None => Ok(()),
    }
}

/// Every violation of `module`'s legality at `stage`, in operation order.
pub fn legality_violations(ctx: &Context, module: Ptr<Operation>, stage: Stage) -> Vec<A1Error> {
    let illegal = |message: String| A1Error::new(A1ErrorKind::Legality, message);
    if !Operation::is_op::<ModuleOp>(module, ctx) {
        return vec![illegal("the container is not `builtin.module`".into())];
    }
    let Some(tables) = attr::<ModuleAttr>(ctx, module, &KEY_TABLES) else {
        return vec![illegal("the module carries no tables".into())];
    };
    let mut violations = Vec::new();
    if tables.schema.as_str() != super::attrs::SCHEMA {
        violations.push(illegal(format!(
            "schema `{}` is not `{}`",
            tables.schema,
            super::attrs::SCHEMA
        )));
    }
    let operations = walk(ctx, module);
    let mut symbols = Vec::new();
    for op in operations.iter().skip(1) {
        let context = describe(ctx, *op);
        let operation = op.deref(ctx);
        let block_arguments = operation.regions().flat_map(|region| {
            region
                .deref(ctx)
                .iter(ctx)
                .flat_map(|block| block.deref(ctx).arguments().collect::<Vec<_>>())
                .collect::<Vec<_>>()
        });
        let foreign = operation
            .results()
            .chain(operation.operands())
            .chain(block_arguments)
            .any(|value| !is_core_type(ctx, value.get_type(ctx)));
        if foreign {
            violations.push(illegal(format!(
                "a value outside the core types ({context})"
            )));
        }
        let Some(kind) = CoreOpKind::of(ctx, *op) else {
            violations.push(illegal(format!(
                "operation `{}` is not registered in `mojito`",
                Operation::get_opid(*op, ctx)
            )));
            continue;
        };
        if !kind.legal_at(stage) {
            violations.push(illegal(format!(
                "`mojito.{}` is illegal at {stage:?} ({context})",
                kind.name()
            )));
        }
        if kind == CoreOpKind::Func {
            if operation.get_parent_op(ctx) != Some(module) {
                violations.push(illegal(format!("a nested function ({context})")));
            }
            if let Some(signature) = attr::<SignatureAttr>(ctx, *op, &KEY_SIGNATURE) {
                symbols.push(signature.symbol);
            }
        }
    }
    for entry in &tables.entries {
        if !symbols.contains(&entry.concrete) {
            violations.push(illegal(format!(
                "entry `{}` names no function of the module",
                entry.concrete
            )));
        }
    }
    for op in operations.iter().skip(1) {
        if let Err(violation) = check_projection(ctx, *op, &tables) {
            violations.push(violation);
        }
        if stage == Stage::ExecutableCore && CoreOpKind::of(ctx, *op) == Some(CoreOpKind::Func) {
            let checked = super::outcomes::FnPlan::build(ctx, *op)
                .and_then(|_| super::lifecycle::check_contract(ctx, *op));
            if let Err(violation) = checked {
                violations.push(violation);
            }
        }
        let Some(call) = attr::<CallAttr>(ctx, *op, &KEY_CALL) else {
            continue;
        };
        if !call_target_known(&call, &symbols, &tables.structs) {
            violations.push(illegal(format!(
                "call target `{}` resolves to no function, struct, or builtin ({})",
                call_target(&call),
                describe(ctx, *op)
            )));
        }
    }
    violations
}

/// The symbol a call resolves to: its checker-selected symbol, else the
/// name it spells.
pub fn call_target(call: &CallAttr) -> &str {
    call.resolved.as_ref().unwrap_or(&call.target).as_str()
}

/// Whether a call's target is a function of the module (`symbols`), a
/// declared struct, or a builtin the VM answers itself.
pub fn call_target_known(call: &CallAttr, symbols: &[Text], structs: &[CoreStruct]) -> bool {
    let target = call_target(call);
    // An indirect call's target is the value it calls; a resolved symbol
    // on it is the checker's abstract `__call__` target, not a declaration.
    if call.kind == CoreCallKind::Indirect {
        return true;
    }
    if call.kind == CoreCallKind::Method && call.resolved.is_none() {
        return BUILTIN_METHODS.contains(&target);
    }
    symbols.iter().any(|symbol| symbol.as_str() == target)
        || structs.iter().any(|decl| decl.name.as_str() == target)
        || BUILTIN_CALLEES.contains(&target)
        || target.starts_with(TRAIT_DISPATCH_PREFIX)
}

/// Each field step of a projection names a field its nominal base
/// declares, at the type the declaration gives it.
pub fn check_projection(
    ctx: &Context,
    op: Ptr<Operation>,
    tables: &ModuleAttr,
) -> std::result::Result<(), A1Error> {
    let Some(projection) = attr::<ProjectionAttr>(ctx, op, &KEY_PROJECTION) else {
        return Ok(());
    };
    let Some(mut base) = projection.root_ty else {
        return Ok(());
    };
    for step in &projection.steps {
        if let CoreStepKind::Field(field) = &step.kind {
            let mut object = base;
            while let Some(reference) = object.deref(ctx).downcast_ref::<RefType>() {
                object = reference.referent;
            }
            let nominal = object
                .deref(ctx)
                .downcast_ref::<NominalType>()
                .map(|nominal| nominal.name.clone());
            let declared = nominal.as_ref().and_then(|name| {
                tables
                    .structs
                    .iter()
                    .find(|decl| decl.name == *name)
                    .and_then(|decl| decl.fields.iter().find(|entry| entry.name == *field))
            });
            if !declared.is_some_and(|entry| same_layout(ctx, entry.ty, step.ty)) {
                return Err(A1Error::new(
                    A1ErrorKind::Legality,
                    format!(
                        "projection step `{field}` names no field of its base at that type ({})",
                        describe(ctx, op)
                    ),
                ));
            }
        }
        base = step.ty;
    }
    Ok(())
}

/// Whether two core types spell one layout: equal, or equal once every
/// origin is erased, since specialization leaves origins symbolic and a
/// shared instance may spell them two ways.
fn same_layout(ctx: &Context, declared: TypeHandle, found: TypeHandle) -> bool {
    if declared == found {
        return true;
    }
    match (
        super::types::export_type(ctx, declared),
        super::types::export_type(ctx, found),
    ) {
        (Ok(declared), Ok(found)) => layout(&declared) == layout(&found),
        _ => false,
    }
}

/// `ty` with every origin erased: the arguments of its nominal types,
/// and the origins of its pointers and references.
fn layout(ty: &mojito_types::types::Ty) -> mojito_types::types::Ty {
    use mojito_types::origin::{Origin, PointerOrigin};
    use mojito_types::types::Ty;
    let erased = mojito_types::types::erase_origin_arguments(ty);
    match erased {
        Ty::Struct(name, arguments) => Ty::Struct(
            name,
            arguments
                .into_iter()
                .map(|argument| match argument {
                    mojito_types::types::TyArg::Ty(inner) => {
                        mojito_types::types::TyArg::Ty(layout(&inner))
                    }
                    other => other,
                })
                .collect(),
        ),
        Ty::Tuple(elements) => Ty::Tuple(elements.iter().map(layout).collect()),
        Ty::RuntimePack(elements) => Ty::RuntimePack(elements.iter().map(layout).collect()),
        Ty::Variant(alternatives) => Ty::Variant(alternatives.iter().map(layout).collect()),
        Ty::Pointer { element, .. } => Ty::Pointer {
            element: Box::new(layout(&element)),
            origin: PointerOrigin::Untracked { mutable: false },
        },
        Ty::Ref(mut reference) => {
            reference.referent = Box::new(layout(&reference.referent));
            reference.origin = Origin::Unbound;
            Ty::Ref(reference)
        }
        other => other,
    }
}

/// Whether `ty` belongs to the core vocabulary.
pub fn is_core_type(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    if let Some(place) = object.downcast_ref::<PlaceType>() {
        return is_core_type(ctx, place.target);
    }
    object.is::<EffectType>() || object.is::<OutcomeType>() || is_value_type(ctx, ty)
}

/// Whether `ty` is the type of a runtime or literal value.
pub fn is_value_type(ctx: &Context, ty: TypeHandle) -> bool {
    super::types::export_type(ctx, ty).is_ok()
}

/// The operation's stable identity, for diagnostics.
pub fn describe(ctx: &Context, op: Ptr<Operation>) -> String {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY).map_or_else(
        || format!("`{}` without identity", Operation::get_opid(op, ctx)),
        |identity| format!("`{}` at {}", Operation::get_opid(op, ctx), identity.key()),
    )
}

/// The registry kind of the operation whose region holds `op`.
pub fn enclosing_kind(ctx: &Context, op: Ptr<Operation>) -> Option<CoreOpKind> {
    op.deref(ctx)
        .get_parent_op(ctx)
        .and_then(|parent| CoreOpKind::of(ctx, parent))
}

#[derive(Debug)]
struct CoreVerifyError(String);

impl std::fmt::Display for CoreVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CoreVerifyError {}

/// The operand, result, successor, and region shape of one operation.
struct Shape {
    op: Ptr<Operation>,
    context: String,
    operand_types: Vec<TypeHandle>,
    operand_values: Vec<Value>,
    result_types: Vec<TypeHandle>,
    successors: Vec<Ptr<BasicBlock>>,
    regions: Vec<Ptr<Region>>,
}

impl Shape {
    fn of(ctx: &Context, op: Ptr<Operation>) -> Self {
        let operation = op.deref(ctx);
        Self {
            op,
            context: describe(ctx, op),
            operand_types: operation
                .operands()
                .map(|value| value.get_type(ctx))
                .collect(),
            operand_values: operation.operands().collect(),
            result_types: operation.result_types().collect(),
            successors: operation.successors().collect(),
            regions: operation.regions().collect(),
        }
    }

    fn fail<T>(&self, ctx: &Context, message: impl std::fmt::Display) -> Result<T> {
        verify_err!(
            self.op.deref(ctx).loc(),
            CoreVerifyError(format!("{message} ({})", self.context))
        )
    }

    fn require(&self, holds: bool, message: &str) -> Result<()> {
        if holds {
            return Ok(());
        }
        verify_err!(
            pliron::location::Location::Unknown,
            CoreVerifyError(format!("{message} ({})", self.context))
        )
    }

    fn require_attr<T: Attribute + Clone>(
        &self,
        ctx: &Context,
        key: &Identifier,
        what: &str,
    ) -> Result<T> {
        match attr::<T>(ctx, self.op, key) {
            Some(value) => Ok(value),
            None => self.fail(ctx, format!("missing or mistyped {what} attribute")),
        }
    }

    fn operands(&self, count: usize) -> Result<()> {
        self.require(
            self.operand_types.len() == count,
            &format!(
                "expected {count} operands, found {}",
                self.operand_types.len()
            ),
        )
    }

    fn results(&self, count: usize) -> Result<()> {
        self.require(
            self.result_types.len() == count,
            &format!(
                "expected {count} results, found {}",
                self.result_types.len()
            ),
        )
    }

    fn successors(&self, count: usize) -> Result<()> {
        self.require(
            self.successors.len() == count,
            &format!(
                "expected {count} successors, found {}",
                self.successors.len()
            ),
        )
    }

    fn forbid_effects(&self, ctx: &Context) -> Result<()> {
        let effect = |ty: &TypeHandle| ty.deref(ctx).is::<EffectType>();
        self.require(
            !self.operand_types.iter().any(effect) && !self.result_types.iter().any(effect),
            "an operation outside the effect chain touches the effect token",
        )
    }

    fn effect_operand_last(&self, ctx: &Context) -> Result<()> {
        let effects = self
            .operand_types
            .iter()
            .filter(|ty| ty.deref(ctx).is::<EffectType>())
            .count();
        let last = self
            .operand_types
            .last()
            .is_some_and(|ty| ty.deref(ctx).is::<EffectType>());
        self.require(
            effects == 1 && last,
            "the effect token is the one, last operand",
        )
    }

    fn effect_result_last(&self, ctx: &Context) -> Result<()> {
        let effects = self
            .result_types
            .iter()
            .filter(|ty| ty.deref(ctx).is::<EffectType>())
            .count();
        let last = self
            .result_types
            .last()
            .is_some_and(|ty| ty.deref(ctx).is::<EffectType>());
        self.require(
            effects == 1 && last,
            "the effect token is the one, last result",
        )
    }

    fn place_operand(&self, ctx: &Context, index: usize) -> Result<()> {
        self.require(
            self.operand_types
                .get(index)
                .is_some_and(|ty| ty.deref(ctx).is::<PlaceType>()),
            &format!("operand {index} is a place"),
        )
    }

    fn value_operands(&self, ctx: &Context, range: std::ops::Range<usize>) -> Result<()> {
        for index in range {
            self.require(
                self.operand_types
                    .get(index)
                    .is_some_and(|ty| is_value_type(ctx, *ty)),
                &format!("operand {index} is a value"),
            )?;
        }
        Ok(())
    }

    /// One value result, carrying its register.
    fn value_result(&self, ctx: &Context) -> Result<()> {
        self.results(1)?;
        self.require(
            is_value_type(ctx, self.result_types[0]),
            "the result is a value",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// A value result carrying its register, then the effect token.
    fn value_result_with_effect(&self, ctx: &Context) -> Result<()> {
        self.require(
            self.result_types
                .first()
                .is_some_and(|ty| is_value_type(ctx, *ty)),
            "the first result is a value",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// A `None` marker result carrying its register, then the effect token.
    fn marker_result(&self, ctx: &Context) -> Result<()> {
        self.require(
            self.result_types
                .first()
                .is_some_and(|ty| ty.deref(ctx).is::<NoneType>()),
            "the marker result is None",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// Successor `index` takes exactly arguments of `types`.
    fn forwards(&self, ctx: &Context, index: usize, types: &[TypeHandle]) -> Result<()> {
        let arguments: Vec<TypeHandle> = self.successors[index]
            .deref(ctx)
            .arguments()
            .map(|value| value.get_type(ctx))
            .collect();
        self.require(
            arguments == types,
            &format!("successor {index} takes the forwarded operands, by type"),
        )
    }
}

fn verify_func(ctx: &Context, shape: &Shape) -> Result<()> {
    let signature: SignatureAttr = shape.require_attr(ctx, &KEY_SIGNATURE, "signature")?;
    shape.require_attr::<OrphansAttr>(ctx, &KEY_ORPHANS, "orphan register")?;
    shape.require(shape.regions.len() == 1, "a function has one region")?;
    let Some(entry) = shape.regions[0].deref(ctx).get_entry_block() else {
        return shape.fail(ctx, "a function has an entry block");
    };
    let arguments: Vec<TypeHandle> = entry
        .deref(ctx)
        .arguments()
        .map(|value| value.get_type(ctx))
        .collect();
    let Some((effect, parameters)) = arguments.split_last() else {
        return shape.fail(ctx, "the entry block takes the effect token");
    };
    shape.require(
        effect.deref(ctx).is::<EffectType>() && parameters == signature.params.as_slice(),
        "the entry block takes the signature's parameters, then the effect token",
    )?;
    for region in &shape.regions {
        verify_effect_chains(ctx, shape, *region)?;
    }
    Ok(())
}

/// Every block of `region`, and of the regions nested in it, threads one
/// effect chain from its last argument to its terminator.
fn verify_effect_chains(ctx: &Context, shape: &Shape, region: Ptr<Region>) -> Result<()> {
    for block in region.deref(ctx).iter(ctx) {
        let Some(mut token) = block.deref(ctx).arguments().last() else {
            return shape.fail(ctx, "a block takes no effect token");
        };
        if !token.get_type(ctx).deref(ctx).is::<EffectType>() {
            return shape.fail(ctx, "a block's last argument is not the effect token");
        }
        for op in block.deref(ctx).iter(ctx) {
            let Some(kind) = CoreOpKind::of(ctx, op) else {
                continue;
            };
            for nested in op.deref(ctx).regions() {
                verify_effect_chains(ctx, shape, nested)?;
            }
            if matches!(kind.effect(), EffectClass::Pure | EffectClass::Structural) {
                continue;
            }
            let consumed = op.deref(ctx).operands().last();
            if consumed != Some(token) {
                return shape.fail(
                    ctx,
                    format!(
                        "{} does not consume the current effect token",
                        describe(ctx, op)
                    ),
                );
            }
            if token.num_uses(ctx) != 1 {
                return shape.fail(
                    ctx,
                    format!(
                        "the effect token {} consumes is used {} times",
                        describe(ctx, op),
                        token.num_uses(ctx)
                    ),
                );
            }
            if kind.effect() == EffectClass::Effectful {
                let Some(next) = op.deref(ctx).results().last() else {
                    return shape.fail(ctx, "an effectful operation yields no token");
                };
                token = next;
            }
        }
    }
    Ok(())
}

fn verify_slot(ctx: &Context, shape: &Shape) -> Result<()> {
    let slot: SlotAttr = shape.require_attr(ctx, &KEY_SLOT, "slot")?;
    shape.results(1)?;
    shape.require(
        shape.result_types[0].deref(ctx).is::<PlaceType>(),
        "a slot is a place",
    )?;
    shape.require(
        enclosing_kind(ctx, shape.op) == Some(CoreOpKind::Func),
        "a slot is allocated in the function's own region",
    )?;
    let in_entry = shape.op.deref(ctx).get_parent_block().is_some_and(|block| {
        block
            .deref(ctx)
            .get_parent_region()
            .and_then(|region| region.deref(ctx).get_entry_block())
            == Some(block)
    });
    shape.require(in_entry, "a slot is allocated once, at function entry")?;
    match slot.param {
        Some(position) => {
            shape.operands(1)?;
            shape.require(
                slot.storage == CoreStorage::Variable,
                "only a variable slot binds a parameter",
            )?;
            let argument = shape.operand_values[0];
            let bound = argument.defining_block().is_some()
                && argument.try_find_index(ctx).ok() == usize::try_from(position).ok();
            shape.require(
                bound,
                "a parameter slot binds the entry argument of its position",
            )
        }
        None => shape.operands(0),
    }
}

fn verify_const(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(0)?;
    shape.value_result(ctx)?;
    let constant: ConstAttr = shape.require_attr(ctx, &KEY_CONSTANT, "constant")?;
    let result = shape.result_types[0].deref(ctx);
    let agrees = match &constant {
        ConstAttr::Int(_) => result.is::<IntType>(),
        ConstAttr::IntLiteral(digits) => {
            result.is::<IntLiteralType>()
                && mojito_common::literal::IntLiteral::parse_radix(digits.as_str(), 10).is_some()
        }
        ConstAttr::Float(_) => result.is::<Float64Type>(),
        ConstAttr::FloatLiteral(text) => {
            result.is::<FloatLiteralType>()
                && mojito_common::literal::FloatLiteral::parse_exact(text.as_str()).is_some()
        }
        ConstAttr::Bool(_) => result.is::<BoolType>(),
        ConstAttr::Str(_) => result.is::<StringLiteralType>(),
        ConstAttr::None => result.is::<NoneType>(),
        ConstAttr::Dtype(_) => result.is::<DtypeType>(),
        ConstAttr::Function(_) => result.is::<FuncType>(),
    };
    shape.require(agrees, "the constant agrees with its result type")
}

fn verify_simd_make(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.value_result(ctx)?;
    shape.value_operands(ctx, 0..shape.operand_types.len())?;
    let declared: SimdMakeAttr = shape.require_attr(ctx, &KEY_SIMD_MAKE, "SIMD shape")?;
    let width = declared.width;
    let result = shape.result_types[0];
    let vector = result
        .deref(ctx)
        .downcast_ref::<SimdType>()
        .is_some_and(|simd| simd.dtype == declared.dtype && simd.width == width);
    let alias = width == 1 && is_scalar_alias(ctx, result, declared.dtype);
    shape.require(
        vector || alias,
        "simd_make yields the vector it declares, or the scalar alias of one lane",
    )?;
    let lanes = shape.operand_types.len() as u64;
    shape.require(
        lanes == width || lanes == 1,
        "simd_make takes one lane per width, or one to splat",
    )
}

fn verify_project(ctx: &Context, shape: &Shape) -> Result<()> {
    let projection: ProjectionAttr = shape.require_attr(ctx, &KEY_PROJECTION, "projection")?;
    shape.results(1)?;
    shape.place_operand(ctx, 0)?;
    let indices = projection
        .steps
        .iter()
        .filter(|step| step.kind == CoreStepKind::Index)
        .count();
    shape.require(
        shape.operand_types.len() == indices + 1,
        "a projection takes its root and one operand per dynamic index",
    )?;
    shape.value_operands(ctx, 1..shape.operand_types.len())?;
    let Some(target) = shape.result_types[0]
        .deref(ctx)
        .downcast_ref::<PlaceType>()
        .map(|place| place.target)
    else {
        return shape.fail(ctx, "a projection yields a place");
    };
    let designated = projection
        .steps
        .last()
        .map(|step| step.ty)
        .or(projection.root_ty);
    let through = designated.and_then(|ty| {
        ty.deref(ctx)
            .downcast_ref::<RefType>()
            .map(|ty| ty.referent)
    });
    shape.require(
        projection.ty == Some(target) && [designated, through].contains(&Some(target)),
        "a projection yields the place its last step designates, or that reference's referent",
    )
}

/// The call facts and the operand segments they declare; the last operand
/// is the effect token.
fn verify_call_operands(ctx: &Context, shape: &Shape) -> Result<CallAttr> {
    let call: CallAttr = shape.require_attr(ctx, &KEY_CALL, "call facts")?;
    let segments = call.segments();
    let total = segments
        .iter()
        .try_fold(0usize, |total, segment| total.checked_add(*segment));
    let Some(total) = total.filter(|total| shape.operand_types.len() == total + 1) else {
        return shape.fail(
            ctx,
            "a call's operands fill its declared segments, then the effect token",
        );
    };
    let aligned =
        |places: &[bool], arguments: usize| places.is_empty() || places.len() == arguments;
    shape.require(
        aligned(&call.arg_places, segments[1]) && aligned(&call.kwarg_places, segments[2]),
        "a call records one place flag per argument, or no place table",
    )?;
    let values = segments[0] + segments[1] + segments[2] + segments[3];
    shape.value_operands(ctx, 0..values)?;
    for index in values..total {
        shape.place_operand(ctx, index)?;
    }
    shape.require(
        call.kind == CoreCallKind::Indirect || !call.target.as_str().is_empty(),
        "a call names its target",
    )?;
    Ok(call)
}

/// The operand layout of a subscript: `values` value operands, the reified
/// compile-time arguments of `call`, then `places` retained places.
fn verify_subscript_operands(
    ctx: &Context,
    shape: &Shape,
    values: usize,
    call: Option<&CoreSubscriptCall>,
    places: usize,
) -> Result<()> {
    let values = values + call.map_or(0, CoreSubscriptCall::reified);
    shape.require(
        shape.operand_types.len() == values + places + 1,
        "a subscript's operands are its values, its retained places, then the effect token",
    )?;
    shape.value_operands(ctx, 0..values)?;
    for index in values..values + places {
        shape.place_operand(ctx, index)?;
    }
    shape.require(
        call.is_none_or(|call| !call.target.as_str().is_empty()),
        "a subscript call names its target",
    )
}

fn verify_index(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.results(2)?;
    shape.value_result_with_effect(ctx)?;
    verify_index_operands(ctx, shape)
}

fn verify_index_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let index: IndexAttr = shape.require_attr(ctx, &KEY_SUBSCRIPT, "subscript")?;
    shape.require(
        index.call.is_none() || index.intrinsic.is_none(),
        "an index dispatches to a call or to a storage family, not both",
    )?;
    let places = usize::from(index.base_place) + usize::from(index.index_place);
    verify_subscript_operands(ctx, shape, 2, index.call.as_ref(), places)
}

fn verify_multi_index_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let subscript: MultiIndexAttr = shape.require_attr(ctx, &KEY_MULTI_INDEX, "subscript")?;
    shape.require(
        subscript.arg_places.is_empty() || subscript.arg_places.len() == subscript.args.len(),
        "a multi_index records one place flag per argument, or no place table",
    )?;
    shape.require(
        subscript.kwarg_places.is_empty() || subscript.kwarg_places.len() == subscript.kwargs.len(),
        "a multi_index records one place flag per keyword argument, or no place table",
    )?;
    verify_subscript_operands(
        ctx,
        shape,
        subscript.values(),
        subscript.call.as_ref(),
        subscript.places(),
    )
}

fn verify_slice_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let slice: SliceAttr = shape.require_attr(ctx, &KEY_SLICE, "slice")?;
    shape.require(
        slice.call.is_none() || slice.intrinsic.is_none(),
        "a slice dispatches to a call or to a storage family, not both",
    )?;
    verify_subscript_operands(
        ctx,
        shape,
        slice.values(),
        slice.call.as_ref(),
        slice.places(),
    )
}

fn verify_multi_set(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.results(1)?;
    verify_multi_set_operands(ctx, shape)
}

fn verify_multi_set_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let store: MultiSetAttr = shape.require_attr(ctx, &KEY_MULTI_SET, "subscript store")?;
    let places = usize::from(store.receiver_place)
        + store.arg_places.iter().filter(|place| **place).count()
        + usize::from(store.value_place);
    let values = store.arg_places.len() + 2;
    verify_subscript_operands(ctx, shape, values, Some(&store.call), places)
}

fn verify_invoke(ctx: &Context, shape: &Shape) -> Result<()> {
    let Some(invoked) = invoked_kind(ctx, shape.op) else {
        return shape.fail(ctx, "an invoke carries the facts of the call it makes");
    };
    match invoked {
        CoreOpKind::Index => verify_index_operands(ctx, shape)?,
        CoreOpKind::MultiSet => verify_multi_set_operands(ctx, shape)?,
        CoreOpKind::IterNext => verify_iter_next_operands(ctx, shape)?,
        CoreOpKind::MultiIndex => verify_multi_index_operands(ctx, shape)?,
        CoreOpKind::Slice => verify_slice_operands(ctx, shape)?,
        _ => verify_call_operands(ctx, shape).map(drop)?,
    }
    shape.successors(2)?;
    let Some(error) = raised_by(ctx, shape.op) else {
        return shape.fail(ctx, "an invoke names the error type its callee raises");
    };
    let effect: TypeHandle = EffectType::get(ctx).into();
    let normal: Vec<TypeHandle> = shape.successors[0]
        .deref(ctx)
        .arguments()
        .map(|value| value.get_type(ctx))
        .collect();
    if invoked == CoreOpKind::MultiSet {
        shape.require(
            normal == [effect],
            "the normal successor of a store takes the effect token",
        )?;
    } else if invoked == CoreOpKind::IterNext {
        shape.require_attr::<RegAttr>(ctx, &KEY_REG, "register")?;
        shape.require(
            normal.len() == 3
                && is_value_type(ctx, normal[0])
                && normal[1].deref(ctx).is::<BoolType>()
                && normal[2] == effect,
            "the normal successor takes the element, whether there was one, and the effect token",
        )?;
    } else {
        shape.require_attr::<RegAttr>(ctx, &KEY_REG, "register")?;
        shape.require(
            normal.len() == 2 && is_value_type(ctx, normal[0]) && normal[1] == effect,
            "the normal successor takes the result and the effect token",
        )?;
    }
    shape.forwards(ctx, 1, &[error, effect])
}

/// The operands of an iterator advance: the iterator's variable slot and
/// the effect token, under the iterator call's facts.
fn verify_iter_next_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(2)?;
    shape.require_attr::<IterNextAttr>(ctx, &KEY_ITER_NEXT, "iterator call")?;
    shape.require(
        variable_slot(ctx, shape.operand_values[0]),
        "iter_next names the iterator variable slot",
    )
}

fn verify_lifecycle(ctx: &Context, shape: &Shape, kinds: &[CoreLifecycle]) -> Result<()> {
    let lifecycle: LifecycleAttr = shape.require_attr(ctx, &KEY_LIFECYCLE, "lifecycle")?;
    shape.require(
        kinds.contains(&lifecycle.kind),
        "the lifecycle kind belongs to this operation",
    )?;
    shape.results(1)?;
    if lifecycle.kind == CoreLifecycle::ConsumePlace {
        // The place, the marker register of the destructor call, the token.
        shape.operands(3)?;
        shape.value_operands(ctx, 1..2)?;
    } else {
        shape.operands(2)?;
    }
    if lifecycle.kind == CoreLifecycle::DropReg {
        return shape.value_operands(ctx, 0..1);
    }
    shape.place_operand(ctx, 0)?;
    let owner = root_slot(ctx, shape.operand_values[0]);
    shape.require(
        owner.is_some_and(|slot| {
            slot.storage == CoreStorage::Variable && slot.id == lifecycle.owner
        }),
        "the lifecycle event names the owner of the place it destroys",
    )
}

fn verify_loans(ctx: &Context, shape: &Shape) -> Result<()> {
    let loans: LoansAttr = shape.require_attr(ctx, &KEY_LOANS, "loan set")?;
    shape.results(2)?;
    shape.marker_result(ctx)?;
    shape.require(
        shape.operand_types.len() == loans.loans.len() + 2,
        "a loan generation takes its holder, one place per loan, and the effect token",
    )?;
    for index in 0..=loans.loans.len() {
        shape.place_operand(ctx, index)?;
    }
    Ok(())
}

fn verify_raise(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(2)?;
    shape.value_operands(ctx, 0..1)?;
    let dead = attr::<DeadTermAttr>(ctx, shape.op, &KEY_DEAD_TERM);
    shape.require(dead.is_some(), "a raise records the terminator it cuts off")?;
    match shape.successors.len() {
        0 => Ok(()),
        1 => shape.forwards(ctx, 0, &shape.operand_types),
        _ => shape.fail(ctx, "a raise has at most one error successor"),
    }
}

fn verify_outcome(ctx: &Context, shape: &Shape) -> Result<()> {
    let outcome: OutcomeAttr = shape.require_attr(ctx, &KEY_OUTCOME, "outcome")?;
    shape.results(2)?;
    shape.require(
        shape.result_types[0].deref(ctx).is::<OutcomeType>(),
        "an outcome yields a pending outcome",
    )?;
    match outcome.kind {
        OutcomeKind::Normal => shape.operands(1),
        OutcomeKind::Error => {
            shape.operands(2)?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<ErrorType>()
                    || shape.operand_types[0].deref(ctx).is::<NominalType>(),
                "an error outcome carries the raised error",
            )
        }
        OutcomeKind::Exit(_) => {
            let values = shape.operand_types.len().saturating_sub(1);
            shape.require(values <= 1, "an exit outcome carries at most one value")?;
            shape.value_operands(ctx, 0..values)
        }
    }
}

fn verify_resume(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(2)?;
    shape.require(
        shape.operand_types[0].deref(ctx).is::<OutcomeType>(),
        "a resume dispatches on a pending outcome",
    )?;
    let effect: TypeHandle = EffectType::get(ctx).into();
    let sites =
        attr::<ResumeAttr>(ctx, shape.op, &KEY_RESUME).map_or(0, |resume| resume.sites.len());
    let arguments = |index: usize| -> Vec<TypeHandle> {
        shape.successors[index]
            .deref(ctx)
            .arguments()
            .map(|value| value.get_type(ctx))
            .collect()
    };
    let fixed = shape.successors.len().checked_sub(sites);
    match fixed {
        Some(1) => shape.forwards(ctx, 0, &[effect])?,
        Some(2) => {
            shape.forwards(ctx, 0, &[effect])?;
            let error = arguments(1);
            shape.require(
                error.len() == 2 && is_value_type(ctx, error[0]) && error[1] == effect,
                "the error successor takes the raised error and the effect token",
            )?;
        }
        _ => {
            return shape.fail(
                ctx,
                "a resume continues normally, to its error target when an error can be pending, then to each exit site",
            );
        }
    }
    for index in shape.successors.len() - sites..shape.successors.len() {
        let continued = arguments(index);
        let values = continued.len().saturating_sub(1);
        shape.require(
            values <= 1
                && continued.last() == Some(&effect)
                && continued[..values].iter().all(|ty| is_value_type(ctx, *ty)),
            "an exit site's successor takes its value, if any, and the effect token",
        )?;
    }
    Ok(())
}

fn verify_try_bridge(ctx: &Context, shape: &Shape) -> Result<()> {
    let parts: TryAttr = shape.require_attr(ctx, &KEY_TRY, "try parts")?;
    shape.results(1)?;
    shape.successors(0)?;
    shape.require(
        shape.regions.len() == 4,
        "a structured try has four regions",
    )?;
    for index in 0..shape.operand_types.len() - 1 {
        shape.place_operand(ctx, index)?;
        let slot = root_slot(ctx, shape.operand_values[index]);
        shape.require(
            slot.is_some_and(|slot| slot.storage == CoreStorage::Variable),
            "a cleanup operand is a variable slot",
        )?;
    }
    shape.require(
        parts.handler || parts.error_var.is_none(),
        "only a handler binds the caught error",
    )?;
    let present = [true, parts.handler, parts.orelse, parts.finalbody];
    for (region, present) in shape.regions.iter().zip(present) {
        let entry = region.deref(ctx).get_entry_block();
        shape.require(
            entry.is_some() == present,
            "a structured try has exactly the regions its parts declare",
        )?;
        if let Some(entry) = entry {
            let arguments: Vec<TypeHandle> = entry
                .deref(ctx)
                .arguments()
                .map(|value| value.get_type(ctx))
                .collect();
            let effect: TypeHandle = EffectType::get(ctx).into();
            shape.require(
                arguments == [effect],
                "a structured region's entry takes the effect token",
            )?;
        }
    }
    Ok(())
}

/// The slot a place is rooted at, through any projection.
pub fn root_slot(ctx: &Context, place: Value) -> Option<SlotAttr> {
    let mut op = place.defining_op()?;
    while CoreOpKind::of(ctx, op)? == CoreOpKind::Project {
        op = op.deref(ctx).operands().next()?.defining_op()?;
    }
    attr::<SlotAttr>(ctx, op, &KEY_SLOT)
}

/// The operation an `invoke` stands for, or a call-like operation is: the
/// one whose facts it carries.
pub fn invoked_kind(ctx: &Context, op: Ptr<Operation>) -> Option<CoreOpKind> {
    let attributes = &op.deref(ctx).attributes;
    if attributes.get::<CallAttr>(&KEY_CALL).is_some() {
        return Some(CoreOpKind::Call);
    }
    if attributes.get::<IndexAttr>(&KEY_SUBSCRIPT).is_some() {
        return Some(CoreOpKind::Index);
    }
    if attributes.get::<IterNextAttr>(&KEY_ITER_NEXT).is_some() {
        return Some(CoreOpKind::IterNext);
    }
    if attributes.get::<MultiIndexAttr>(&KEY_MULTI_INDEX).is_some() {
        return Some(CoreOpKind::MultiIndex);
    }
    if attributes.get::<SliceAttr>(&KEY_SLICE).is_some() {
        return Some(CoreOpKind::Slice);
    }
    attributes
        .get::<MultiSetAttr>(&KEY_MULTI_SET)
        .map(|_| CoreOpKind::MultiSet)
}

/// The error type the call `op` makes may raise.
pub fn raised_by(ctx: &Context, op: Ptr<Operation>) -> Option<TypeHandle> {
    match invoked_kind(ctx, op)? {
        CoreOpKind::Index => attr::<IndexAttr>(ctx, op, &KEY_SUBSCRIPT)?.call?.raises,
        CoreOpKind::MultiSet => attr::<MultiSetAttr>(ctx, op, &KEY_MULTI_SET)?.call.raises,
        CoreOpKind::IterNext => attr::<IterNextAttr>(ctx, op, &KEY_ITER_NEXT)?.raises,
        CoreOpKind::MultiIndex => {
            attr::<MultiIndexAttr>(ctx, op, &KEY_MULTI_INDEX)?
                .call?
                .raises
        }
        CoreOpKind::Slice => attr::<SliceAttr>(ctx, op, &KEY_SLICE)?.call?.raises,
        _ => attr::<CallAttr>(ctx, op, &KEY_CALL)?.raises,
    }
}

/// Whether `place` is a variable slot itself, under no projection.
pub fn variable_slot(ctx: &Context, place: Value) -> bool {
    place
        .defining_op()
        .and_then(|op| attr::<SlotAttr>(ctx, op, &KEY_SLOT))
        .is_some_and(|slot| slot.storage == CoreStorage::Variable)
}

/// Whether `op`'s identity marks it as register transport.
pub fn is_transport(ctx: &Context, op: Ptr<Operation>) -> bool {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY).is_some_and(|identity| {
        matches!(
            identity.role,
            CoreRole::RegisterLoad(_) | CoreRole::RegisterStore | CoreRole::YieldStore
        )
    })
}

/// An exact literal type: an integer or floating literal not yet
/// materialized into a machine scalar.
pub fn is_exact_literal(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    object.is::<IntLiteralType>() || object.is::<FloatLiteralType>()
}

/// Whether `ty` is the scalar the checker spells a one-lane vector of
/// `dtype` as: `Int`, `Bool`, or `Float64` for their lane types.
pub fn is_scalar_alias(ctx: &Context, ty: TypeHandle, dtype: CoreDtype) -> bool {
    let object = ty.deref(ctx);
    match dtype {
        CoreDtype::Int => object.is::<IntType>(),
        CoreDtype::Bool => object.is::<BoolType>(),
        CoreDtype::Float64 => object.is::<Float64Type>(),
        _ => false,
    }
}

/// A reference, or a pointer whose origin names a place, an origin binder,
/// or the receiver: the handles MIR admits as reference capabilities.
pub fn is_capability(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    object.is::<RefType>()
        || object.downcast_ref::<PointerType>().is_some_and(|pointer| {
            matches!(
                pointer.origin,
                CorePointerOrigin::Place { .. }
                    | CorePointerOrigin::Param { .. }
                    | CorePointerOrigin::SelfPlace { .. }
            )
        })
}

/// The integer and unsigned core types, as the scalar operand types of
/// indices and arithmetic.
pub fn is_machine_integer(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    object.is::<IntType>() || object.is::<UIntType>()
}
