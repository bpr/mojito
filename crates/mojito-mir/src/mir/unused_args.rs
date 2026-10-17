//! Argument uses: which of a function's runtime parameters its body never
//! uses, the analysis upstream's `RemoveUnusedParams` runs first
//! (`identifyUnusedArguments`), before it reads the generator for unused
//! compile-time parameters with those arguments gone.
//!
//! A slot is used by every instruction, handler binding, and cleanup list
//! that names it, but a drop of a value whose drop runs no code
//! ([`MirDeclarations::drop_runs_no_code`]): upstream counts no such drop as
//! a user, so an owned `var a: Arr[n]` whose struct only holds a vector is
//! as unused as a borrowed one.

use crate::mir::call_defaults::erase_unpassed_defaults;
use crate::mir::param_uses::{erase_forward_chains, register_reads};
use crate::mir::verify::instruction_result_regs;
use crate::mir::{
    CleanupDrop, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    MirPlace, MirTerm, Reg, SlotParameter, cleanup_slots, instruction_regions,
    instruction_regions_mut, instruction_slots, retire_parameters, table_member,
};
use mojito_ast::call::{ArgSlot, CallVariadics, MatchError, match_call_slots};
use mojito_hir::hir::VarId;
use mojito_types::origin::Origin;
use mojito_types::types::{Ty, TyArg, TyKind, mentions};
use std::collections::{BTreeSet, HashMap, HashSet};

/// The parameter slots of `name`, the body `function`, that nothing in it
/// uses ([`used_slots`]), ascending: the arguments upstream's
/// `RemoveUnusedParams` removes from the generator a direct call names.
///
/// Only a free function, a nested one, a method a resolved method call,
/// an operator, or a subscript names, and a static method a call through
/// its struct or an instance names, the callees whose every demand the
/// elaborator reduces, have candidates: neither a constructor nor a table
/// member ([`crate::mir::table_member`]) or protocol member
/// ([`PROTOCOL_MEMBERS`]), which a backend reaches from a value's type
/// with every argument, nor one whose signature ties an origin or a
/// returned reference to a parameter. A nested body's calls by name are
/// direct calls, one declared in a `comptime for` too, which each unrolled
/// copy makes, and its captures are leading arguments like any other. A
/// `deinit` parameter and a callable one are never candidates; a method's
/// receiver is one like any other argument, and the call of a generator
/// without it passes none, but a subscript accessor's
/// ([`SUBSCRIPT_ACCESSORS`]), which every subscript passes. A collector is
/// one too, whose elements the rewritten call passes none of, but an owned
/// collector's drop runs code, so it is a use.
pub fn unused_arguments(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<VarId> {
    let Some(declaration) = declarations.function(name) else {
        return Vec::new();
    };
    if !reducible(name, function, declaration) {
        return Vec::new();
    }
    let used = used_slots(function, declarations);
    let keeps_receiver = declaration.has_receiver
        && mojito_symbol::symbol::split_method_symbol(name)
            .is_some_and(|(_, method)| SUBSCRIPT_ACCESSORS.contains(&member_base(method)));
    (0..function.n_params)
        .filter(|&slot| !(keeps_receiver && slot == 0))
        .filter(|&slot| {
            !function.deinit_params.get(slot).copied().unwrap_or(true)
                && !mentions(&function.param_types[slot], &|ty| {
                    matches!(
                        ty.kind(),
                        TyKind::Func(_)
                            | TyKind::GenericFunc(_)
                            | TyKind::Param {
                                callable_bound: Some(_),
                                ..
                            }
                    )
                })
        })
        .map(|slot| slot as VarId)
        .filter(|slot| !used.contains(slot))
        .collect()
}

/// The slots `function` uses: those its instructions, handler bindings,
/// and cleanup lists name, in its regions too, but a drop, by a `drop.var`
/// or a cleanup list, of a slot whose drop runs no code.
pub fn used_slots(function: &MirFunction, declarations: &MirDeclarations) -> BTreeSet<VarId> {
    let trivial = |slot: VarId| {
        function
            .var_tys
            .get(&slot)
            .or_else(|| function.param_types.get(slot as usize))
            .is_some_and(|ty| declarations.drop_runs_no_code(ty))
    };
    let mut used = BTreeSet::new();
    collect_used(&function.blocks, &trivial, &mut used);
    used
}

/// `function` and its `declaration` with the parameter slots `removed` gone.
///
/// It is the generator upstream's `RemoveUnusedParams` leaves a direct call
/// naming: each slot's drops leave the body, its parameter entries the body
/// and the declaration, and every slot above one moves down past it. Each
/// slot maps to its parameter by the body-slot layout
/// ([`MirFunctionDeclaration::slot_parameter`]): a generator without its
/// receiver declares none, and one without a collector collects nothing.
pub fn without_removed_arguments(
    function: &MirFunction,
    declaration: &MirFunctionDeclaration,
    removed: &[VarId],
) -> (MirFunction, MirFunctionDeclaration) {
    let retired: BTreeSet<VarId> = removed.iter().copied().collect();
    let mut view = function.clone();
    remove_drops(&mut view.blocks, &retired);
    retire_parameters(&mut view, &retired);
    view.removed_args.clear();
    let mut declaration = declaration.clone();
    let parameters: Vec<SlotParameter> = retired
        .iter()
        .map(|&slot| declaration.slot_parameter(slot as usize))
        .collect();
    let positions: BTreeSet<usize> = parameters
        .iter()
        .filter_map(|parameter| match parameter {
            SlotParameter::Position(position) => Some(*position),
            _ => None,
        })
        .collect();
    let below = |limit: usize| positions.range(..limit).count();
    let mut keyword_shift = declaration.kw_variadic_index.map_or(0, below);
    declaration.variadic_index = declaration.variadic_index.map(|index| index - below(index));
    for parameter in &parameters {
        match parameter {
            SlotParameter::Receiver => {
                declaration.has_receiver = false;
                declaration.receiver_convention = None;
            }
            SlotParameter::Variadic => {
                declaration.variadic = None;
                declaration.variadic_convention = None;
                declaration.variadic_index = None;
                // The keyword collector comes last, past the positional one.
                keyword_shift += 1;
            }
            SlotParameter::KwVariadic => {
                declaration.kw_variadic = None;
                declaration.kw_variadic_convention = None;
                declaration.kw_variadic_index = None;
            }
            SlotParameter::Position(_) => {}
        }
    }
    declaration.kw_variadic_index = declaration
        .kw_variadic_index
        .map(|index| index - keyword_shift);
    for &position in positions.iter().rev() {
        let remove = |length: usize| position < length;
        if remove(declaration.param_names.len()) {
            declaration.param_names.remove(position);
        }
        if remove(declaration.param_types.len()) {
            declaration.param_types.remove(position);
        }
        if remove(declaration.defaults.len()) {
            declaration.defaults.remove(position);
        }
        if remove(declaration.required.len()) {
            declaration.required.remove(position);
        }
        if remove(declaration.param_conventions.len()) {
            declaration.param_conventions.remove(position);
        }
        if remove(declaration.ref_params.len()) {
            declaration.ref_params.remove(position);
        }
        if remove(declaration.param_writes.len()) {
            declaration.param_writes.remove(position);
        }
    }
    let shifted = |limit: usize| limit - below(limit);
    declaration.positional_only = declaration.positional_only.map(shifted);
    declaration.keyword_only = declaration.keyword_only.map(shifted);
    (view, declaration)
}

/// The positional and keyword operand indexes that bind the `removed`
/// slots of `declaration`'s function in a call passing `positional`
/// arguments and `keywords`.
///
/// They are the operands upstream's rewritten call of the generator
/// without those slots passes none of. A removed slot the call
/// omits, whose default is never evaluated, binds none, and a removed
/// collector binds every overflow operand of its kind.
pub fn removed_argument_operands(
    declaration: &MirFunctionDeclaration,
    removed: &[VarId],
    positional: usize,
    keywords: &[&str],
) -> Result<(BTreeSet<usize>, BTreeSet<usize>), MatchError> {
    let slots = match_call_slots(
        &declaration.param_names,
        &declaration.required,
        declaration.positional_only,
        declaration.keyword_only,
        positional,
        keywords,
        CallVariadics {
            positional: declaration.variadic.is_some(),
            keyword: declaration.kw_variadic.is_some(),
        },
    )?;
    let mut positionals = BTreeSet::new();
    let mut keyword_operands = BTreeSet::new();
    for &slot in removed {
        match declaration.slot_parameter(slot as usize) {
            SlotParameter::Position(position) => match slots.slots.get(position) {
                Some(ArgSlot::Positional(index)) => {
                    positionals.insert(*index);
                }
                Some(ArgSlot::Keyword(index)) => {
                    keyword_operands.insert(*index);
                }
                Some(ArgSlot::Default) | None => {}
            },
            SlotParameter::Variadic => positionals.extend(slots.positional_overflow.iter()),
            SlotParameter::KwVariadic => {
                keyword_operands.extend(slots.keyword_overflow.iter());
            }
            SlotParameter::Receiver => {}
        }
    }
    Ok((positionals, keyword_operands))
}

/// `function` with its direct calls passing no operand for their callees'
/// removed arguments, `removed(callee)`; `None` when no call drops one.
///
/// The instructions that only computed such an operand are erased too.
/// Returns the view and the callees whose calls dropped an operand.
///
/// It is the body upstream's `RemoveUnusedParams` reads once it has
/// rewritten each call of a generator it removed an argument from to name
/// the clone without it (`_REMOVED_ARG`), as it walks callees first: an
/// argument the body only passed on to one is unused in turn. A call drops
/// the operands [`removed_argument_operands`] binds, with their places, and
/// a resolved method call whose callee removed its receiver takes a
/// register no instruction defines, of the receiver's type, in its stead:
/// the rewritten call passes no receiver but still names the struct
/// parameters its type spells. An erased forward is a
/// whole-place load, a borrow, a copy of a slot whose type has no
/// destructor, or a move of a parameter whose drop runs no code; a copy of
/// any other value is a call of its own, and stays. The evaluation of a
/// default the call left out goes too, with the read temporary it was
/// bound to ([`crate::mir::call_defaults::erase_unpassed_defaults`]).
pub fn without_removed_argument_forwards<'a>(
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [VarId]>,
) -> Option<(MirFunction, Vec<String>)> {
    argument_forwards_view(function, declarations, removed, Operands::Drop)
        .map(|view| (view.function, view.discounted))
}

/// The body an instance of `function` runs, as
/// [`without_removed_argument_forwards`] views it but for the operands.
///
/// The instructions that only computed an operand its direct calls pass
/// for their callees' removed arguments, `removed(callee)`, are erased,
/// a left-out default's evaluation among them; `None` when no call drops
/// one.
///
/// Each call keeps those operands, with their registers' types, and only
/// loses their places, a removed receiver taking a register no instruction
/// defines as in that view, since the elaborator binds the call against the
/// callee's full declaration before it drops them, as it names the
/// generator without them. Returns the view beside the kept operands no
/// instruction defines any more, ascending, whose types the instance
/// sheds once its calls drop them.
pub fn with_removed_argument_forwards_erased<'a>(
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [VarId]>,
) -> Option<(MirFunction, Vec<Reg>)> {
    argument_forwards_view(function, declarations, removed, Operands::Keep)
        .map(|view| (view.function, view.undefined))
}

/// Erase from `function` each instruction that only computed an operand a
/// rewrite took from a call for a callee's removed argument.
///
/// `before` is how many times the body read each register before it: an
/// inlined generator without the argument takes none, as upstream's
/// rewritten call passes none. The erasures are
/// [`without_removed_argument_forwards`]'s.
pub fn erase_removed_operands(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    before: &[u32],
) {
    erase_forward_chains(function, declarations, &HashSet::new(), before);
}

/// The members a backend runs from a value's type with every argument its
/// protocol passes, beside the member table's ([`table_member`]): an
/// iteration's `__iter__` and `__next__`, and the conversions and
/// arithmetic the builtins `len`, `abs`, `round`, `Bool`, `Float64`, and
/// `math` dispatch. No call the elaborator rewrites names one, so none
/// loses an argument.
const PROTOCOL_MEMBERS: [&str; 14] = [
    "__iter__",
    "__next__",
    "__len__",
    "__abs__",
    "__round__",
    "__bool__",
    "__float__",
    "__trunc__",
    "__floor__",
    "__ceil__",
    "__fma__",
    "__ceildiv__",
    "__contains__",
    "__mlir_index__",
];

/// The accessors a subscript reaches, whose receiver every subscript
/// instruction passes, so it is never removed.
const SUBSCRIPT_ACCESSORS: [&str; 2] = ["__getitem__", "__getitem_param__"];

/// Whether `name`, so declared, is a function whose arguments the pass may
/// remove ([`unused_arguments`]).
fn reducible(name: &str, function: &MirFunction, declaration: &MirFunctionDeclaration) -> bool {
    name != "__toplevel__"
        && (declaration.enclosing.is_some()
            || mojito_symbol::symbol::split_method_symbol(name).is_none_or(|(_, method)| {
                table_member(method).is_none() && !PROTOCOL_MEMBERS.contains(&member_base(method))
            }))
        && !declaration.returns_reference
        && usize::from(declaration.has_receiver)
            + declaration.param_names.len()
            + usize::from(declaration.variadic.is_some())
            + usize::from(declaration.kw_variadic.is_some())
            == function.n_params
        && !declaration
            .param_types
            .iter()
            .chain(std::iter::once(&declaration.ret_ty))
            .any(names_a_place)
}

/// The method spelling `method` before any overload qualifier.
fn member_base(method: &str) -> &str {
    method.split('$').next().unwrap_or(method)
}

/// Whether `ty` holds an origin that names a place, which may be a
/// parameter's.
fn names_a_place(ty: &Ty) -> bool {
    fn place(origin: &Origin) -> bool {
        match origin {
            Origin::Place(_) => true,
            Origin::Union(origins) => origins.iter().any(place),
            _ => false,
        }
    }
    mentions(ty, &|ty| match ty.kind() {
        TyKind::Ref(reference) => place(&reference.origin),
        TyKind::Struct(_, arguments) => arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Origin(origin) if place(origin))),
        _ => false,
    })
}

/// Add to `used` each slot `blocks` and their regions use ([`used_slots`]),
/// where `trivial` answers whether dropping a slot runs no code.
fn collect_used(blocks: &[MirBlock], trivial: &dyn Fn(VarId) -> bool, used: &mut BTreeSet<VarId>) {
    let cleanup = |drops: &[CleanupDrop], used: &mut BTreeSet<VarId>| {
        used.extend(cleanup_slots(drops).filter(|&slot| !trivial(slot)));
    };
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::DropVar { var, .. } if trivial(*var) => {}
                MirInstr::Try {
                    handler,
                    cleanup: slots,
                    ..
                } => {
                    used.extend(instruction_slots(instruction));
                    used.extend(handler.iter().filter_map(|(slot, _)| *slot));
                    cleanup(slots, used);
                }
                _ => used.extend(instruction_slots(instruction)),
            }
            for region in instruction_regions(instruction) {
                collect_used(region, trivial, used);
            }
        }
        if let MirTerm::ReturnWithCleanup { cleanup: slots, .. }
        | MirTerm::EscapeJump { cleanup: slots, .. } = &block.term
        {
            cleanup(slots, used);
        }
    }
}

/// Remove from `blocks` and their regions every drop of a `retired` slot,
/// by a `drop.var` or a cleanup list.
fn remove_drops(blocks: &mut [MirBlock], retired: &BTreeSet<VarId>) {
    for block in blocks {
        block.instrs.retain(
            |instruction| !matches!(instruction, MirInstr::DropVar { var, .. } if retired.contains(var)),
        );
        for instruction in &mut block.instrs {
            if let MirInstr::Try { cleanup, .. } = instruction {
                cleanup.retain(|drop| !retired.contains(&drop.var));
            }
            for region in instruction_regions_mut(instruction) {
                remove_drops(region, retired);
            }
        }
        if let MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } =
            &mut block.term
        {
            cleanup.retain(|drop| !retired.contains(&drop.var));
        }
    }
}

/// Whether a view of a body drops the operands its calls pass for their
/// callees' removed arguments, as [`without_removed_argument_forwards`]
/// does, or keeps them, as [`with_removed_argument_forwards_erased`] does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operands {
    Drop,
    Keep,
}

/// A view [`argument_forwards_view`] builds.
struct ArgumentForwards {
    function: MirFunction,
    /// The callees whose calls dropped an operand, sorted.
    discounted: Vec<String>,
    /// The operands a call keeps that no instruction defines any more,
    /// ascending; empty when the calls drop them.
    undefined: Vec<Reg>,
}

/// The view of `function` that [`without_removed_argument_forwards`] or,
/// by `operands`, [`with_removed_argument_forwards_erased`] describes.
fn argument_forwards_view<'a>(
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [VarId]>,
    operands: Operands,
) -> Option<ArgumentForwards> {
    if !calls_removing(&function.blocks, removed) {
        return None;
    }
    let mut view = function.clone();
    let original = register_reads(&mut view);
    let mut discounted = BTreeSet::new();
    let mut dropped = BTreeSet::new();
    drop_removed_operands(
        &mut view.blocks,
        &mut Receivers {
            n_regs: &mut view.n_regs,
            reg_types: &mut view.reg_types,
        },
        declarations,
        removed,
        Operands::Drop,
        &mut discounted,
        &mut dropped,
    );
    if discounted.is_empty() {
        return None;
    }
    let mut erased = erase_unpassed_defaults(&mut view, &dropped, &original);
    view.reg_types.retain(|reg, _| !erased.contains(reg));
    erased.extend(erase_forward_chains(
        &mut view,
        declarations,
        &HashSet::new(),
        &original,
    ));
    let mut undefined = Vec::new();
    if operands == Operands::Keep {
        view = function.clone();
        let mut kept = BTreeSet::new();
        drop_removed_operands(
            &mut view.blocks,
            &mut Receivers {
                n_regs: &mut view.n_regs,
                reg_types: &mut view.reg_types,
            },
            declarations,
            removed,
            Operands::Keep,
            &mut BTreeSet::new(),
            &mut kept,
        );
        erase_unpassed_defaults(&mut view, &kept, &original);
        erase_results(&mut view.blocks, &erased);
        undefined = kept
            .into_iter()
            .filter(|&reg| erased.contains(&reg) || reg >= function.n_regs)
            .map(Reg)
            .collect();
        view.reg_types
            .retain(|reg, _| !erased.contains(reg) || undefined.contains(&Reg(*reg)));
    }
    Some(ArgumentForwards {
        function: view,
        discounted: discounted.into_iter().collect(),
        undefined,
    })
}

/// Whether `blocks` or their regions hold a direct call, or a resolved
/// method call, of a callee `removed` names arguments of.
fn calls_removing<'a>(blocks: &[MirBlock], removed: &dyn Fn(&str) -> Option<&'a [VarId]>) -> bool {
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .any(|instruction| {
            let target = match instruction {
                MirInstr::Call {
                    func, spread: None, ..
                } => Some(func.0.as_str()),
                MirInstr::MethodCall { resolved, .. } => resolved.as_deref(),
                _ => None,
            };
            target
                .and_then(removed)
                .is_some_and(|slots| !slots.is_empty())
                || instruction_regions(instruction)
                    .into_iter()
                    .any(|region| calls_removing(region, removed))
        })
}

/// Where a view takes a register no instruction defines, for a call's
/// receiver its rewrite passes none of.
struct Receivers<'f> {
    n_regs: &'f mut u32,
    reg_types: &'f mut HashMap<u32, Ty>,
}

impl Receivers<'_> {
    /// A fresh register of the type `reg` holds.
    fn fresh(&mut self, reg: Reg) -> Reg {
        let fresh = Reg(*self.n_regs);
        *self.n_regs += 1;
        if let Some(ty) = self.reg_types.get(&reg.0).cloned() {
            self.reg_types.insert(fresh.0, ty);
        }
        fresh
    }
}

/// Drop from each direct call in `blocks` and their regions the operands
/// of its callee's removed arguments or, by `operands`, only their places,
/// adding each callee that lost one to `discounted` and each operand's
/// register to `dropped`.
fn drop_removed_operands<'a>(
    blocks: &mut [MirBlock],
    receivers: &mut Receivers<'_>,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [VarId]>,
    operands: Operands,
    discounted: &mut BTreeSet<String>,
    dropped: &mut BTreeSet<u32>,
) {
    let callee = |target: &str| {
        removed(target)
            .filter(|slots| !slots.is_empty())
            .zip(declarations.function(target))
    };
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        for region in instruction_regions_mut(instruction) {
            drop_removed_operands(
                region,
                receivers,
                declarations,
                removed,
                operands,
                discounted,
                dropped,
            );
        }
        match instruction {
            MirInstr::Call {
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread: None,
                ..
            } => {
                let call = CallOperands {
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                };
                if let Some((slots, declaration)) = callee(&func.0)
                    && call.drop(declaration, slots, operands, dropped)
                {
                    discounted.insert(func.0.clone());
                }
            }
            MirInstr::MethodCall {
                recv,
                resolved: Some(target),
                args,
                kwargs,
                recv_place,
                arg_places,
                kwarg_places,
                spread,
                ..
            } => {
                let Some((slots, declaration)) = callee(target) else {
                    continue;
                };
                let name = target.clone();
                let call = CallOperands {
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                };
                // A spread operand binds as many slots as its pack holds,
                // which only the elaborator's expansion knows.
                let mut changed =
                    spread.is_none() && call.drop(declaration, slots, operands, dropped);
                if declaration.has_receiver && slots.contains(&0) {
                    changed = true;
                    // The rewritten call passes no receiver, but still
                    // names its struct's parameters, which its type spells:
                    // a register no instruction defines carries that type.
                    let fresh = receivers.fresh(*recv);
                    *recv = fresh;
                    *recv_place = None;
                    dropped.insert(fresh.0);
                }
                if changed {
                    discounted.insert(name);
                }
            }
            _ => {}
        }
    }
}

/// The runtime operands of one call, with their places.
struct CallOperands<'c> {
    args: &'c mut Vec<Reg>,
    arg_places: &'c mut Vec<Option<MirPlace>>,
    kwargs: &'c mut Vec<(String, Reg)>,
    kwarg_places: &'c mut Vec<Option<MirPlace>>,
}

impl CallOperands<'_> {
    /// Drop the operands that bind the removed `slots` of the function
    /// `declaration` declares or, by `operands`, only their places, adding
    /// their registers to `dropped`; returns whether it dropped one. A call
    /// whose arguments do not bind is left whole.
    fn drop(
        self,
        declaration: &MirFunctionDeclaration,
        slots: &[VarId],
        operands: Operands,
        dropped: &mut BTreeSet<u32>,
    ) -> bool {
        let keywords: Vec<&str> = self.kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let Ok((positional, keyword)) =
            removed_argument_operands(declaration, slots, self.args.len(), &keywords)
        else {
            return false;
        };
        dropped.extend(positional.iter().map(|&index| self.args[index].0));
        dropped.extend(keyword.iter().map(|&index| self.kwargs[index].1.0));
        let drop = |indexes: &BTreeSet<usize>, places: &mut Vec<Option<MirPlace>>| {
            for &index in indexes.iter().rev() {
                if index >= places.len() {
                    continue;
                }
                match operands {
                    Operands::Drop => {
                        places.remove(index);
                    }
                    Operands::Keep => places[index] = None,
                }
            }
        };
        drop(&positional, self.arg_places);
        drop(&keyword, self.kwarg_places);
        if operands == Operands::Drop {
            for &index in positional.iter().rev() {
                self.args.remove(index);
            }
            for &index in keyword.iter().rev() {
                self.kwargs.remove(index);
            }
        }
        !positional.is_empty() || !keyword.is_empty()
    }
}

/// Remove from `blocks` and their regions each instruction that defines
/// only `erased` registers.
fn erase_results(blocks: &mut [MirBlock], erased: &HashSet<u32>) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
                erase_results(region, erased);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            results.is_empty() || !results.iter().all(|reg| erased.contains(&reg.0))
        });
    }
}
