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

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirTerm,
    instruction_regions, instruction_regions_mut, instruction_slots, retire_parameters,
};
use mojito_hir::hir::VarId;
use mojito_types::origin::Origin;
use mojito_types::types::{Ty, TyArg, TyKind, mentions};
use std::collections::BTreeSet;

/// The parameter slots of `name`, the body `function`, that nothing in it
/// uses ([`used_slots`]), ascending: the arguments upstream's
/// `RemoveUnusedParams` removes from the generator a direct call names.
///
/// Only a top-level free function, the only callee whose every direct
/// call the elaborator reduces, has candidates: neither a method nor a
/// nested, lifted, or variadic function, nor one whose signature ties an
/// origin or a returned reference to a parameter. A `deinit` parameter and
/// a callable one are never candidates.
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
    (0..function.n_params)
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
/// and the declaration, and every slot above one moves down past it.
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
    for &slot in retired.iter().rev() {
        let position = slot as usize;
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
    let shifted = |limit: usize| limit - retired.range(..limit as VarId).count();
    declaration.positional_only = declaration.positional_only.map(shifted);
    declaration.keyword_only = declaration.keyword_only.map(shifted);
    (view, declaration)
}

/// Whether `name`, so declared, is a function whose arguments the pass may
/// remove ([`unused_arguments`]).
fn reducible(name: &str, function: &MirFunction, declaration: &MirFunctionDeclaration) -> bool {
    name != "__toplevel__"
        && !declaration.has_receiver
        && mojito_symbol::symbol::split_method_symbol(name).is_none()
        && mojito_symbol::symbol::lifted_owner(name).is_none()
        && declaration.enclosing.is_none()
        && declaration.region_binders.is_empty()
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none()
        && !declaration.returns_reference
        && declaration.param_names.len() == function.n_params
        && !declaration
            .param_types
            .iter()
            .chain(std::iter::once(&declaration.ret_ty))
            .any(names_a_place)
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
    let cleanup = |slots: &[VarId], used: &mut BTreeSet<VarId>| {
        used.extend(slots.iter().copied().filter(|&slot| !trivial(slot)));
    };
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::DropVar { var } if trivial(*var) => {}
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
            |instruction| !matches!(instruction, MirInstr::DropVar { var } if retired.contains(var)),
        );
        for instruction in &mut block.instrs {
            if let MirInstr::Try { cleanup, .. } = instruction {
                cleanup.retain(|slot| !retired.contains(slot));
            }
            for region in instruction_regions_mut(instruction) {
                remove_drops(region, retired);
            }
        }
        if let MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } =
            &mut block.term
        {
            cleanup.retain(|slot| !retired.contains(slot));
        }
    }
}
