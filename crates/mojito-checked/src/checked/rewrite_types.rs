//! A rewrite of every type the checked program records.
//!
//! Upstream's `liftClosure` replaces each reference to a closure's nested
//! storage struct throughout the front-end IR with one whole-IR type
//! replacement (MLIR's `AttrTypeReplacer`). [`RewriteTypes`] is that walk
//! over the checked tables and the program's own maps: every type, type
//! argument, compile-time value, binder, and parameter expression they
//! hold is rebuilt through one [`TyRewrite`]. The walks destructure every
//! field and variant, so a new one must say whether it holds a type.
//!
//! The program's overload and constant tables are not walked: they hold
//! declared signatures and package declarations, whose types are spelled.

use super::{
    CheckedAugmentedSubscript, CheckedCallArgument, CheckedCallArgumentBoundary,
    CheckedCallBoundary, CheckedCallContract, CheckedCallValueAdjustment, CheckedCapture,
    CheckedClosureStorage, CheckedClosureUses, CheckedCompareLink, CheckedComprehensionBinding,
    CheckedComprehensionClause, CheckedComprehensionUnpack, CheckedDeclaration, CheckedDef,
    CheckedDefaultWrapper, CheckedElementInvocation, CheckedExpr, CheckedField,
    CheckedIterationBinding, CheckedIteratorCall, CheckedMethod, CheckedOp, CheckedParamArg,
    CheckedProgram, CheckedSequenceTarget, CheckedSignature, CheckedSliceLiteral, CheckedStmt,
    CheckedStmtOp, CheckedStorageField, CheckedStruct, CheckedSubscriptWriteBack, CheckedTables,
    CheckedTarget, CheckedTargetKind, CheckedTupleUnpackElement, ComptimeIteration,
    ConformanceFacts, DeclarationEffect, EffectFacts, GenericInstantiation, IterationProtocol,
    MethodInstantiation, OmittedSliceOperand, ReflectedFieldStep, SemanticAdjustment,
    StructInstantiation,
};
use crate::declaration::{
    ParameterizedAssociatedType, ParameterizedAssociatedValue, TraitDeclaration, TraitRequirement,
};
use mojito_types::ct::CtValue;
use mojito_types::origin::RefTy;
use mojito_types::param_expr::ParamExpr;
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind, TyRewrite};
use std::collections::HashMap;

/// A checked value whose recorded types a [`TyRewrite`] rebuilds in
/// place. A type the rewrite cannot rebuild stays as it was.
pub trait RewriteTypes {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite);
}

impl CheckedProgram {
    /// Rebuild every type the program records through `rewrite`
    /// ([`RewriteTypes`]). The check's tables must be unshared.
    pub(crate) fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            warnings: _,
            compatibility_overload_targets: _,
            generic_instantiations,
            method_instantiations,
            struct_instantiations,
            compatibility_implicit_conversions: _,
            implicit_conversion_types,
            checked_types,
            generic_parameters,
            tables,
            explicit_destroy_types: _,
            struct_destructors: _,
            struct_modules: _,
            conformances,
            declaration_effects,
            overloads: _,
            constants: _,
            default_wrappers,
            trait_members: _,
        } = self;
        generic_instantiations.rewrite_types(rewrite);
        method_instantiations.rewrite_types(rewrite);
        struct_instantiations.rewrite_types(rewrite);
        implicit_conversion_types.rewrite_types(rewrite);
        checked_types.rewrite_types(rewrite);
        generic_parameters.rewrite_types(rewrite);
        std::sync::Arc::get_mut(tables)
            .expect("the check's tables are unshared while its types are rewritten")
            .rewrite_types(rewrite);
        conformances.rewrite_types(rewrite);
        declaration_effects.rewrite_types(rewrite);
        default_wrappers.rewrite_types(rewrite);
    }
}

impl RewriteTypes for Ty {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Ok(rebuilt) = mojito_types::types::rewrite_ty(self, rewrite) {
            *self = rebuilt;
        }
    }
}

impl RewriteTypes for TyArg {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Ok(rebuilt) =
            mojito_types::types::rewrite_tyargs(std::slice::from_ref(self), rewrite)
            && let Some(rebuilt) = rebuilt.into_iter().next()
        {
            *self = rebuilt;
        }
    }
}

impl RewriteTypes for CtValue {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Ok(rebuilt) = mojito_types::types::rewrite_value(self, rewrite) {
            *self = rebuilt;
        }
    }
}

impl RewriteTypes for ParamDecl {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Ok(rebuilt) = mojito_types::types::rewrite_decl(self, rewrite) {
            *self = rebuilt;
        }
    }
}

impl RewriteTypes for ParamExpr {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Ok(rebuilt) = rewrite.expr(self) {
            *self = rebuilt;
        }
    }
}

impl RewriteTypes for RefTy {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let mut ty = Ty::from(TyKind::Ref(self.clone()));
        ty.rewrite_types(rewrite);
        if let TyKind::Ref(rebuilt) = ty.kind() {
            *self = rebuilt.clone();
        }
    }
}

impl<T: RewriteTypes> RewriteTypes for Option<T> {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        if let Some(value) = self {
            value.rewrite_types(rewrite);
        }
    }
}

impl<T: RewriteTypes> RewriteTypes for Box<T> {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        T::rewrite_types(self, rewrite);
    }
}

impl<T: RewriteTypes> RewriteTypes for Vec<T> {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        for value in self {
            value.rewrite_types(rewrite);
        }
    }
}

impl<T: RewriteTypes, const N: usize> RewriteTypes for [T; N] {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        for value in self {
            value.rewrite_types(rewrite);
        }
    }
}

/// A map's keys name sites, never types.
impl<K, V: RewriteTypes, S> RewriteTypes for HashMap<K, V, S> {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        for value in self.values_mut() {
            value.rewrite_types(rewrite);
        }
    }
}

/// A named entry: an associated type or value.
impl<T: RewriteTypes> RewriteTypes for (String, T) {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        self.1.rewrite_types(rewrite);
    }
}

impl RewriteTypes for EffectFacts {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            raises,
            may_suspend: _,
            diverges: _,
        } = self;
        raises.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCallArgument {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            source: _,
            parameter_ty,
            requires_place: _,
            convention: _,
        } = self;
        parameter_ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCallValueAdjustment {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::ImplicitConversion {
                target: _,
                instantiated_args,
            } => {
                instantiated_args.rewrite_types(rewrite);
            }
            Self::MaterializeLiteral { target } => {
                target.rewrite_types(rewrite);
            }
            Self::SplatScalar { target } => {
                target.rewrite_types(rewrite);
            }
            Self::SimdFromLength { target } => {
                target.rewrite_types(rewrite);
            }
            Self::ResolveCallable { target: _ } | Self::IndexNormalization { target: _ } => {}
        }
    }
}

impl RewriteTypes for CheckedCallArgumentBoundary {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            source: _,
            value_source: _,
            adjustments,
            invalidations: _,
        } = self;
        adjustments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCallBoundary {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            arguments,
            invalidations: _,
        } = self;
        arguments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCallContract {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            target: _,
            raises,
            result_ty,
            result_adapter: _,
            receiver_requires_place: _,
            receiver_elided: _,
            receiver_convention: _,
            arguments,
            captures: _,
            reference_result,
            parameter_arguments: _,
            param_decls,
            boundary,
            refined_bound: _,
        } = self;
        raises.rewrite_types(rewrite);
        result_ty.rewrite_types(rewrite);
        arguments.rewrite_types(rewrite);
        reference_result.rewrite_types(rewrite);
        param_decls.rewrite_types(rewrite);
        boundary.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedAugmentedSubscript {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            getter,
            setter,
            inplace,
            operand_ty,
            result_ty,
        } = self;
        getter.rewrite_types(rewrite);
        setter.rewrite_types(rewrite);
        inplace.rewrite_types(rewrite);
        operand_ty.rewrite_types(rewrite);
        result_ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedSubscriptWriteBack {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            getter,
            setter,
            value_keyword: _,
            element,
        } = self;
        getter.rewrite_types(rewrite);
        setter.rewrite_types(rewrite);
        element.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedElementInvocation {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            getter,
            callable,
            target: _,
            raises,
        } = self;
        getter.rewrite_types(rewrite);
        callable.rewrite_types(rewrite);
        raises.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedIterationBinding {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            mode: _,
            action: _,
            yielded_ty,
            binding_ty,
            mutable: _,
        } = self;
        yielded_ty.rewrite_types(rewrite);
        binding_ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedIteratorCall {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            target: _,
            result_ty,
            reference_result,
            raises,
            result_adapter: _,
        } = self;
        result_ty.rewrite_types(rewrite);
        reference_result.rewrite_types(rewrite);
        raises.rewrite_types(rewrite);
    }
}

impl RewriteTypes for GenericInstantiation {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            callee: _,
            parameter_names: _,
            parameter_types: _,
            variadic: _,
            arguments,
            folded_arguments: _,
        } = self;
        arguments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for MethodInstantiation {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            owner: _,
            owner_arguments,
            method: _,
            parameter_names: _,
            overload: _,
            arguments,
            folded_arguments: _,
        } = self;
        owner_arguments.rewrite_types(rewrite);
        arguments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for StructInstantiation {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            template: _,
            arguments,
        } = self;
        arguments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for IterationProtocol {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            mode: _,
            binding,
            borrowed_origin: _,
            yield_interior: _,
            source_retained: _,
            prepare: _,
            prepare_raises,
            iterator_ty,
            next,
            exhaustion,
        } = self;
        binding.rewrite_types(rewrite);
        prepare_raises.rewrite_types(rewrite);
        iterator_ty.rewrite_types(rewrite);
        next.rewrite_types(rewrite);
        exhaustion.rewrite_types(rewrite);
    }
}

impl RewriteTypes for ComptimeIteration {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            binder: _,
            initial,
            has_next,
            next_iter,
            next_value,
        } = self;
        initial.rewrite_types(rewrite);
        has_next.rewrite_types(rewrite);
        next_iter.rewrite_types(rewrite);
        next_value.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCompareLink {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            target: _,
            call,
            instantiated_args,
            left_adjustments,
            right_adjustments,
        } = self;
        call.rewrite_types(rewrite);
        instantiated_args.rewrite_types(rewrite);
        left_adjustments.rewrite_types(rewrite);
        right_adjustments.rewrite_types(rewrite);
    }
}

impl RewriteTypes for ReflectedFieldStep {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::At(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Named(_) => {}
        }
    }
}

impl RewriteTypes for SemanticAdjustment {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::Rebind { operand, dest } => {
                operand.rewrite_types(rewrite);
                dest.rewrite_types(rewrite);
            }
            Self::ConstructType { ty } => {
                ty.rewrite_types(rewrite);
            }
            Self::SelectedCall(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::AugmentedSubscript(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::SubscriptWriteBack(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::AugmentedInPlace {
                call,
                place_reference,
            } => {
                call.rewrite_types(rewrite);
                place_reference.rewrite_types(rewrite);
            }
            Self::ParameterizedMethodCall { param_decls } => {
                param_decls.rewrite_types(rewrite);
            }
            Self::FieldInvocation { callable } => {
                callable.rewrite_types(rewrite);
            }
            Self::ElementInvocation(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::InstantiatedCallableContract {
                contract,
                arguments,
            } => {
                contract.rewrite_types(rewrite);
                arguments.rewrite_types(rewrite);
            }
            Self::ConversionResultType(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ConversionRaises(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ConversionInstantiatedArguments(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::MaterializeLiteral(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::SplatScalar(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::SimdFromLength(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ReferenceResult { reference } => {
                reference.rewrite_types(rewrite);
            }
            Self::ReflectedField(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::TupleUnpack { elements } => {
                elements.rewrite_types(rewrite);
            }
            Self::InstantiatedArguments(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::CompareLinks(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ComptimeCondition {
                constraint: _,
                value,
            } => {
                value.rewrite_types(rewrite);
            }
            Self::ComptimeIteration(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ComptimeDisplay {
                construction: _,
                sequence,
            } => {
                sequence.rewrite_types(rewrite);
            }
            Self::ComptimeApplication {
                operation: _,
                application,
            } => {
                application.rewrite_types(rewrite);
            }
            Self::Iterate(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::ParamListAddress { values } => {
                values.rewrite_types(rewrite);
            }
            Self::SimdLength { length } => {
                length.rewrite_types(rewrite);
            }
            Self::ParamValue {
                value,
                materialized: _,
                literal_target,
            } => {
                value.rewrite_types(rewrite);
                literal_target.rewrite_types(rewrite);
            }
            Self::ConstructVariant {
                alternatives,
                index: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::ConstructVariantInitWith {
                alternatives,
                index: _,
                payload,
            } => {
                alternatives.rewrite_types(rewrite);
                payload.rewrite_types(rewrite);
            }
            Self::ConstructCollection {
                target,
                insert: _,
                constructor: _,
                marker: _,
                materialized: _,
                literal: _,
            } => {
                target.rewrite_types(rewrite);
            }
            Self::ConstructArrayLiteral {
                target,
                constructor: _,
                materialized: _,
            } => {
                target.rewrite_types(rewrite);
            }
            Self::VariantTag { alternatives } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::VariantIs {
                alternatives,
                index: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::VariantProject {
                alternatives,
                index: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::VariantSet {
                alternatives,
                index: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::VariantTake {
                alternatives,
                index: _,
                checked: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::VariantReplace {
                alternatives,
                input_index: _,
                output_index: _,
                checked: _,
            } => {
                alternatives.rewrite_types(rewrite);
            }
            Self::LlvmIntrinsic {
                intrinsic,
                has_side_effect,
                spread: _,
            } => {
                intrinsic.rewrite_types(rewrite);
                has_side_effect.rewrite_types(rewrite);
            }
            Self::KgenStructExtract { index } => {
                index.rewrite_types(rewrite);
            }
            Self::PopArrayGet { index } => {
                index.rewrite_types(rewrite);
            }
            Self::PopArrayReplace { index } => {
                index.rewrite_types(rewrite);
            }
            Self::PointerStorageTake { element } => {
                element.rewrite_types(rewrite);
            }
            Self::PointerStorageDestroy { element } => {
                element.rewrite_types(rewrite);
            }
            Self::UninitStorageMake { element, init: _ } => {
                element.rewrite_types(rewrite);
            }
            Self::UninitStorageWrite { element } => {
                element.rewrite_types(rewrite);
            }
            Self::UninitStorageTake { element } => {
                element.rewrite_types(rewrite);
            }
            Self::UninitStorageDestroy { element } => {
                element.rewrite_types(rewrite);
            }
            Self::PointerWrite { element, copy: _ } => {
                element.rewrite_types(rewrite);
            }
            Self::SliceDescriptors {
                descriptors,
                set_value_keyword: _,
            } => {
                descriptors.rewrite_types(rewrite);
            }
            Self::ResolveCallable(_)
            | Self::ConstructTypeParam { param: _ }
            | Self::ConstructPackElement { pack: _ }
            | Self::BuiltinCopyConstruction
            | Self::ConvertedArgument
            | Self::ScalarCast(_)
            | Self::IndexNormalization { target: _ }
            | Self::ImplicitConversion(_)
            | Self::BorrowShared
            | Self::BorrowMutable
            | Self::BorrowRefArguments {
                arguments: _,
                materialized: _,
            }
            | Self::MaterializeBorrowSource {
                owner: _,
                mutable: _,
            }
            | Self::BorrowConversionSource { mutable: _ }
            | Self::BorrowViewResult {
                materialized: _,
                interior: _,
            }
            | Self::CopyPlaceValue
            | Self::RetainCallPlace
            | Self::BorrowReadArgument
            | Self::ReadTemporaryArgument
            | Self::ForwardedArgument
            | Self::CallableCaptureAccesses {
                captures: _,
                materialized: _,
            }
            | Self::FoldedParameterArguments(_)
            | Self::ImplicitlyCopyConsumingReceiver
            | Self::ReflectedOperator
            | Self::InvertedWrite
            | Self::InvertedReprWrite
            | Self::ReceiverFromFirstArgument { inner: _ }
            | Self::Truthiness
            | Self::RegisterPassableResult
            | Self::RegisterPassableRead
            | Self::IndirectResult
            | Self::ConversionIndirectResult
            | Self::ConversionReadsSource
            | Self::ComptimeConditional
            | Self::Transfer(_)
            | Self::ExplicitDestroy
            | Self::ConstructSimd { dtype: _, width: _ }
            | Self::DtypeConstant { dtype: _ }
            | Self::SimdShuffle { mask: _ }
            | Self::MarkInitialized
            | Self::MarkDestroyed
            | Self::MlirOp(_)
            | Self::InstanceAssertion(_)
            | Self::PopSimdShuffle { mask: _ }
            | Self::PopArrayRepeat
            | Self::PopArrayGep
            | Self::SpelledConstruction { construction: _ }
            | Self::PointerToPlace {
                mutable: _,
                materialized: _,
            }
            | Self::PointerOffset { materialized: _ }
            | Self::PointerOriginCast {
                origin: _,
                materialized: _,
            }
            | Self::PointerConvert {
                origin: _,
                materialized: _,
            }
            | Self::InteriorReference { origin: _ }
            | Self::InvalidateInteriors { invalidations: _ } => {}
        }
    }
}

impl RewriteTypes for CheckedSliceLiteral {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            target,
            constructor: _,
            omitted,
            indirect_result: _,
        } = self;
        target.rewrite_types(rewrite);
        omitted.rewrite_types(rewrite);
    }
}

impl RewriteTypes for OmittedSliceOperand {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self { conversion } = self;
        conversion.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedTupleUnpackElement {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            ty,
            accessor: _,
            param_decls,
            temporary: _,
            reference,
            carries_loans: _,
            elements: _,
        } = self;
        ty.rewrite_types(rewrite);
        param_decls.rewrite_types(rewrite);
        reference.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedExpr {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            id: _,
            op,
            span: _,
            source: _,
            syntax_id: _,
            ty,
            place_ty,
            binding_ty,
            category: _,
            binding: _,
            contextual_base: _,
            effects,
            adjustments,
            comprehension_bindings,
            call_transfers: _,
        } = self;
        op.rewrite_types(rewrite);
        ty.rewrite_types(rewrite);
        place_ty.rewrite_types(rewrite);
        binding_ty.rewrite_types(rewrite);
        effects.rewrite_types(rewrite);
        adjustments.rewrite_types(rewrite);
        comprehension_bindings.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedOp {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::TypeValue {
                ty,
                applications: _,
            } => {
                ty.rewrite_types(rewrite);
            }
            Self::Call {
                name: _,
                param_args,
                args: _,
                kwargs: _,
            } => {
                param_args.rewrite_types(rewrite);
            }
            Self::Invoke {
                callee: _,
                param_args,
                args: _,
                kwargs: _,
            } => {
                param_args.rewrite_types(rewrite);
            }
            Self::TypeApply { name: _, args } => {
                args.rewrite_types(rewrite);
            }
            Self::Comprehension {
                kind: _,
                key: _,
                value: _,
                clauses,
            } => {
                clauses.rewrite_types(rewrite);
            }
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::Str(_)
            | Self::None
            | Self::Uninitialized
            | Self::EmptySubscript
            | Self::IsRunInComptimeInterpreter
            | Self::Identifier(_)
            | Self::Prefix(_, _)
            | Self::Infix(_, _, _)
            | Self::Member {
                object: _,
                field: _,
            }
            | Self::MethodCall {
                object: _,
                method: _,
                args: _,
                kwargs: _,
            }
            | Self::Index {
                object: _,
                index: _,
            }
            | Self::Spread(_)
            | Self::Transfer(_)
            | Self::ListLit(_)
            | Self::BraceLit(_)
            | Self::InitializerList { args: _, kwargs: _ }
            | Self::Lambda { def: _ }
            | Self::TupleLit(_)
            | Self::Named {
                target: _,
                value: _,
            }
            | Self::IfExpr {
                cond: _,
                then_branch: _,
                else_branch: _,
            }
            | Self::Compare { first: _, rest: _ }
            | Self::Slice {
                object: _,
                lower: _,
                upper: _,
                step: _,
            }
            | Self::MultiIndex { object: _, args: _ }
            | Self::TString { parts: _, raw: _ } => {}
        }
    }
}

impl RewriteTypes for CheckedParamArg {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::Attribute {
                value,
                applications: _,
            } => {
                value.rewrite_types(rewrite);
            }
            Self::Value(_) | Self::Named { name: _, value: _ } => {}
        }
    }
}

impl RewriteTypes for CheckedComprehensionClause {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::For {
                var: _,
                binding: _,
                iter: _,
                unpack,
            } => {
                unpack.rewrite_types(rewrite);
            }
            Self::If(_) => {}
        }
    }
}

impl RewriteTypes for CheckedComprehensionUnpack {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self { target, value: _ } = self;
        target.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedStmt {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            id: _,
            op,
            span: _,
            module: _,
            syntax_id: _,
            binding: _,
            binding_ty,
            declaration: _,
        } = self;
        op.rewrite_types(rewrite);
        binding_ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedStmtOp {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::Destructure { target, value: _ } => {
                target.rewrite_types(rewrite);
            }
            Self::Def(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Struct(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Comptime {
                name: _,
                where_clauses: _,
                value: _,
                denotes,
            } => {
                denotes.rewrite_types(rewrite);
            }
            Self::ComptimeAssert {
                cond: _,
                message: _,
                denotes,
                fails: _,
            } => {
                denotes.rewrite_types(rewrite);
            }
            Self::VarDecl {
                name: _,
                applications: _,
                value: _,
            }
            | Self::RefDecl { name: _, value: _ }
            | Self::Assign { name: _, value: _ }
            | Self::AugAssign {
                place: _,
                op: _,
                value: _,
            }
            | Self::SetPlace { place: _, value: _ }
            | Self::Trait(_)
            | Self::ComptimeIf {
                branches: _,
                orelse: _,
            }
            | Self::ComptimeFor {
                var: _,
                iter: _,
                body: _,
            }
            | Self::ComptimeMatch {
                anchor: _,
                cases: _,
            }
            | Self::If {
                branches: _,
                orelse: _,
            }
            | Self::While {
                cond: _,
                body: _,
                orelse: _,
            }
            | Self::Match {
                anchor: _,
                cases: _,
                emission: _,
            }
            | Self::For {
                var: _,
                binding: _,
                iter: _,
                body: _,
                orelse: _,
            }
            | Self::Return(_)
            | Self::Raise(_)
            | Self::Import { path: _, alias: _ }
            | Self::FromImport {
                level: _,
                path: _,
                names: _,
            }
            | Self::With { items: _, body: _ }
            | Self::Try {
                body: _,
                except: _,
                orelse: _,
                finalbody: _,
            }
            | Self::Scope(_)
            | Self::Pass
            | Self::Break
            | Self::Continue
            | Self::Expr(_) => {}
        }
    }
}

impl RewriteTypes for CheckedDef {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            symbol: _,
            signature,
            params: _,
            positional_only: _,
            keyword_only: _,
            raises: _,
            where_clauses: _,
            body: _,
            captures,
            capture_list: _,
            storage,
            inline: _,
            unavailable: _,
        } = self;
        signature.rewrite_types(rewrite);
        captures.rewrite_types(rewrite);
        storage.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedSignature {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            parameters,
            params,
            result,
            effect,
        } = self;
        parameters.rewrite_types(rewrite);
        params.rewrite_types(rewrite);
        result.rewrite_types(rewrite);
        effect.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedStruct {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            conforms: _,
            conformance_conditions: _,
            where_clauses: _,
            fields,
            associated: _,
            methods,
            fieldwise_init: _,
        } = self;
        fields.rewrite_types(rewrite);
        methods.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedField {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            ty,
            fieldwise_convention: _,
            fieldwise_ref: _,
        } = self;
        ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedMethod {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            registered: _,
            symbol: _,
            signature,
            has_self: _,
            self_convention: _,
            params: _,
            positional_only: _,
            keyword_only: _,
            raises: _,
            where_clauses: _,
            body: _,
            provenance: _,
            inline: _,
        } = self;
        signature.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedTarget {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            kind,
            span: _,
            syntax_id: _,
        } = self;
        kind.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedTargetKind {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::Declare {
                name: _,
                mode: _,
                binding: _,
                binding_ty,
            } => {
                binding_ty.rewrite_types(rewrite);
            }
            Self::Tuple(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Sequence(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Discard | Self::Place(_) => {}
        }
    }
}

impl RewriteTypes for CheckedSequenceTarget {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        match self {
            Self::Element(f0) => {
                f0.rewrite_types(rewrite);
            }
            Self::Star(f0) => {
                f0.rewrite_types(rewrite);
            }
        }
    }
}

impl RewriteTypes for CheckedComprehensionBinding {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            owner: _,
            ty,
            plan,
            deinitable: _,
        } = self;
        ty.rewrite_types(rewrite);
        plan.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedDeclaration {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            id: _,
            kind: _,
            name: _,
            location: _,
            binding: _,
            ty,
            children: _,
        } = self;
        ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedClosureStorage {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            fields,
            contract,
            writes: _,
            copyable: _,
            passability: _,
            uses,
        } = self;
        fields.rewrite_types(rewrite);
        contract.rewrite_types(rewrite);
        uses.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedClosureUses {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            binders,
            parameters,
            method: _,
        } = self;
        binders.rewrite_types(rewrite);
        parameters.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedStorageField {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            ty,
            reference: _,
        } = self;
        ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedCapture {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            binding: _,
            ty,
            kind: _,
            origins: _,
        } = self;
        ty.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedTables {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            expressions,
            statements,
            declarations,
            roots: _,
            wrapper_roots: _,
        } = self;
        expressions.rewrite_types(rewrite);
        statements.rewrite_types(rewrite);
        declarations.rewrite_types(rewrite);
    }
}

impl RewriteTypes for CheckedDefaultWrapper {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            module: _,
            declaration: _,
            method: _,
            method_index: _,
            lowered_name: _,
            member: _,
            self_convention: _,
            params: _,
            positional_only: _,
            keyword_only: _,
            signature,
            body: _,
        } = self;
        signature.rewrite_types(rewrite);
    }
}

impl RewriteTypes for ConformanceFacts {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            traits,
            structs: _,
            declared: _,
            associated,
            parameterized_associated,
            associated_values,
            parameterized_values,
            fieldwise_conventions: _,
            fieldwise_ref_params: _,
            witness_tables: _,
        } = self;
        traits.rewrite_types(rewrite);
        associated.rewrite_types(rewrite);
        parameterized_associated.rewrite_types(rewrite);
        associated_values.rewrite_types(rewrite);
        parameterized_values.rewrite_types(rewrite);
    }
}

impl RewriteTypes for DeclarationEffect {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            raises: _,
            error,
            returns_reference: _,
            param_writes: _,
            availability: _,
        } = self;
        error.rewrite_types(rewrite);
    }
}

impl RewriteTypes for ParameterizedAssociatedType {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            params: _,
            template,
        } = self;
        template.rewrite_types(rewrite);
    }
}

impl RewriteTypes for ParameterizedAssociatedValue {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            params,
            body,
        } = self;
        params.rewrite_types(rewrite);
        body.rewrite_types(rewrite);
    }
}

impl RewriteTypes for TraitDeclaration {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            refines: _,
            requirements,
        } = self;
        requirements.rewrite_types(rewrite);
    }
}

impl RewriteTypes for TraitRequirement {
    fn rewrite_types(&mut self, rewrite: &mut dyn TyRewrite) {
        let Self {
            name: _,
            arity: _,
            dispatch: _,
            param_names: _,
            param_types,
            param_conventions: _,
            variadic_index: _,
            required: _,
            positional_only: _,
            keyword_only: _,
            variadic,
            kw_variadic,
            variadic_convention: _,
        } = self;
        param_types.rewrite_types(rewrite);
        variadic.rewrite_types(rewrite);
        kw_variadic.rewrite_types(rewrite);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CheckedNodeId, ValueCategory};
    use super::*;

    /// Rewrites `Int` to `Bool`.
    struct IntToBool;

    impl TyRewrite for IntToBool {
        fn whole(&mut self, ty: &Ty) -> Option<Ty> {
            (*ty == Ty::int()).then(Ty::bool)
        }

        fn expr(
            &mut self,
            expr: &ParamExpr,
        ) -> Result<ParamExpr, mojito_types::param_expr::ParamError> {
            Ok(expr.clone())
        }
    }

    #[test]
    fn rewrites_a_node_type_and_its_adjustments() {
        let mut node = CheckedExpr {
            id: CheckedNodeId::default(),
            op: CheckedOp::Bool(true),
            span: (3, 7),
            source: None,
            syntax_id: mojito_common::token::SyntaxId(0),
            ty: Some(Ty::int()),
            place_ty: None,
            binding_ty: Some(TyKind::Tuple(vec![Ty::int()]).into()),
            category: ValueCategory::Place,
            binding: None,
            contextual_base: None,
            effects: EffectFacts::default(),
            adjustments: vec![SemanticAdjustment::MaterializeLiteral(Ty::int())],
            comprehension_bindings: Vec::new(),
            call_transfers: Vec::new(),
        };
        node.rewrite_types(&mut IntToBool);
        assert_eq!(node.ty, Some(Ty::bool()));
        assert_eq!(
            node.binding_ty,
            Some(TyKind::Tuple(vec![Ty::bool()]).into())
        );
        assert!(matches!(
            node.adjustments.as_slice(),
            [SemanticAdjustment::MaterializeLiteral(ty)] if *ty == Ty::bool()
        ));
    }
}
