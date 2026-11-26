//! `atomic`: `std.atomic`'s atomic `pop` ops, lowered to the LLVM
//! dialect's `atomic_load`, `atomic_store`, `atomicrmw`, `cmpxchg`, and
//! `fence` through the element's address. An `unordered` access lowers
//! `monotonic`, a strengthening LLVM permits and the dialect's one spelling
//! of it, and a weak cmpxchg lowers strong, which a weak one may always
//! be. A load or store at `not_atomic` is the plain access it is.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::ct::CtValue;
use mojito_types::mlir::{AtomicBinOp, AtomicOp, AtomicOrdering};
use pliron_llvm::attributes::{AtomicOrderingAttr, AtomicRmwKindAttr, SyncScopeAttr};
use pliron_llvm::ops::{
    AtomicCmpxchgOp, AtomicLoadOp, AtomicRmwOp, AtomicStoreOp, ExtractValueOp, FenceOp,
};

impl FnLowering<'_> {
    /// Lower the concrete atomic op `op` at `dest` through `reference`
    /// over `operands`, at its closed orderings and scope.
    pub(super) fn lower_atomic(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        op: AtomicOp,
        reference: Option<Reg>,
        operands: &[Reg],
        (ordering, failure_ordering, syncscope): (&MirConst, Option<&MirConst>, Option<&MirConst>),
    ) -> Result<(), PlironError> {
        let closed = |constant: &MirConst| match constant {
            MirConst::Value(CtValue::MlirAttr(attribute)) => AtomicOrdering::parse(attribute),
            _ => None,
        };
        let Some(success) = closed(ordering) else {
            return Err(self.unsupported_reg(format!("the ordering `{ordering:?}`"), dest));
        };
        let failure = failure_ordering.map(closed);
        let scope = match syncscope {
            None => SyncScopeAttr::System,
            Some(MirConst::Value(CtValue::KgenString(scope))) => match scope.as_str() {
                "" => SyncScopeAttr::System,
                "singlethread" => SyncScopeAttr::SingleThread,
                named => SyncScopeAttr::NamedScope(StringAttr::new(named.to_string())),
            },
            Some(other) => {
                return Err(self.unsupported_reg(format!("the scope `{other:?}`"), dest));
            }
        };
        if op == AtomicOp::Fence {
            let fence = FenceOp::new(ctx, llvm_ordering(success), scope);
            self.append(ctx, fence.get_operation(), Some(dest));
            self.erased.insert(dest.0);
            return Ok(());
        }
        let Some(reference) = reference else {
            return Err(self.unsupported_reg(format!("'{}' with no address", op.name()), dest));
        };
        if success == AtomicOrdering::NotAtomic {
            return match op {
                AtomicOp::Load => self.lower_read_ref(ctx, dest, reference),
                AtomicOp::Store => {
                    self.lower_write_ref(ctx, reference, operands[0])?;
                    self.erased.insert(dest.0);
                    Ok(())
                }
                _ => Err(self.unsupported_reg(format!("a non-atomic '{}'", op.name()), dest)),
            };
        }
        let pointer = self.reg_value(ctx, reference, ScalarTy::PTR)?;
        let element_reg = match op {
            AtomicOp::Load | AtomicOp::Rmw(_) => dest,
            _ => operands[0],
        };
        let scalar = self.atomic_scalar(element_reg)?;
        let ordering = llvm_ordering(success);
        match op {
            AtomicOp::Load => {
                let load = AtomicLoadOp::new(ctx, pointer, scalar.handle(ctx), ordering, scope);
                load.set_alignment(ctx, scalar_bytes(scalar));
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            AtomicOp::Store => {
                let value = self.reg_value(ctx, operands[0], scalar)?;
                let store = AtomicStoreOp::new(ctx, value, pointer, ordering, scope);
                store.set_alignment(ctx, scalar_bytes(scalar));
                self.append(ctx, store.get_operation(), Some(dest));
                self.erased.insert(dest.0);
                Ok(())
            }
            AtomicOp::Rmw(bin_op) => {
                let value = self.reg_value(ctx, operands[0], scalar)?;
                let rmw = AtomicRmwOp::new(
                    ctx,
                    pointer,
                    value,
                    rmw_kind(bin_op, scalar),
                    ordering,
                    scope,
                );
                self.define(ctx, dest, rmw.get_operation(), rmw.get_result(ctx))
            }
            AtomicOp::Cmpxchg { .. } => {
                let expected = self.reg_value(ctx, operands[0], scalar)?;
                let desired = self.reg_value(ctx, operands[1], scalar)?;
                let failure = failure
                    .flatten()
                    .map_or(AtomicOrderingAttr::Monotonic, llvm_ordering);
                let cmpxchg =
                    AtomicCmpxchgOp::new(ctx, pointer, expected, desired, ordering, failure, scope);
                self.append(ctx, cmpxchg.get_operation(), Some(dest));
                let pair = cmpxchg.get_result(ctx);
                self.store_cmpxchg_pair(ctx, dest, pair)
            }
            AtomicOp::Fence => unreachable!("a fence returned above"),
        }
    }

    /// The scalar an atomic op's element register `reg` lowers to; an
    /// element LLVM's atomics take no other.
    fn atomic_scalar(&self, reg: Reg) -> Result<ScalarTy, PlironError> {
        let Some(ty) = self.func.reg_types.get(&reg.0) else {
            return Err(self.unsupported_reg("an untyped atomic element".into(), reg));
        };
        match lower_ty(
            self.name,
            ty,
            &self.layout,
            self.mir_declarations,
            self.reg_span(reg),
        )? {
            LowerTy::Scalar(scalar) if scalar != ScalarTy::Bool && scalar != ScalarTy::Dtype => {
                Ok(scalar)
            }
            _ => Err(self.unsupported_reg(format!("an atomic access to `{ty}`"), reg)),
        }
    }

    /// Store a `cmpxchg`'s `{ value, i1 }` result into fresh storage of
    /// `dest`'s literal struct of the value and its `!kgen.scalar<bool>`.
    fn store_cmpxchg_pair(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        pair: Value,
    ) -> Result<(), PlironError> {
        let Some(mojito_types::types::TyKind::Tuple(elements)) =
            self.func.reg_types.get(&dest.0).map(Ty::kind)
        else {
            return Err(self.unsupported_reg("an untyped cmpxchg result".into(), dest));
        };
        let elements = elements.clone();
        let composed = self.struct_layout_of(&elements, dest)?;
        let storage = self.entry_alloca(
            ctx,
            composed.layout.size.max(1),
            composed.layout.align.max(1),
        );
        for (index, offset) in composed.offsets.iter().enumerate() {
            let extract = ExtractValueOp::new(ctx, pair, vec![index as u32]).map_err(|error| {
                self.unsupported_reg(format!("a cmpxchg result ({error})"), dest)
            })?;
            self.append(ctx, extract.get_operation(), Some(dest));
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            let store = StoreOp::new(ctx, extract.get_result(ctx), address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }
}

/// The LLVM ordering of an atomic `ordering`: `unordered` strengthens to
/// `monotonic`, which the dialect spells for both.
const fn llvm_ordering(ordering: AtomicOrdering) -> AtomicOrderingAttr {
    match ordering {
        AtomicOrdering::NotAtomic | AtomicOrdering::Unordered | AtomicOrdering::Monotonic => {
            AtomicOrderingAttr::Monotonic
        }
        AtomicOrdering::Acquire => AtomicOrderingAttr::Acquire,
        AtomicOrdering::Release => AtomicOrderingAttr::Release,
        AtomicOrdering::AcqRel => AtomicOrderingAttr::AcqRel,
        AtomicOrdering::SeqCst => AtomicOrderingAttr::SeqCst,
    }
}

/// The natural alignment of `scalar`, its byte width, which LLVM requires
/// an atomic load or store to name.
fn scalar_bytes(scalar: ScalarTy) -> u32 {
    match scalar {
        ScalarTy::Int | ScalarTy::UInt | ScalarTy::Float64 | ScalarTy::Ptr(_) => 8,
        ScalarTy::Bool | ScalarTy::Dtype => 1,
        ScalarTy::Sized(Dtype::Float16) => 2,
        ScalarTy::Sized(Dtype::Float32) => 4,
        ScalarTy::Sized(dtype) => {
            mojito_vm::runtime::integer_dtype_bits(dtype).map_or(8, |(bits, _)| bits / 8)
        }
    }
}

/// The `atomicrmw` kind of `bin_op` over `scalar`, by its floatness and
/// signedness.
fn rmw_kind(bin_op: AtomicBinOp, scalar: ScalarTy) -> AtomicRmwKindAttr {
    let float = matches!(
        scalar,
        ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float16 | Dtype::Float32)
    );
    let unsigned = scalar.int_shape().is_some_and(|(_, signed)| !signed);
    match (bin_op, float, unsigned) {
        (AtomicBinOp::Xchg, ..) => AtomicRmwKindAttr::Xchg,
        (AtomicBinOp::Add, true, _) => AtomicRmwKindAttr::FAdd,
        (AtomicBinOp::Add, false, _) => AtomicRmwKindAttr::Add,
        (AtomicBinOp::Sub, true, _) => AtomicRmwKindAttr::FSub,
        (AtomicBinOp::Sub, false, _) => AtomicRmwKindAttr::Sub,
        (AtomicBinOp::Max, true, _) => AtomicRmwKindAttr::FMax,
        (AtomicBinOp::Max, false, true) => AtomicRmwKindAttr::UMax,
        (AtomicBinOp::Max, false, false) => AtomicRmwKindAttr::Max,
        (AtomicBinOp::Min, true, _) => AtomicRmwKindAttr::FMin,
        (AtomicBinOp::Min, false, true) => AtomicRmwKindAttr::UMin,
        (AtomicBinOp::Min, false, false) => AtomicRmwKindAttr::Min,
    }
}
