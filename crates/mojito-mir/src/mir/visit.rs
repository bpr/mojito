//! The walk over every type and compile-time parameter site of a MIR body.
//!
//! It reaches them as MLIR's `AttrTypeReplacer` reaches every op's
//! attributes and types, regions included.
//! A client implements [`MutTypeVisitor`], one hook per kind of site, and
//! drives it with [`walk_function_mut`] or [`walk_blocks_mut`]. The walk
//! matches every [`MirInstr`] variant by name, so a new variant cannot skip
//! it. Within an instruction the sites are visited in a
//! fixed order, which is the order a failing client reports its first error
//! in.

use super::{
    Const, MirBlock, MirFunction, MirInstr, MirParamArg, MirPlace, MirSubscriptCall, Proj,
    instruction_regions_mut,
};
use mojito_checked::checked::ComptimeSequence;
use mojito_hir::hir::VarId;
use mojito_types::param_expr::ParamRef;
use mojito_types::types::{
    GenericConstraint, LaneMask, ParamDecl, SimdDtype, SimdWidth, Ty, TyArg, TyKind, VariantIndex,
};

/// Where a visited type sits. A client that treats sites differently
/// matches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TySite {
    /// A function parameter's type.
    Parameter,
    /// A variable slot's type.
    Slot(VarId),
    /// A register's type.
    Register(u32),
    /// The function's return type.
    Return,
    /// The function's declared error type.
    Error,
    /// A place's root, projection, or designated type.
    Place,
    /// A type an instruction names: a size query's, a literal
    /// materialization's target, a storage element, an exhaustion type, a
    /// binding type.
    Instruction,
    /// A type whose unqualified spelling the instruction reads.
    TypeName,
    /// The type a `ConstructType` constructs.
    ConstructedType,
    /// A call's error type or instantiated contract.
    Contract,
    /// The spelled receiver of a static call on a generic struct.
    Receiver,
    /// An element type of a tuple construction.
    TupleElement,
    /// A call's result type or a reference result's referent.
    Result,
    /// A subscript call's parameter type.
    SubscriptParameter,
    /// A SIMD instruction's slots, visited as the vector type they build.
    VectorSlots,
}

/// Where a visited compile-time argument sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamArgSite {
    /// A call's, an indirect call's, or a method call's bracket argument.
    Call,
    /// A subscript call's bracket argument.
    Subscript,
    /// The index of a constructed pack element (`Ts[i]()`).
    ConstructedElement,
}

/// Whose binders a visited declaration list declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamDeclSite {
    /// A callable value's generic contract.
    CallableContract,
    /// A method call's generic contract.
    MethodContract,
    /// A subscript call's generic contract.
    SubscriptContract,
}

/// A mutating visitor over every type and compile-time parameter site of a
/// MIR body. A hook that fails stops the walk.
pub trait MutTypeVisitor {
    type Error;

    fn visit_ty(&mut self, ty: &mut Ty, site: TySite) -> Result<(), Self::Error>;
    fn visit_ty_arg(&mut self, arg: &mut TyArg) -> Result<(), Self::Error>;
    /// A `Variant` construction's alternatives, as one list: a bound `*Ts`
    /// spread expands it, so it is not visited element by element.
    fn visit_variant_alternatives(&mut self, alternatives: &mut Vec<Ty>)
    -> Result<(), Self::Error>;
    fn visit_variant_index(&mut self, index: &mut VariantIndex) -> Result<(), Self::Error>;
    fn visit_lane_mask(&mut self, mask: &mut LaneMask) -> Result<(), Self::Error>;
    fn visit_param_arg(
        &mut self,
        arg: &mut MirParamArg,
        site: ParamArgSite,
    ) -> Result<(), Self::Error>;
    fn visit_param_decls(
        &mut self,
        decls: &mut [ParamDecl],
        site: ParamDeclSite,
    ) -> Result<(), Self::Error>;
    /// A constant an instruction reads or addresses: a `Const`'s and a
    /// parameter list address's values.
    fn visit_constant(&mut self, constant: &mut Const) -> Result<(), Self::Error>;
    /// A parameter an instruction names: a constructed type
    /// parameter, a `comptime for` binder.
    fn visit_param_ref(&mut self, param: &mut ParamRef) -> Result<(), Self::Error>;
    fn visit_constraint(&mut self, cond: &mut GenericConstraint) -> Result<(), Self::Error>;
    fn visit_sequence(&mut self, source: &mut ComptimeSequence) -> Result<(), Self::Error>;

    /// One instruction, its regions not included: [`walk_blocks_mut`] walks
    /// them after this returns, so an override sees, and may rewrite, the
    /// instruction first.
    fn visit_instruction(&mut self, instruction: &mut MirInstr) -> Result<(), Self::Error> {
        walk_instruction_mut(self, instruction)
    }

    fn visit_place(&mut self, place: &mut MirPlace) -> Result<(), Self::Error> {
        walk_place_mut(self, place)
    }
}

/// Visit `function`'s parameter, slot, register, return, and error types,
/// then its blocks.
pub fn walk_function_mut<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    function: &mut MirFunction,
) -> Result<(), V::Error> {
    for ty in &mut function.param_types {
        visitor.visit_ty(ty, TySite::Parameter)?;
    }
    for (var, ty) in &mut function.var_tys {
        visitor.visit_ty(ty, TySite::Slot(*var))?;
    }
    for (reg, ty) in &mut function.reg_types {
        visitor.visit_ty(ty, TySite::Register(*reg))?;
    }
    if let Some(ty) = &mut function.ret_ty {
        visitor.visit_ty(ty, TySite::Return)?;
    }
    if let Some(ty) = &mut function.error_ty {
        visitor.visit_ty(ty, TySite::Error)?;
    }
    walk_blocks_mut(visitor, &mut function.blocks)
}

/// Visit each block's instructions, each followed by its regions. A
/// terminator names no type or parameter site.
pub fn walk_blocks_mut<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    blocks: &mut [MirBlock],
) -> Result<(), V::Error> {
    for block in blocks {
        for instruction in &mut block.instrs {
            visitor.visit_instruction(instruction)?;
            for region in instruction_regions_mut(instruction) {
                walk_blocks_mut(visitor, region)?;
            }
        }
    }
    Ok(())
}

/// Visit every site of one instruction, none of its regions'.
pub fn walk_instruction_mut<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    instruction: &mut MirInstr,
) -> Result<(), V::Error> {
    use MirInstr as I;
    match instruction {
        I::MakeVariant {
            alternatives,
            index,
            ..
        } => {
            visitor.visit_variant_index(index)?;
            visitor.visit_variant_alternatives(alternatives)
        }
        I::VariantIs { index, .. }
        | I::VariantGet { index, .. }
        | I::VariantTake { index, .. }
        | I::VariantDeinitWith { index, .. } => visitor.visit_variant_index(index),
        I::VariantSet { place, index, .. } | I::VariantSetInitWith { place, index, .. } => {
            visitor.visit_variant_index(index)?;
            visitor.visit_place(place)
        }
        I::VariantReplace {
            place,
            input_index,
            output_index,
            ..
        } => {
            visitor.visit_variant_index(input_index)?;
            visitor.visit_variant_index(output_index)?;
            visitor.visit_place(place)
        }
        I::MakeSimd { dtype, width, .. }
        | I::SimdCast { dtype, width, .. }
        | I::SimdBitcast { dtype, width, .. } => visit_vector_slots(visitor, dtype, width),
        I::SimdShuffle { mask, .. } => visitor.visit_lane_mask(mask),
        I::EstablishLoans { loans, .. } => loans
            .iter_mut()
            .try_for_each(|loan| visitor.visit_place(&mut loan.place)),
        I::MakeRef { place, .. }
        | I::MovePlace { place, .. }
        | I::Store { place, .. }
        | I::StoreRef { place, .. }
        | I::LoadPlace { place, .. }
        | I::DropPlace { place, .. }
        | I::MarkInitialized { place }
        | I::MarkDestroyed { place } => visitor.visit_place(place),
        I::MakeClosure { captures, .. } => captures
            .iter_mut()
            .try_for_each(|capture| visitor.visit_place(&mut capture.place)),
        I::MaterializeLiteral { target: ty, .. }
        | I::SizeOf { ty, .. }
        | I::PointerStorageTake { element: ty, .. }
        | I::PointerStorageDestroy { element: ty, .. }
        | I::UninitStorageTake { element: ty, .. }
        | I::UninitStorageDestroy { element: ty, .. }
        | I::DefVar {
            binding_ty: Some(ty),
            ..
        } => visitor.visit_ty(ty, TySite::Instruction),
        I::Call {
            raises,
            arg_places,
            kwarg_places,
            param_arg_regs,
            receiver,
            instantiated_args,
            ..
        } => {
            visit_opt_ty(
                visitor,
                raises.as_mut().map(|raises| &mut raises.ty),
                TySite::Contract,
            )?;
            visit_places(visitor, arg_places)?;
            visit_places(visitor, kwarg_places)?;
            visit_opt_ty(visitor, receiver.as_mut(), TySite::Receiver)?;
            visit_ty_args(visitor, instantiated_args)?;
            visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)
        }
        I::CallIndirect {
            raises,
            callee_place,
            arg_places,
            kwarg_places,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            visit_opt_ty(
                visitor,
                raises.as_mut().map(|raises| &mut raises.ty),
                TySite::Contract,
            )?;
            visit_opt_place(visitor, callee_place.as_deref_mut())?;
            visit_places(visitor, arg_places)?;
            visit_places(visitor, kwarg_places)?;
            visit_opt_ty(visitor, instantiated_contract.as_mut(), TySite::Contract)?;
            visit_ty_args(visitor, instantiated_args)?;
            visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)?;
            visitor.visit_param_decls(param_decls, ParamDeclSite::CallableContract)
        }
        I::MethodCall {
            raises,
            reference_result,
            recv_place,
            arg_places,
            kwarg_places,
            param_arg_regs,
            param_decls,
            instantiated_args,
            ..
        } => {
            visit_opt_ty(
                visitor,
                raises.as_mut().map(|raises| &mut raises.ty),
                TySite::Contract,
            )?;
            if let Some(reference) = reference_result {
                visitor.visit_ty(&mut reference.referent, TySite::Result)?;
            }
            visit_opt_place(visitor, recv_place.as_deref_mut())?;
            visit_places(visitor, arg_places)?;
            visit_places(visitor, kwarg_places)?;
            visit_ty_args(visitor, instantiated_args)?;
            visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)?;
            visitor.visit_param_decls(param_decls, ParamDeclSite::MethodContract)
        }
        I::UnOp { raises, .. } | I::BinOp { raises, .. } => visit_opt_ty(
            visitor,
            raises.as_mut().map(|raises| &mut raises.ty),
            TySite::Contract,
        ),
        I::Index {
            base_place,
            index_place,
            call,
            ..
        } => {
            visit_opt_place(visitor, base_place.as_deref_mut())?;
            visit_opt_place(visitor, index_place.as_deref_mut())?;
            call.as_deref_mut()
                .map_or(Ok(()), |call| walk_subscript_call(visitor, call))
        }
        I::MultiIndex {
            object_place,
            arg_places,
            kwarg_places,
            call,
            ..
        } => {
            visit_opt_place(visitor, object_place.as_deref_mut())?;
            visit_places(visitor, arg_places)?;
            visit_places(visitor, kwarg_places)?;
            call.as_deref_mut()
                .map_or(Ok(()), |call| walk_subscript_call(visitor, call))
        }
        I::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            ..
        } => {
            visit_opt_place(visitor, receiver_place.as_deref_mut())?;
            visit_places(visitor, arg_places)?;
            visit_opt_place(visitor, value_place.as_deref_mut())?;
            walk_subscript_call(visitor, call)
        }
        I::MakeTuple {
            element_types: Some(types),
            ..
        } => types
            .iter_mut()
            .try_for_each(|ty| visitor.visit_ty(ty, TySite::TupleElement)),
        I::TypeName { ty, .. } => visitor.visit_ty(ty, TySite::TypeName),
        I::ConstructType { ty, .. } => visitor.visit_ty(ty, TySite::ConstructedType),
        I::ConstructTypeParam {
            param,
            kwarg_places,
            element,
            ..
        } => {
            visitor.visit_param_ref(param)?;
            visit_places(visitor, kwarg_places)?;
            element.as_mut().map_or(Ok(()), |element| {
                visitor.visit_param_arg(element, ParamArgSite::ConstructedElement)
            })
        }
        I::Const { k: constant, .. }
        | I::ParamListAddress {
            values: constant, ..
        } => visitor.visit_constant(constant),
        I::ComptimeIf { conds, .. } => conds
            .iter_mut()
            .try_for_each(|cond| visitor.visit_constraint(cond)),
        I::ComptimeFor { binder, source, .. } => {
            visitor.visit_param_ref(binder)?;
            visitor.visit_sequence(source)
        }
        I::ComptimeAssert { cond } => visitor.visit_constraint(cond),
        I::Match { .. } => Ok(()),
        I::DefVar {
            binding_ty: None, ..
        }
        | I::MakeTuple {
            element_types: None,
            ..
        }
        | I::MakeStruct { .. }
        | I::MakeArray { .. }
        | I::InvalidateInteriors { .. }
        | I::ReadRef { .. }
        | I::Rebind { .. }
        | I::CopyValue { .. }
        | I::WriteRef { .. }
        | I::KeepAlive { .. }
        | I::StoreToMem { .. }
        | I::ScalarCast { .. }
        | I::UseVar { .. }
        | I::DeclareVar { .. }
        | I::UninitStorage { .. }
        | I::GetField { .. }
        | I::Try { .. }
        | I::If { .. }
        | I::Loop { .. }
        | I::Drop { .. }
        | I::DropVar { .. }
        | I::ConsumeVar { .. }
        | I::Unsupported(_)
        | I::GetIter { .. }
        | I::HasNext { .. }
        | I::VariantTag { .. }
        | I::Next { .. } => Ok(()),
    }
}

/// Visit a place's root type, its projection types, the alternatives its
/// variant projections select, then its designated type.
pub fn walk_place_mut<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    place: &mut MirPlace,
) -> Result<(), V::Error> {
    visit_opt_ty(visitor, place.root_ty.as_mut(), TySite::Place)?;
    for ty in &mut place.projection_tys {
        visitor.visit_ty(ty, TySite::Place)?;
    }
    for projection in &mut place.proj {
        if let Proj::Variant(index) = projection {
            visitor.visit_variant_index(index)?;
        }
    }
    visit_opt_ty(visitor, place.ty.as_mut(), TySite::Place)
}

fn visit_opt_ty<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    ty: Option<&mut Ty>,
    site: TySite,
) -> Result<(), V::Error> {
    ty.map_or(Ok(()), |ty| visitor.visit_ty(ty, site))
}

fn visit_places<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    places: &mut [Option<MirPlace>],
) -> Result<(), V::Error> {
    places
        .iter_mut()
        .try_for_each(|place| visit_opt_place(visitor, place.as_mut()))
}

fn visit_opt_place<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    place: Option<&mut MirPlace>,
) -> Result<(), V::Error> {
    place.map_or(Ok(()), |place| visitor.visit_place(place))
}

fn visit_param_args<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    args: &mut [MirParamArg],
    site: ParamArgSite,
) -> Result<(), V::Error> {
    args.iter_mut()
        .try_for_each(|arg| visitor.visit_param_arg(arg, site))
}

fn visit_ty_args<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    args: &mut [TyArg],
) -> Result<(), V::Error> {
    args.iter_mut()
        .try_for_each(|arg| visitor.visit_ty_arg(arg))
}

/// A subscript call's error, result, and parameter types, its bracket
/// arguments, its reference result's referent, then its contract.
fn walk_subscript_call<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    call: &mut MirSubscriptCall,
) -> Result<(), V::Error> {
    visit_opt_ty(
        visitor,
        call.raises.as_mut().map(|raises| &mut raises.ty),
        TySite::Contract,
    )?;
    visitor.visit_ty(&mut call.result_ty, TySite::Result)?;
    for argument in &mut call.arguments {
        visitor.visit_ty(&mut argument.parameter_ty, TySite::SubscriptParameter)?;
    }
    visit_param_args(visitor, &mut call.param_arg_regs, ParamArgSite::Subscript)?;
    if let Some(reference) = &mut call.reference_result {
        visitor.visit_ty(&mut reference.referent, TySite::Result)?;
    }
    visitor.visit_param_decls(&mut call.param_decls, ParamDeclSite::SubscriptContract)
}

/// A SIMD instruction's slots, visited as the vector type they build; a
/// visited type that is no vector leaves them as they were.
fn visit_vector_slots<V: MutTypeVisitor + ?Sized>(
    visitor: &mut V,
    dtype: &mut SimdDtype,
    width: &mut SimdWidth,
) -> Result<(), V::Error> {
    let mut built = Ty::new(TyKind::Simd {
        dtype: dtype.clone(),
        width: width.clone(),
    });
    visitor.visit_ty(&mut built, TySite::VectorSlots)?;
    if let Some((visited_dtype, visited_width)) = mojito_types::types::simd_slots(&built) {
        *dtype = visited_dtype;
        *width = visited_width;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirTerm, Reg, TryLabel};

    /// Counts each hook's visits and the sites its types sit at.
    #[derive(Default)]
    struct Counter {
        sites: Vec<TySite>,
        variant_lists: usize,
        variant_indices: usize,
        lane_masks: usize,
        constants: usize,
        constraints: usize,
    }

    impl MutTypeVisitor for Counter {
        type Error = ();

        fn visit_ty(&mut self, _: &mut Ty, site: TySite) -> Result<(), ()> {
            self.sites.push(site);
            Ok(())
        }
        fn visit_ty_arg(&mut self, _: &mut TyArg) -> Result<(), ()> {
            Ok(())
        }
        fn visit_variant_alternatives(&mut self, _: &mut Vec<Ty>) -> Result<(), ()> {
            self.variant_lists += 1;
            Ok(())
        }
        fn visit_variant_index(&mut self, _: &mut VariantIndex) -> Result<(), ()> {
            self.variant_indices += 1;
            Ok(())
        }
        fn visit_lane_mask(&mut self, _: &mut LaneMask) -> Result<(), ()> {
            self.lane_masks += 1;
            Ok(())
        }
        fn visit_param_arg(&mut self, _: &mut MirParamArg, _: ParamArgSite) -> Result<(), ()> {
            Ok(())
        }
        fn visit_param_decls(&mut self, _: &mut [ParamDecl], _: ParamDeclSite) -> Result<(), ()> {
            Ok(())
        }
        fn visit_constant(&mut self, _: &mut Const) -> Result<(), ()> {
            self.constants += 1;
            Ok(())
        }
        fn visit_param_ref(&mut self, _: &mut ParamRef) -> Result<(), ()> {
            Ok(())
        }
        fn visit_constraint(&mut self, _: &mut GenericConstraint) -> Result<(), ()> {
            self.constraints += 1;
            Ok(())
        }
        fn visit_sequence(&mut self, _: &mut ComptimeSequence) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn visit_walk_reaches_every_site_kind() {
        let int = || Ty::int();
        let mut place = MirPlace::root(0, Some(int()));
        place.project(Proj::Variant(VariantIndex::Known(0)), Ty::new(TyKind::Bool));
        let region = vec![MirBlock {
            instrs: vec![MirInstr::SizeOf {
                dest: Reg(5),
                ty: int(),
            }],
            term: MirTerm::TryYield(Vec::new()),
        }];
        let mut function = MirFunction {
            param_types: vec![int()],
            ret_ty: Some(int()),
            reg_types: std::iter::once((0, int())).collect(),
            blocks: vec![MirBlock {
                instrs: vec![
                    MirInstr::Const {
                        dest: Reg(0),
                        k: Const::Int(1),
                    },
                    MirInstr::MakeVariant {
                        dest: Reg(1),
                        alternatives: vec![int()],
                        index: VariantIndex::Known(0),
                        value: Reg(0),
                        constructor: None,
                    },
                    MirInstr::SimdShuffle {
                        dest: Reg(2),
                        value: Reg(1),
                        other: None,
                        mask: LaneMask::Known(vec![0]),
                    },
                    MirInstr::MakeSimd {
                        dest: Reg(3),
                        dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
                        width: SimdWidth::Known(4),
                        elems: vec![Reg(0)],
                    },
                    MirInstr::LoadPlace {
                        dest: Reg(4),
                        place,
                    },
                    MirInstr::Try {
                        label: TryLabel(0),
                        error: None,
                        results: Vec::new(),
                        body: region,
                        handler_args: Vec::new(),
                        handler: None,
                        else_args: Vec::new(),
                        orelse: None,
                    },
                    MirInstr::ComptimeIf {
                        results: Vec::new(),
                        conds: vec![GenericConstraint::Bool(true)],
                        arms: vec![vec![MirBlock {
                            instrs: Vec::new(),
                            term: MirTerm::ComptimeYield(Vec::new()),
                        }]],
                        orelse: vec![MirBlock {
                            instrs: Vec::new(),
                            term: MirTerm::ComptimeYield(Vec::new()),
                        }],
                    },
                ],
                term: MirTerm::Return(None),
            }],
            ..MirFunction::default()
        };
        let mut counter = Counter::default();
        walk_function_mut(&mut counter, &mut function).unwrap();
        assert_eq!(
            counter.sites,
            [
                TySite::Parameter,
                TySite::Register(0),
                TySite::Return,
                TySite::VectorSlots,
                TySite::Place,
                TySite::Place,
                TySite::Place,
                TySite::Instruction,
            ]
        );
        assert_eq!(counter.constants, 1);
        assert_eq!(counter.variant_lists, 1);
        assert_eq!(counter.variant_indices, 2);
        assert_eq!(counter.lane_masks, 1);
        assert_eq!(counter.constraints, 1);
    }
}
