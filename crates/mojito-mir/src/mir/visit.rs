//! The walks over every type and compile-time parameter site of a MIR body.
//!
//! They reach them as MLIR's `AttrTypeWalker` and `AttrTypeReplacer` reach
//! every op's attributes and types, regions included, and are expanded from
//! one inventory of sites as those two are derived from one sub-element
//! declaration. A client implements [`TypeVisitor`] to read the sites or
//! [`MutTypeVisitor`] to rewrite them, one hook per kind of site, and drives
//! it with [`walk_function`] or [`walk_blocks`] (`*_mut` for the rewrite).
//! The walk matches every [`MirInstr`] variant by name, so a new variant
//! cannot skip it. Within an instruction the sites are visited in a fixed
//! order, which is the order a failing client reports its first error in.

use super::{
    Const, MirBlock, MirFunction, MirInstr, MirParamArg, MirPlace, MirSubscriptCall, Proj,
    instruction_regions, instruction_regions_mut,
};
use mojito_checked::checked::ComptimeSequence;
use mojito_hir::hir::VarId;
use mojito_types::param_expr::ParamRef;
use mojito_types::types::{LaneMask, ParamDecl, SimdDtype, SimdWidth, Ty, TyArg, VariantIndex};

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

/// Take an `Option`'s contents by the walk's mutability.
macro_rules! opt_ref {
    (mut $option:expr) => {
        $option.as_mut()
    };
    ($option:expr) => {
        $option.as_ref()
    };
}

/// Take a boxed `Option`'s contents by the walk's mutability.
macro_rules! opt_deref {
    (mut $option:expr) => {
        $option.as_deref_mut()
    };
    ($option:expr) => {
        $option.as_deref()
    };
}

/// Iterate a slice by the walk's mutability.
macro_rules! slice_iter {
    (mut $slice:expr) => {
        $slice.iter_mut()
    };
    ($slice:expr) => {
        $slice.iter()
    };
}

/// The mutating walk writes a visited vector type's slots back into the
/// instruction; the read-only walk leaves them.
macro_rules! write_back_slots {
    (mut $built:ident, $dtype:ident, $width:ident) => {
        if let Some((visited_dtype, visited_width)) = mojito_types::types::simd_slots(&$built) {
            *$dtype = visited_dtype;
            *$width = visited_width;
        }
    };
    ($built:ident, $dtype:ident, $width:ident) => {};
}

/// One inventory of a MIR body's type and parameter sites, expanded once
/// per mutability as rustc's `make_mir_visitor!` is: MLIR derives its
/// `AttrTypeWalker` and `AttrTypeReplacer` from one sub-element declaration
/// the same way, so the two walks cannot reach different sites.
macro_rules! type_visitor {
    (
        $(#[$trait_doc:meta])*
        trait $Visitor:ident;
        walks $walk_function:ident, $walk_blocks:ident, $walk_instruction:ident, $walk_place:ident;
        regions $regions:path;
        alternatives $Alternatives:ty;
        helpers $helpers:ident;
        mutability ($($m:ident)?)
    ) => {
        $(#[$trait_doc])*
        pub trait $Visitor {
            type Error;

            fn visit_ty(&mut self, ty: &$($m)? Ty, site: TySite) -> Result<(), Self::Error>;
            fn visit_ty_arg(&mut self, arg: &$($m)? TyArg) -> Result<(), Self::Error>;
            /// A `Variant` construction's alternatives, as one list: a bound
            /// `*Ts` spread expands it, so it is not visited element by
            /// element.
            fn visit_variant_alternatives(
                &mut self,
                alternatives: &$($m)? $Alternatives,
            ) -> Result<(), Self::Error>;
            fn visit_variant_index(
                &mut self,
                index: &$($m)? VariantIndex,
            ) -> Result<(), Self::Error>;
            /// A symbolic field step ([`Proj::FieldAt`]) over the type of the
            /// storage it selects into, already visited: a client closing the
            /// step rewrites it to its [`Proj::Field`].
            fn visit_field_step(
                &mut self,
                step: &$($m)? Proj,
                base: Option<&Ty>,
            ) -> Result<(), Self::Error>;
            fn visit_lane_mask(&mut self, mask: &$($m)? LaneMask) -> Result<(), Self::Error>;
            fn visit_param_arg(
                &mut self,
                arg: &$($m)? MirParamArg,
                site: ParamArgSite,
            ) -> Result<(), Self::Error>;
            fn visit_param_decls(
                &mut self,
                decls: &$($m)? [ParamDecl],
                site: ParamDeclSite,
            ) -> Result<(), Self::Error>;
            /// A constant an instruction reads or addresses: a `Const`'s and
            /// a parameter list address's values, an LLVM intrinsic call's
            /// name and side-effect flag, an op's fast-math flag, an array
            /// index, an inline count, and the `comptime if` conditions and
            /// `comptime assert` values.
            fn visit_constant(&mut self, constant: &$($m)? Const) -> Result<(), Self::Error>;
            /// A parameter an instruction names: a constructed type
            /// parameter, a `comptime for` binder.
            fn visit_param_ref(&mut self, param: &$($m)? ParamRef) -> Result<(), Self::Error>;
            fn visit_sequence(
                &mut self,
                source: &$($m)? ComptimeSequence,
            ) -> Result<(), Self::Error>;

            #[doc = concat!(
                "One instruction, its regions not included: [`",
                stringify!($walk_blocks),
                "`] walks them after this returns, so an override sees the instruction first."
            )]
            fn visit_instruction(
                &mut self,
                instruction: &$($m)? MirInstr,
            ) -> Result<(), Self::Error> {
                $walk_instruction(self, instruction)
            }

            fn visit_place(&mut self, place: &$($m)? MirPlace) -> Result<(), Self::Error> {
                $walk_place(self, place)
            }
        }

        /// Visit `function`'s parameter, slot, register, return, and error
        /// types, then its blocks.
        pub fn $walk_function<V: $Visitor + ?Sized>(
            visitor: &mut V,
            function: &$($m)? MirFunction,
        ) -> Result<(), V::Error> {
            for ty in &$($m)? function.param_types {
                visitor.visit_ty(ty, TySite::Parameter)?;
            }
            for (var, ty) in &$($m)? function.var_tys {
                visitor.visit_ty(ty, TySite::Slot(*var))?;
            }
            for (reg, ty) in &$($m)? function.reg_types {
                visitor.visit_ty(ty, TySite::Register(*reg))?;
            }
            if let Some(ty) = &$($m)? function.ret_ty {
                visitor.visit_ty(ty, TySite::Return)?;
            }
            if let Some(ty) = &$($m)? function.error_ty {
                visitor.visit_ty(ty, TySite::Error)?;
            }
            $walk_blocks(visitor, &$($m)? function.body)
        }

        /// Visit each block's instructions, each followed by its regions. A
        /// terminator names no type or parameter site.
        pub fn $walk_blocks<V: $Visitor + ?Sized>(
            visitor: &mut V,
            block: &$($m)? MirBlock,
        ) -> Result<(), V::Error> {
            for instruction in &$($m)? block.instrs {
                visitor.visit_instruction(instruction)?;
                for region in $regions(instruction) {
                    $walk_blocks(visitor, region)?;
                }
            }
            Ok(())
        }

        /// Visit every site of one instruction, none of its regions'.
        pub fn $walk_instruction<V: $Visitor + ?Sized>(
            visitor: &mut V,
            instruction: &$($m)? MirInstr,
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
                | I::VariantTake { index, .. } => visitor.visit_variant_index(index),
                I::VariantSet { place, index, .. } => {
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
                I::SimdCast {
                    dtype,
                    width,
                    fastmath,
                    ..
                } => {
                    $helpers::visit_vector_slots(visitor, dtype, width)?;
                    $helpers::visit_opt_constant(visitor, opt_ref!($($m)? fastmath))
                }
                I::MakeSimd { dtype, width, .. } | I::SimdBitcast { dtype, width, .. } => {
                    $helpers::visit_vector_slots(visitor, dtype, width)
                }
                I::SimdShuffle { mask, .. } => visitor.visit_lane_mask(mask),
                I::EstablishLoans { loans, .. } => slice_iter!($($m)? loans)
                    .try_for_each(|loan| visitor.visit_place(&$($m)? loan.place)),
                I::MakeRef { place, .. }
                | I::MovePlace { place, .. }
                | I::Store { place, .. }
                | I::StoreRef { place, .. }
                | I::LoadPlace { place, .. }
                | I::DropPlace { place, .. }
                | I::MarkInitialized { place }
                | I::MarkDestroyed { place } => visitor.visit_place(place),
                I::MakeClosure { captures, .. } => slice_iter!($($m)? captures)
                    .try_for_each(|capture| visitor.visit_place(&$($m)? capture.place)),
                I::MaterializeLiteral { target: ty, .. }
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
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? raises).map(|raises| &$($m)? raises.ty),
                        TySite::Contract,
                    )?;
                    $helpers::visit_places(visitor, arg_places)?;
                    $helpers::visit_places(visitor, kwarg_places)?;
                    $helpers::visit_opt_ty(visitor, opt_ref!($($m)? receiver), TySite::Receiver)?;
                    $helpers::visit_ty_args(visitor, instantiated_args)?;
                    $helpers::visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)
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
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? raises).map(|raises| &$($m)? raises.ty),
                        TySite::Contract,
                    )?;
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? callee_place))?;
                    $helpers::visit_places(visitor, arg_places)?;
                    $helpers::visit_places(visitor, kwarg_places)?;
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? instantiated_contract),
                        TySite::Contract,
                    )?;
                    $helpers::visit_ty_args(visitor, instantiated_args)?;
                    $helpers::visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)?;
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
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? raises).map(|raises| &$($m)? raises.ty),
                        TySite::Contract,
                    )?;
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? reference_result)
                            .map(|reference| &$($m)? reference.referent),
                        TySite::Result,
                    )?;
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? recv_place))?;
                    $helpers::visit_places(visitor, arg_places)?;
                    $helpers::visit_places(visitor, kwarg_places)?;
                    $helpers::visit_ty_args(visitor, instantiated_args)?;
                    $helpers::visit_param_args(visitor, param_arg_regs, ParamArgSite::Call)?;
                    visitor.visit_param_decls(param_decls, ParamDeclSite::MethodContract)
                }
                I::UnOp { raises, .. } => $helpers::visit_opt_ty(
                    visitor,
                    opt_ref!($($m)? raises).map(|raises| &$($m)? raises.ty),
                    TySite::Contract,
                ),
                I::BinOp {
                    raises, fastmath, ..
                } => {
                    $helpers::visit_opt_ty(
                        visitor,
                        opt_ref!($($m)? raises).map(|raises| &$($m)? raises.ty),
                        TySite::Contract,
                    )?;
                    $helpers::visit_opt_constant(visitor, opt_ref!($($m)? fastmath))
                }
                I::Pop { fastmath, .. } => {
                    $helpers::visit_opt_constant(visitor, opt_ref!($($m)? fastmath))
                }
                I::Index {
                    base_place,
                    index_place,
                    call,
                    ..
                } => {
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? base_place))?;
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? index_place))?;
                    opt_deref!($($m)? call)
                        .map_or(Ok(()), |call| $helpers::walk_subscript_call(visitor, call))
                }
                I::MultiIndex {
                    object_place,
                    arg_places,
                    kwarg_places,
                    call,
                    ..
                } => {
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? object_place))?;
                    $helpers::visit_places(visitor, arg_places)?;
                    $helpers::visit_places(visitor, kwarg_places)?;
                    opt_deref!($($m)? call)
                        .map_or(Ok(()), |call| $helpers::walk_subscript_call(visitor, call))
                }
                I::MultiSet {
                    receiver_place,
                    arg_places,
                    value_place,
                    call,
                    ..
                } => {
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? receiver_place))?;
                    $helpers::visit_places(visitor, arg_places)?;
                    $helpers::visit_opt_place(visitor, opt_deref!($($m)? value_place))?;
                    $helpers::walk_subscript_call(visitor, call)
                }
                I::MakeTuple {
                    element_types: Some(types),
                    ..
                } => slice_iter!($($m)? types)
                    .try_for_each(|ty| visitor.visit_ty(ty, TySite::TupleElement)),
                I::ConstructType { ty, .. } => visitor.visit_ty(ty, TySite::ConstructedType),
                I::ConstructTypeParam {
                    param,
                    kwarg_places,
                    element,
                    ..
                } => {
                    visitor.visit_param_ref(param)?;
                    $helpers::visit_places(visitor, kwarg_places)?;
                    opt_ref!($($m)? element).map_or(Ok(()), |element| {
                        visitor.visit_param_arg(element, ParamArgSite::ConstructedElement)
                    })
                }
                I::Const { k: constant, .. }
                | I::ParamListAddress {
                    values: constant, ..
                } => visitor.visit_constant(constant),
                I::CallLlvmIntrinsic {
                    intrinsic,
                    has_side_effect,
                    ..
                } => {
                    visitor.visit_constant(intrinsic)?;
                    visitor.visit_constant(has_side_effect)
                }
                I::ArrayGet { index, .. }
                | I::ArrayReplace { index, .. }
                | I::SourceLoc {
                    inline_count: index,
                    ..
                } => visitor.visit_constant(index),
                I::ComptimeIf { values, .. } => slice_iter!($($m)? values)
                    .try_for_each(|value| visitor.visit_constant(value)),
                I::ComptimeFor { binder, source, .. } => {
                    visitor.visit_param_ref(binder)?;
                    visitor.visit_sequence(source)
                }
                I::ComptimeAssert { value, message } => {
                    visitor.visit_constant(value)?;
                    $helpers::visit_opt_constant(visitor, opt_ref!($($m)? message))
                }
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
                | I::MakeRefPack { .. }
                | I::LoadIndirect { .. }
                | I::ArrayRepeat { .. }
                | I::InvalidateInteriors { .. }
                | I::ReadRef { .. }
                | I::Rebind { .. }
                | I::CopyValue { .. }
                | I::WriteRef { .. }
                | I::KeepAlive { .. }
                | I::StoreToMem { .. }
                | I::ScalarCast { .. }
                | I::BuiltinCast { .. }
                | I::PointerConvert { .. }
                | I::ArrayGep { .. }
                | I::IsRunInComptimeInterpreter { .. }
                | I::Select { .. }
                | I::SimdSelect { .. }
                | I::SimdExtract { .. }
                | I::SimdInsert { .. }
                | I::SimdReduce { .. }
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

        /// Visit a place's root type, its projection types, the alternatives
        /// its variant projections select, its symbolic field steps, then its
        /// designated type.
        pub fn $walk_place<V: $Visitor + ?Sized>(
            visitor: &mut V,
            place: &$($m)? MirPlace,
        ) -> Result<(), V::Error> {
            $helpers::visit_opt_ty(visitor, opt_ref!($($m)? place.root_ty), TySite::Place)?;
            for ty in &$($m)? place.projection_tys {
                visitor.visit_ty(ty, TySite::Place)?;
            }
            for (position, projection) in slice_iter!($($m)? place.proj).enumerate() {
                match projection {
                    Proj::Variant(index) => visitor.visit_variant_index(index)?,
                    Proj::FieldAt(_) => {
                        let base = match position.checked_sub(1) {
                            Some(previous) => place.projection_tys.get(previous),
                            None => place.root_ty.as_ref(),
                        };
                        visitor.visit_field_step(projection, base)?;
                    }
                    _ => {}
                }
            }
            $helpers::visit_opt_ty(visitor, opt_ref!($($m)? place.ty), TySite::Place)
        }

        mod $helpers {
            use super::{
                $Visitor, Const, MirParamArg, MirPlace, MirSubscriptCall, ParamArgSite,
                ParamDeclSite, SimdDtype, SimdWidth, Ty, TyArg, TySite,
            };

            pub(super) fn visit_opt_ty<V: $Visitor + ?Sized>(
                visitor: &mut V,
                ty: Option<&$($m)? Ty>,
                site: TySite,
            ) -> Result<(), V::Error> {
                ty.map_or(Ok(()), |ty| visitor.visit_ty(ty, site))
            }

            pub(super) fn visit_opt_constant<V: $Visitor + ?Sized>(
                visitor: &mut V,
                constant: Option<&$($m)? Const>,
            ) -> Result<(), V::Error> {
                constant.map_or(Ok(()), |constant| visitor.visit_constant(constant))
            }

            pub(super) fn visit_opt_place<V: $Visitor + ?Sized>(
                visitor: &mut V,
                place: Option<&$($m)? MirPlace>,
            ) -> Result<(), V::Error> {
                place.map_or(Ok(()), |place| visitor.visit_place(place))
            }

            pub(super) fn visit_places<V: $Visitor + ?Sized>(
                visitor: &mut V,
                places: &$($m)? [Option<MirPlace>],
            ) -> Result<(), V::Error> {
                slice_iter!($($m)? places)
                    .flatten()
                    .try_for_each(|place| visitor.visit_place(place))
            }

            pub(super) fn visit_param_args<V: $Visitor + ?Sized>(
                visitor: &mut V,
                args: &$($m)? [MirParamArg],
                site: ParamArgSite,
            ) -> Result<(), V::Error> {
                slice_iter!($($m)? args).try_for_each(|arg| visitor.visit_param_arg(arg, site))
            }

            pub(super) fn visit_ty_args<V: $Visitor + ?Sized>(
                visitor: &mut V,
                args: &$($m)? [TyArg],
            ) -> Result<(), V::Error> {
                slice_iter!($($m)? args).try_for_each(|arg| visitor.visit_ty_arg(arg))
            }

            /// A subscript call's error, result, and parameter types, its
            /// bracket arguments, its reference result's referent, then its
            /// contract.
            pub(super) fn walk_subscript_call<V: $Visitor + ?Sized>(
                visitor: &mut V,
                call: &$($m)? MirSubscriptCall,
            ) -> Result<(), V::Error> {
                visit_opt_ty(
                    visitor,
                    opt_ref!($($m)? call.raises).map(|raises| &$($m)? raises.ty),
                    TySite::Contract,
                )?;
                visitor.visit_ty(&$($m)? call.result_ty, TySite::Result)?;
                for argument in &$($m)? call.arguments {
                    visitor.visit_ty(&$($m)? argument.parameter_ty, TySite::SubscriptParameter)?;
                }
                visit_param_args(visitor, &$($m)? call.param_arg_regs, ParamArgSite::Subscript)?;
                visit_opt_ty(
                    visitor,
                    opt_ref!($($m)? call.reference_result)
                        .map(|reference| &$($m)? reference.referent),
                    TySite::Result,
                )?;
                visitor.visit_param_decls(
                    &$($m)? call.param_decls,
                    ParamDeclSite::SubscriptContract,
                )
            }

            /// A SIMD instruction's slots, visited as the vector type they
            /// build; a visited type that is no vector leaves them as they
            /// were.
            pub(super) fn visit_vector_slots<V: $Visitor + ?Sized>(
                visitor: &mut V,
                dtype: &$($m)? SimdDtype,
                width: &$($m)? SimdWidth,
            ) -> Result<(), V::Error> {
                let $($m)? built = Ty::simd(dtype.clone(), width.clone());

                visitor.visit_ty(&$($m)? built, TySite::VectorSlots)?;
                write_back_slots!($($m)? built, dtype, width);
                Ok(())
            }
        }
    };
}

type_visitor! {
    /// A read-only visitor over every type and compile-time parameter site
    /// of a MIR body, as MLIR's `AttrTypeWalker` is. A hook that fails stops
    /// the walk.
    trait TypeVisitor;
    walks walk_function, walk_blocks, walk_instruction, walk_place;
    regions instruction_regions;
    alternatives [Ty];
    helpers read_sites;
    mutability ()
}

type_visitor! {
    /// A mutating visitor over every type and compile-time parameter site
    /// of a MIR body, as MLIR's `AttrTypeReplacer` is. A hook that fails
    /// stops the walk.
    trait MutTypeVisitor;
    walks walk_function_mut, walk_blocks_mut, walk_instruction_mut, walk_place_mut;
    regions instruction_regions_mut;
    alternatives Vec<Ty>;
    helpers write_sites;
    mutability (mut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirTerm, Reg, TryLabel};

    /// Counts each hook's visits and the sites its types sit at.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Counter {
        sites: Vec<TySite>,
        ty_args: usize,
        variant_lists: usize,
        variant_indices: usize,
        field_steps: usize,
        lane_masks: usize,
        param_args: Vec<ParamArgSite>,
        param_decls: Vec<ParamDeclSite>,
        constants: usize,
        param_refs: usize,
        sequences: usize,
    }

    impl TypeVisitor for Counter {
        type Error = ();

        fn visit_ty(&mut self, _: &Ty, site: TySite) -> Result<(), ()> {
            self.sites.push(site);
            Ok(())
        }
        fn visit_ty_arg(&mut self, _: &TyArg) -> Result<(), ()> {
            self.ty_args += 1;
            Ok(())
        }
        fn visit_variant_alternatives(&mut self, _: &[Ty]) -> Result<(), ()> {
            self.variant_lists += 1;
            Ok(())
        }
        fn visit_variant_index(&mut self, _: &VariantIndex) -> Result<(), ()> {
            self.variant_indices += 1;
            Ok(())
        }
        fn visit_field_step(&mut self, _: &Proj, _: Option<&Ty>) -> Result<(), ()> {
            self.field_steps += 1;
            Ok(())
        }
        fn visit_lane_mask(&mut self, _: &LaneMask) -> Result<(), ()> {
            self.lane_masks += 1;
            Ok(())
        }
        fn visit_param_arg(&mut self, _: &MirParamArg, site: ParamArgSite) -> Result<(), ()> {
            self.param_args.push(site);
            Ok(())
        }
        fn visit_param_decls(&mut self, _: &[ParamDecl], site: ParamDeclSite) -> Result<(), ()> {
            self.param_decls.push(site);
            Ok(())
        }
        fn visit_constant(&mut self, _: &Const) -> Result<(), ()> {
            self.constants += 1;
            Ok(())
        }
        fn visit_param_ref(&mut self, _: &ParamRef) -> Result<(), ()> {
            self.param_refs += 1;
            Ok(())
        }
        fn visit_sequence(&mut self, _: &ComptimeSequence) -> Result<(), ()> {
            self.sequences += 1;
            Ok(())
        }
    }

    impl MutTypeVisitor for Counter {
        type Error = ();

        fn visit_ty(&mut self, _: &mut Ty, site: TySite) -> Result<(), ()> {
            self.sites.push(site);
            Ok(())
        }
        fn visit_ty_arg(&mut self, _: &mut TyArg) -> Result<(), ()> {
            self.ty_args += 1;
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
        fn visit_field_step(&mut self, _: &mut Proj, _: Option<&Ty>) -> Result<(), ()> {
            self.field_steps += 1;
            Ok(())
        }
        fn visit_lane_mask(&mut self, _: &mut LaneMask) -> Result<(), ()> {
            self.lane_masks += 1;
            Ok(())
        }
        fn visit_param_arg(&mut self, _: &mut MirParamArg, site: ParamArgSite) -> Result<(), ()> {
            self.param_args.push(site);
            Ok(())
        }
        fn visit_param_decls(
            &mut self,
            _: &mut [ParamDecl],
            site: ParamDeclSite,
        ) -> Result<(), ()> {
            self.param_decls.push(site);
            Ok(())
        }
        fn visit_constant(&mut self, _: &mut Const) -> Result<(), ()> {
            self.constants += 1;
            Ok(())
        }
        fn visit_param_ref(&mut self, _: &mut ParamRef) -> Result<(), ()> {
            self.param_refs += 1;
            Ok(())
        }
        fn visit_sequence(&mut self, _: &mut ComptimeSequence) -> Result<(), ()> {
            self.sequences += 1;
            Ok(())
        }
    }

    /// A body holding one instruction of each kind of site the walks reach.
    fn fixture() -> MirFunction {
        let int = || Ty::int();
        let mut place = MirPlace::root(0, Some(int()));
        place.project(Proj::Variant(VariantIndex::Known(0)), Ty::bool());
        let region = MirBlock {
            instrs: vec![MirInstr::MaterializeLiteral {
                dest: Reg(5),
                value: Reg(0),
                target: mojito_types::types::kgen_simd_ty(mojito_ast::ast::Dtype::Int, 1),
            }],
            term: MirTerm::TryYield(Vec::new()),
        };
        MirFunction {
            param_types: vec![int()],
            ret_ty: Some(int()),
            reg_types: std::iter::once((0, int())).collect(),
            body: MirBlock {
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
                        body: Box::new(region),
                        handler_args: Vec::new(),
                        handler: None,
                        else_args: Vec::new(),
                        orelse: None,
                    },
                    MirInstr::ComptimeIf {
                        results: Vec::new(),
                        values: vec![Const::Bool(true)],
                        arms: vec![MirBlock {
                            instrs: Vec::new(),
                            term: MirTerm::ComptimeYield(Vec::new()),
                        }],
                        orelse: Box::new(MirBlock {
                            instrs: Vec::new(),
                            term: MirTerm::ComptimeYield(Vec::new()),
                        }),
                    },
                ],
                term: MirTerm::Return(None),
            },
            ..MirFunction::default()
        }
    }

    /// A method call's error, reference result, arguments, slots, and
    /// contract, the sites a read-only client must not miss.
    fn method_call() -> MirInstr {
        MirInstr::MethodCall {
            dest: Reg(6),
            recv: Reg(0),
            method: "get".into(),
            resolved: Some("S.get".into()),
            raises: None,
            reference_result: Some(Box::new(mojito_types::origin::RefTy {
                referent: Ty::int(),
                origin: mojito_types::origin::Origin::Untracked { mutable: false },
                mutability: mojito_types::origin::Mutability::Immutable,
            })),
            result_adapter: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_place: None,
            recv_writes: false,
            receiver_convention: None,
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: vec![MirParamArg {
                name: None,
                value: None,
                expr: None,
            }],
            param_decls: Vec::new(),
            instantiated_args: vec![TyArg::Ty(Ty::int())],
            spread: None,
            refined_bound: None,
        }
    }

    #[test]
    fn visit_walk_reaches_every_site_kind() {
        let mut function = fixture();
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
        assert_eq!(counter.constants, 2);
        assert_eq!(counter.variant_lists, 1);
        assert_eq!(counter.variant_indices, 2);
        assert_eq!(counter.lane_masks, 1);
    }

    /// The read-only walk reaches exactly the sites the mutating walk does,
    /// in the same order.
    #[test]
    fn read_only_walk_matches_the_mutating_walk() {
        let mut function = fixture();
        function.body.instrs.push(method_call());
        let mut read = Counter::default();
        walk_function(&mut read, &function).unwrap();
        let mut written = Counter::default();
        walk_function_mut(&mut written, &mut function).unwrap();
        assert_eq!(read, written);
        assert_eq!(read.sites.last(), Some(&TySite::Result));
        assert_eq!(read.ty_args, 1);
        assert_eq!(read.param_args, [ParamArgSite::Call]);
        assert_eq!(read.param_decls, [ParamDeclSite::MethodContract]);
    }
}
