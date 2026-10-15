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
//! A callee this pass inlines names none of its own compile-time parameters
//! (a concrete function, or a method of a non-parametric struct), takes its
//! arguments positionally into `read` or owned parameters, does not raise,
//! and returns a value. An owned parameter, or a `read` one of a type no
//! destructor tears down, is a fresh slot the argument initializes; any
//! other `read` parameter is the caller's place the argument was loaded
//! from, as upstream's inliner substitutes the borrowed address for the
//! callee's argument. A call of any other shape stays a call.

use super::pre_elaboration::{CallGraphPass, Callees};
use mojito_checked::declaration::InlineLevel;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_places_mut;
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirTerm, Reg, UseMode, has_no_destructor, instruction_regions_mut, instruction_regs_mut,
    instruction_slots, renumber_block_slots, terminator_regs_mut, terminator_targets_mut,
};
use mojito_types::types::TyKind;

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
                    && let Some(blocks) = self.inlined_blocks(callee, &bindings)
                {
                    let site = CallSite { block, index, dest };
                    self.splice(list, &site, callee, &bindings, blocks);
                    changed = true;
                    break;
                }
                index += 1;
            }
            block += 1;
        }
        changed
    }

    /// The callee of the call at `index` of `block` and the binding of each
    /// of its parameters, when the pass inlines the call.
    fn inlinable(
        &self,
        block: &MirBlock,
        index: usize,
    ) -> Option<(Reg, &'c MirFunction, Vec<Binding>)> {
        let (dest, target, arguments, arg_places) = match &block.instrs[index] {
            MirInstr::Call {
                dest,
                func,
                raises: None,
                args,
                kwargs,
                arg_places,
                capture_accesses,
                param_arg_regs,
                receiver: None,
                instantiated_args,
                spread: None,
                ..
            } if kwargs.is_empty()
                && capture_accesses.is_empty()
                && param_arg_regs.is_empty()
                && instantiated_args.is_empty() =>
            {
                (*dest, func.0.as_str(), args.clone(), arg_places.clone())
            }
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
                param_arg_regs,
                param_decls,
                instantiated_args,
                spread: None,
                ..
            } if kwargs.is_empty()
                && capture_accesses.is_empty()
                && param_arg_regs.is_empty()
                && param_decls.is_empty()
                && instantiated_args.is_empty() =>
            {
                let arguments = std::iter::once(*recv).chain(args.iter().copied()).collect();
                let places = std::iter::once(None)
                    .chain(arg_places.iter().cloned())
                    .collect();
                (*dest, target.as_str(), arguments, places)
            }
            _ => return None,
        };
        let callee = self.callees.body(target)?;
        let declaration = self.declarations.function(target)?;
        if !self.is_closed(callee, declaration) || arguments.len() != callee.n_params {
            return None;
        }
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

    /// Whether the callee names none of its own compile-time parameters
    /// and has a shape the splice handles: no raise, no reference result,
    /// no variadic collector, and blocks of plain control flow with no
    /// `try`, region, or closure.
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
            && declaration.param_decls.is_empty()
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
