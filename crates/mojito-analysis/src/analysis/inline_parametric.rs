//! `inline-parametric`: upstream's `InlineParametric`, which its
//! pre-elaboration pipeline runs to open its second round, so that a call
//! to an `@always_inline("nodebug")` or `@always_inline("builtin")`
//! function holds the callee's body instead of naming an instance of it.
//!
//! The callee's body is its finished one, drops already inserted, so its
//! drops come with it. Every register it brings takes the call's location,
//! which is what `nodebug` asks for: the inlined code steps as the call.
//! The pin inlines such a callee at any size, and refuses one that lies on a
//! cycle of such calls or holds a `try`.
//!
//! A callee this pass inlines is a function, or a method of a
//! non-parametric struct, whose own compile-time parameters the call binds
//! in full; it takes its arguments positionally into `read` or owned
//! parameters, does not raise, and returns a value. The call's compile-time
//! arguments replace the callee's binders through every type and parameter
//! site of the body, as upstream substitutes them through MLIR's
//! `AttrTypeReplacer`; an argument over the caller's own binders stays
//! symbolic, for the elaborator to fold in each instance of the caller. A
//! read of a value binder becomes its argument as a constant, upstream's
//! `kgen.param.constant`. A binder that is callable or variadic, a `where`
//! clause, or a binding the body's sites cannot hold (`T()` at a concrete
//! `T`, a conformance test of a concrete binding) keeps the call.
//!
//! An owned parameter, or a `read` one of a type no destructor tears down,
//! is a fresh slot the argument initializes; any other `read` parameter is
//! the caller's place the argument was loaded from, as upstream's inliner
//! substitutes the borrowed address for the callee's argument. A call of
//! any other shape stays a call.

use super::pre_elaboration::{CallGraphPass, Callees};
use mojito_checked::checked::ComptimeSequence;
use mojito_checked::declaration::InlineLevel;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::{instruction_places, instruction_places_mut};
use mojito_mir::mir::visit::{
    MutTypeVisitor, ParamArgSite, ParamDeclSite, TySite, walk_function_mut,
};
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg,
    MirPlace, MirTerm, Reg, UseMode, has_no_destructor, instruction_regions_mut,
    instruction_regs_mut, instruction_slots, renumber_block_slots, terminator_regs_mut,
    terminator_targets_mut,
};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{ParamBindings, ParamContext, ParamError, ParamExpr, ParamRef};
use mojito_types::types::{
    GenericConstraint, LaneMask, ParamDecl, Ty, TyArg, TyKind, VariantIndex,
    replace_argument_parameters, replace_constraint_parameters, replace_parameters,
    replace_value_parameters,
};
use std::borrow::Cow;

pub const INLINE_PARAMETRIC: CallGraphPass = CallGraphPass {
    name: "inline-parametric",
    follows: is_inlined_before_elaboration,
    run: inline_parametric,
};

/// Whether upstream inlines every call of a function so declared.
///
/// It inlines an `@always_inline("nodebug")` or `@always_inline("builtin")`
/// callee before elaboration at any size; a plain `@always_inline` callee
/// only below a size bound.
pub const fn is_inlined_before_elaboration(declaration: &MirFunctionDeclaration) -> bool {
    matches!(
        declaration.inline,
        InlineLevel::NoDebug | InlineLevel::Builtin
    )
}

/// Inline into `function` every call [`Callees::body`] answers whose shape
/// this pass inlines, in its `try` regions too; returns whether it inlined
/// one.
pub fn inline_parametric(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) -> bool {
    let mut blocks = std::mem::take(&mut function.blocks);
    let mut inliner = Inliner {
        function,
        declarations,
        callees,
    };
    let changed = inliner.block_list(&mut blocks);
    function.blocks = blocks;
    changed
}

/// The caller, its blocks taken out, and where its callees come from.
struct Inliner<'f, 'c> {
    function: &'f mut MirFunction,
    declarations: &'c MirDeclarations,
    callees: &'c Callees<'c>,
}

/// Where an inlined call stands: its block, its index there, and the
/// register it defines.
struct CallSite {
    block: usize,
    index: usize,
    dest: Reg,
}

/// How an inlined parameter's slot is bound.
enum Binding {
    /// A fresh slot, initialized from the argument register.
    Value(Reg),
    /// The caller's place the argument was loaded from.
    Place(MirPlace),
}

/// What an inlined call returns through.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Returned {
    /// Nothing: the callee returns `None`.
    None,
    /// The callee's one returned register, which becomes the call's.
    Register(Reg),
    /// A slot every return stores to, which the call's register moves from.
    Slot,
}

impl<'c> Inliner<'_, 'c> {
    /// Inline every inlinable call of `list`, a block list of the caller
    /// whose ids are its own; returns whether it inlined one.
    fn block_list(&mut self, list: &mut Vec<MirBlock>) -> bool {
        let mut changed = false;
        let mut block = 0;
        while block < list.len() {
            let mut index = 0;
            while index < list[block].instrs.len() {
                for region in instruction_regions_mut(&mut list[block].instrs[index]) {
                    changed |= self.block_list(region);
                }
                if let Some((dest, callee, bindings)) = self.inlinable(&list[block], index)
                    && let Some(blocks) = self.inlined_blocks(&callee, &bindings)
                {
                    let site = CallSite { block, index, dest };
                    self.splice(list, &site, &callee, &bindings, blocks);
                    changed = true;
                    break;
                }
                index += 1;
            }
            block += 1;
        }
        changed
    }

    /// The callee of the call at `index` of `block`, its compile-time
    /// parameters bound to the call's, and the binding of each of its
    /// parameters, when the pass inlines the call.
    fn inlinable(
        &self,
        block: &MirBlock,
        index: usize,
    ) -> Option<(Reg, Cow<'c, MirFunction>, Vec<Binding>)> {
        let (dest, target, arguments, arg_places, compile_time) = match &block.instrs[index] {
            MirInstr::Call {
                dest,
                func,
                raises: None,
                args,
                kwargs,
                arg_places,
                capture_accesses,
                receiver: None,
                instantiated_args,
                spread: None,
                ..
            } if kwargs.is_empty() && capture_accesses.is_empty() => (
                *dest,
                func.0.as_str(),
                args.clone(),
                arg_places.clone(),
                instantiated_args,
            ),
            MirInstr::MethodCall {
                dest,
                recv,
                resolved: Some(target),
                raises: None,
                reference_result: None,
                result_adapter: None,
                args,
                kwargs,
                arg_places,
                capture_accesses,
                instantiated_args,
                spread: None,
                ..
            } if kwargs.is_empty() && capture_accesses.is_empty() => {
                let arguments = std::iter::once(*recv).chain(args.iter().copied()).collect();
                let places = std::iter::once(None)
                    .chain(arg_places.iter().cloned())
                    .collect();
                (*dest, target.as_str(), arguments, places, instantiated_args)
            }
            _ => return None,
        };
        let callee = self.callees.body(target)?;
        let declaration = self.declarations.function(target)?;
        if !self.is_closed(callee, declaration) || arguments.len() != callee.n_params {
            return None;
        }
        let callee = instantiated(callee, declaration, compile_time)?;
        let bindings = arguments
            .iter()
            .enumerate()
            .map(|(param, &argument)| {
                if callee.ref_params.get(param).copied().unwrap_or(true)
                    || callee.deinit_params.get(param).copied().unwrap_or(true)
                    || arg_places.get(param).is_some_and(Option::is_some)
                {
                    return None;
                }
                if callee.owned_params.get(param).copied().unwrap_or(false)
                    || callee.param_types.get(param).is_some_and(has_no_destructor)
                {
                    return Some(Binding::Value(argument));
                }
                loaded_place(block, index, argument).map(Binding::Place)
            })
            .collect::<Option<Vec<_>>>()?;
        Some((dest, callee, bindings))
    }

    /// Whether the callee has a shape the splice handles: no enclosing
    /// declaration's or receiver struct's compile-time parameters, no
    /// raise, no reference result, no variadic collector, and blocks of
    /// plain control flow with no `try`, region, or closure.
    fn is_closed(&self, callee: &MirFunction, declaration: &MirFunctionDeclaration) -> bool {
        let receiver_closed = !declaration.has_receiver
            || callee
                .param_types
                .first()
                .is_some_and(|receiver| match receiver.kind() {
                    TyKind::Struct(name, arguments) => {
                        arguments.is_empty()
                            && self
                                .declarations
                                .structure(name)
                                .is_some_and(|structure| structure.param_decls.is_empty())
                    }
                    _ => false,
                });
        receiver_closed
            && declaration.enclosing.is_none()
            && declaration.region_binders.is_empty()
            && declaration.variadic.is_none()
            && declaration.kw_variadic.is_none()
            && !declaration.returns_reference
            && !callee.raises
            && !callee.returns_reference
            && callee.blocks.iter().all(|block| {
                block.instrs.iter().all(|instruction| {
                    !matches!(
                        instruction,
                        MirInstr::Try { .. }
                            | MirInstr::If { .. }
                            | MirInstr::Loop { .. }
                            | MirInstr::ComptimeIf { .. }
                            | MirInstr::ComptimeFor { .. }
                            | MirInstr::MakeClosure { .. }
                            | MirInstr::Raise { .. }
                    )
                }) && matches!(
                    block.term,
                    MirTerm::Jump(_)
                        | MirTerm::Branch { .. }
                        | MirTerm::ComptimeBranch { .. }
                        | MirTerm::Return(_)
                        | MirTerm::Unreachable
                )
            })
    }

    /// The callee's blocks with its slots moved past the caller's and each
    /// place-bound parameter replaced by the caller's place, or `None` when
    /// a place-bound parameter is named other than as a place's root.
    fn inlined_blocks(&self, callee: &MirFunction, bindings: &[Binding]) -> Option<Vec<MirBlock>> {
        let base = self.function.n_vars as VarId;
        let mut blocks = callee.blocks.clone();
        renumber_block_slots(&mut blocks, &|var| var + base);
        let bound = |var: VarId| match bindings.get(var.checked_sub(base)? as usize)? {
            Binding::Place(place) => Some(place),
            Binding::Value(_) => None,
        };
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            for place in instruction_places_mut(instruction) {
                if let Some(outer) = bound(place.root) {
                    if place.through.is_some() && outer.through.is_some() {
                        return None;
                    }
                    *place = composed_place(outer, place);
                }
            }
            if instruction_slots(instruction)
                .into_iter()
                .any(|var| bound(var).is_some())
            {
                return None;
            }
        }
        Some(blocks)
    }

    /// Replace the call at `site` of `list` by `blocks`, the callee's body
    /// [`Self::inlined_blocks`] prepared: the call's block binds the
    /// parameters and jumps to it, and every return jumps to a new block
    /// holding what followed the call.
    fn splice(
        &mut self,
        list: &mut Vec<MirBlock>,
        site: &CallSite,
        callee: &MirFunction,
        bindings: &[Binding],
        mut blocks: Vec<MirBlock>,
    ) {
        let function = &mut *self.function;
        let CallSite { block, index, dest } = *site;
        let after = list[block].instrs.split_off(index + 1);
        list[block].instrs.truncate(index);
        let var_base = function.n_vars as VarId;
        let reg_base = function.n_regs;
        let entry = list.len();
        let continuation = entry + blocks.len();
        let result = result_of(callee);
        let result_slot = var_base + callee.n_vars as VarId;
        let renumber = |reg: Reg| {
            if result == Returned::Register(reg) {
                dest
            } else {
                Reg(reg.0 + reg_base)
            }
        };
        for inlined in &mut blocks {
            for instruction in &mut inlined.instrs {
                for reg in instruction_regs_mut(instruction) {
                    *reg = renumber(*reg);
                }
            }
            for reg in terminator_regs_mut(&mut inlined.term) {
                *reg = renumber(*reg);
            }
            for target in terminator_targets_mut(&mut inlined.term) {
                *target += entry;
            }
            inlined.term = match std::mem::replace(&mut inlined.term, MirTerm::Unreachable) {
                MirTerm::Return(Some(value)) if result == Returned::Slot => {
                    inlined.instrs.push(MirInstr::DefVar {
                        var: result_slot,
                        src: value,
                        binding_ty: callee.ret_ty.clone(),
                        in_place: false,
                    });
                    MirTerm::Jump(continuation)
                }
                MirTerm::Return(_) if result == Returned::None => MirTerm::Jump(continuation),
                MirTerm::Return(Some(_)) => MirTerm::Jump(continuation),
                // A value-less return from a function returning a value is
                // the checker's unreachable fall-off.
                MirTerm::Return(None) => MirTerm::Unreachable,
                term => term,
            };
        }

        let call_span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
        for reg in 0..callee.n_regs {
            let inlined = renumber(Reg(reg));
            if inlined == dest {
                continue;
            }
            if let Some(ty) = callee.reg_types.get(&reg) {
                function.reg_types.insert(inlined.0, ty.clone());
            }
            if let Some(span) = &call_span {
                function.spans.0.insert(inlined.0, (span.clone(), None));
            }
        }
        function.n_regs += callee.n_regs;
        for (var, name) in callee.var_names.iter().enumerate() {
            function.var_names.push(format!("{name}$inline{var_base}"));
            if let Some(ty) = callee.var_tys.get(&(var as VarId)) {
                function.var_tys.insert(var as VarId + var_base, ty.clone());
            }
        }
        function.n_vars += callee.n_vars;
        let mut resumed = Vec::with_capacity(after.len() + 1);
        match result {
            Returned::None => resumed.push(MirInstr::Const {
                dest,
                k: Const::None,
            }),
            Returned::Register(_) => {}
            Returned::Slot => {
                function.var_names.push(format!("$inline{var_base}$result"));
                if let Some(ty) = &callee.ret_ty {
                    function.var_tys.insert(result_slot, ty.clone());
                }
                function.n_vars += 1;
                resumed.push(MirInstr::UseVar {
                    dest,
                    var: result_slot,
                    mode: UseMode::Move,
                });
            }
        }
        resumed.extend(after.into_iter().map(|instruction| match instruction {
            MirInstr::DefVar {
                var,
                src,
                binding_ty,
                in_place: true,
            } if src == dest => MirInstr::DefVar {
                var,
                src,
                binding_ty,
                in_place: false,
            },
            other => other,
        }));

        let caller = &mut list[block];
        for (param, binding) in bindings.iter().enumerate() {
            if let Binding::Value(argument) = binding {
                let var = param as VarId;
                caller.instrs.push(MirInstr::DefVar {
                    var: var + var_base,
                    src: *argument,
                    binding_ty: callee
                        .var_tys
                        .get(&var)
                        .or_else(|| callee.param_types.get(param))
                        .cloned(),
                    in_place: false,
                });
            }
        }
        let term = std::mem::replace(&mut caller.term, MirTerm::Jump(entry));
        list.extend(blocks);
        list.push(MirBlock {
            instrs: resumed,
            term,
        });
    }
}

/// The constant a read of the parameter value `value` at `ty` is: its
/// typed constant when closed, the parameter expression otherwise.
fn parameter_constant(value: &ParamExpr, ty: Option<&Ty>) -> Const {
    value
        .as_constant()
        .and_then(|constant| Const::of_parameter_value(constant, ty))
        .unwrap_or_else(|| Const::Param(value.clone()))
}

/// The place the register `argument` was loaded from in `block` before the
/// call at `index`, when nothing between the load and the call names the
/// place's root but another load.
fn loaded_place(block: &MirBlock, index: usize, argument: Reg) -> Option<MirPlace> {
    let (load, place) =
        block.instrs[..index]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(position, instruction)| match instruction {
                MirInstr::LoadPlace { dest, place } if *dest == argument => Some((position, place)),
                _ => None,
            })?;
    let roots = [Some(place.root), place.through];
    block.instrs[load + 1..index]
        .iter()
        .all(|instruction| {
            matches!(instruction, MirInstr::LoadPlace { .. })
                || !instruction_slots(instruction)
                    .iter()
                    .any(|var| roots.contains(&Some(*var)))
        })
        .then(|| place.clone())
}

/// The callee's place `inner`, rooted at a parameter bound to the caller's
/// place `outer`, as a place of the caller.
fn composed_place(outer: &MirPlace, inner: &MirPlace) -> MirPlace {
    if inner.proj.is_empty() {
        return outer.clone();
    }
    let mut place = outer.clone();
    place.proj.extend(inner.proj.iter().cloned());
    place
        .projection_tys
        .extend(inner.projection_tys.iter().cloned());
    place.ty.clone_from(&inner.ty);
    place.through = outer.through.or(inner.through);
    place
}

/// What the callee returns through: nothing when it returns `None`, its
/// register when one return names it, a slot otherwise.
fn result_of(callee: &MirFunction) -> Returned {
    if callee
        .ret_ty
        .as_ref()
        .is_none_or(|ty| matches!(ty.kind(), TyKind::None))
    {
        return Returned::None;
    }
    let mut returned = callee.blocks.iter().filter_map(|block| match block.term {
        MirTerm::Return(Some(value)) => Some(value),
        _ => None,
    });
    match (returned.next(), returned.next()) {
        (Some(value), None) => Returned::Register(value),
        _ => Returned::Slot,
    }
}

/// `callee` with its own compile-time parameters bound to the call's
/// `arguments`: the callee itself when it declares none, `None` when the
/// binding keeps the call.
fn instantiated<'c>(
    callee: &'c MirFunction,
    declaration: &MirFunctionDeclaration,
    arguments: &[TyArg],
) -> Option<Cow<'c, MirFunction>> {
    if declaration.param_decls.is_empty() {
        return arguments.is_empty().then_some(Cow::Borrowed(callee));
    }
    let mut instantiation = Instantiation::new(declaration, arguments)?;
    let mut function = callee.clone();
    walk_function_mut(&mut instantiation, &mut function).ok()?;
    instantiation.read_binders(&mut function)?;
    Some(Cow::Owned(function))
}

/// A callee's compile-time parameters bound to a call's arguments, as an
/// environment of [`replace_parameters`].
struct Instantiation {
    context: ParamContext,
    bindings: ParamBindings,
    /// Each value binder's spelling, which names the slot a read of it
    /// loads, and its argument.
    values: Vec<(String, ParamExpr)>,
    /// The callee's type binders, which a site may name only to rename.
    types: Vec<ParamRef>,
}

impl Instantiation {
    /// The binding of `declaration`'s binders to `arguments`, one per
    /// binder in declaration order; `None` for a binder or argument this
    /// pass does not bind.
    fn new(declaration: &MirFunctionDeclaration, arguments: &[TyArg]) -> Option<Self> {
        if !declaration.availability.is_empty() || arguments.len() != declaration.param_decls.len()
        {
            return None;
        }
        let context = ParamContext::detached();
        let mut instantiation = Self {
            bindings: ParamBindings::new(),
            values: Vec::new(),
            types: Vec::new(),
            context,
        };
        for (decl, argument) in declaration.param_decls.iter().zip(arguments) {
            match (decl, argument) {
                (
                    ParamDecl::Type {
                        id,
                        name,
                        callable_bound: None,
                        variadic: false,
                        constraints,
                        ..
                    },
                    TyArg::Ty(ty),
                ) if constraints.is_empty() => {
                    instantiation.bindings.bind_type(id.clone(), ty.clone());
                    instantiation.types.push(ParamRef {
                        id: id.clone(),
                        name: name.as_str().into(),
                    });
                }
                (
                    ParamDecl::Value {
                        id,
                        name,
                        ty,
                        callable_default: None,
                        variadic: false,
                        constraints,
                        ..
                    },
                    TyArg::Val(value),
                ) if constraints.is_empty()
                    && !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
                    && !matches!(value, CtValue::Deferred(_) | CtValue::Marker(_)) =>
                {
                    let value = instantiation.context.constant(value.clone()).ok()?;
                    instantiation.bindings.bind(id.clone(), value.clone());
                    instantiation
                        .values
                        .push((name.trim_start_matches('*').to_string(), value));
                }
                _ => return None,
            }
        }
        Some(instantiation)
    }

    /// Rewrite each read of a value binder's slot in `function` into its
    /// argument as a constant, and delete the slot's drops: the slot holds
    /// nothing. `None` when a binder's slot is used otherwise.
    fn read_binders(&self, function: &mut MirFunction) -> Option<()> {
        for (name, value) in &self.values {
            let Some(slot) = function.var_names.iter().position(|var| var == name) else {
                continue;
            };
            read_binder(
                &mut function.blocks,
                slot as VarId,
                value,
                &function.reg_types,
            )?;
        }
        fold_parameter_constants(&mut function.blocks, &function.reg_types);
        Some(())
    }
}

impl MutTypeVisitor for Instantiation {
    type Error = ParamError;

    fn visit_ty(&mut self, ty: &mut Ty, _: TySite) -> Result<(), ParamError> {
        *ty = replace_parameters(&self.context, ty, &self.bindings, 0)?;
        Ok(())
    }

    fn visit_ty_arg(&mut self, arg: &mut TyArg) -> Result<(), ParamError> {
        *arg = replace_argument_parameters(&self.context, arg, &self.bindings)?;
        Ok(())
    }

    fn visit_variant_alternatives(&mut self, alternatives: &mut Vec<Ty>) -> Result<(), ParamError> {
        for ty in alternatives {
            *ty = replace_parameters(&self.context, ty, &self.bindings, 0)?;
        }
        Ok(())
    }

    fn visit_variant_index(&mut self, index: &mut VariantIndex) -> Result<(), ParamError> {
        *index = index.try_map_expr(|expr| self.context.replace(expr, &self.bindings))?;
        Ok(())
    }

    fn visit_lane_mask(&mut self, mask: &mut LaneMask) -> Result<(), ParamError> {
        *mask = mask.try_map_exprs(&mut |expr| self.context.replace(expr, &self.bindings))?;
        Ok(())
    }

    fn visit_param_arg(
        &mut self,
        arg: &mut MirParamArg,
        _: ParamArgSite,
    ) -> Result<(), ParamError> {
        if let Some(expr) = &mut arg.expr {
            *expr = self.context.replace(expr, &self.bindings)?;
        }
        Ok(())
    }

    /// A contract's own binders are bound by its callee, as the elaborator
    /// leaves them.
    fn visit_param_decls(
        &mut self,
        _: &mut [ParamDecl],
        _: ParamDeclSite,
    ) -> Result<(), ParamError> {
        Ok(())
    }

    fn visit_constant(&mut self, constant: &mut Const) -> Result<(), ParamError> {
        match constant {
            Const::Param(expr) => *expr = self.context.replace(expr, &self.bindings)?,
            Const::Value(value) => {
                *value = replace_value_parameters(&self.context, value, &self.bindings)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A constructed type binder renames to the binder it is bound to; at a
    /// concrete type its construction is the elaborator's to lower.
    fn visit_param_ref(&mut self, param: &mut ParamRef) -> Result<(), ParamError> {
        if !self.types.contains(param) {
            return Ok(());
        }
        match self.bindings.types().get(&param.id).map(Ty::kind) {
            Some(TyKind::Param { binder, .. }) => {
                *param = binder.clone();
                Ok(())
            }
            _ => Err(ParamError::Unsupported(format!(
                "constructing `{}` at a concrete binding",
                param.name
            ))),
        }
    }

    fn visit_constraint(&mut self, cond: &mut GenericConstraint) -> Result<(), ParamError> {
        *cond = replace_constraint_parameters(&self.context, cond, &self.bindings)?;
        Ok(())
    }

    fn visit_sequence(&mut self, _: &mut ComptimeSequence) -> Result<(), ParamError> {
        Err(ParamError::Unsupported(
            "a `comptime for` sequence".to_string(),
        ))
    }
}

/// Rewrite each read of the value binder slot `slot` in `blocks`, its
/// regions included, into `value` as a constant typed by the read's
/// register, and delete the slot's drops; `None` when the slot is used
/// otherwise.
fn read_binder(
    blocks: &mut [MirBlock],
    slot: VarId,
    value: &ParamExpr,
    reg_types: &std::collections::HashMap<u32, Ty>,
) -> Option<()> {
    for block in blocks {
        let mut kept = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            for region in instruction_regions_mut(&mut instruction) {
                read_binder(region, slot, value, reg_types)?;
            }
            let read = match &instruction {
                MirInstr::UseVar { dest, var, .. } if *var == slot => Some((*dest, None)),
                MirInstr::LoadPlace { dest, place } if place.root == slot => {
                    Some((*dest, Some(place)))
                }
                MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } if *var == slot => {
                    continue;
                }
                _ => None,
            };
            if let Some((dest, place)) = read {
                let ty = reg_types.get(&dest.0);
                let k = match place {
                    None => parameter_constant(value, ty),
                    Some(place) if place.is_whole_root() => parameter_constant(value, ty),
                    Some(place) => Const::of_projected_parameter_value(
                        value.as_constant()?,
                        &place.storage().proj,
                        place.ty.as_ref(),
                    )?,
                };
                kept.push(MirInstr::Const { dest, k });
                continue;
            }
            if instruction_slots(&instruction).contains(&slot)
                || instruction_places(&instruction)
                    .iter()
                    .any(|place| place.root == slot || place.through == Some(slot))
            {
                return None;
            }
            kept.push(instruction);
        }
        block.instrs = kept;
    }
    Some(())
}

/// Fold each `Const::Param` of `blocks` the substitution closed into the
/// constant its register reads it as.
fn fold_parameter_constants(
    blocks: &mut [MirBlock],
    reg_types: &std::collections::HashMap<u32, Ty>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                fold_parameter_constants(region, reg_types);
            }
            if let MirInstr::Const { dest, k } = instruction
                && let Const::Param(expr) = k
                && expr.as_constant().is_some()
            {
                *k = parameter_constant(expr, reg_types.get(&dest.0));
            }
        }
    }
}
