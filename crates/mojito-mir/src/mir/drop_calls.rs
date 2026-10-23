//! Drops lowered to calls of their destructor.
//!
//! Upstream's destructor insertion leaves, at each point a value dies, a
//! plain call of its type's `__del__` consuming the storage, a field's
//! included, which it calls on the field's address. Drop elaboration
//! decides every drop where it places it: a value live at a point is
//! initialized there, since the move scan rejects every use of one only
//! some paths initialize, so a `drop.var` or `drop.place` naming its
//! destructor destroys unconditionally. This pass writes the call, as
//! `copy_calls.rs` writes a copy's: a dying slot becomes a forwarding use of
//! it, which ends it and hands its storage to the callee as Mojo's owned
//! argument convention does, and a call of the named destructor on it as the
//! `deinit self` receiver, which is what an explicit `x^.finish()` lowers
//! to; a dying part becomes a load of the part, the call on it with the part
//! as its receiver's place, and a consume of the part, which is what an
//! explicit `x.f^.finish()` lowers to.

use super::{
    MirBlock, MirFunction, MirInstr, UseMode, VarId, fresh_register, instruction_regions_mut,
};
use mojito_common::token::SourceSpan;
use mojito_types::types::TyKind;

/// Lower every named drop of `function`, in its regions too, to a call of
/// its destructor.
///
/// A destructor `unconditional` does not hold of, one available only where
/// a `where` clause holds, stays a drop for the elaborator to name per
/// instance.
pub fn lower_drops(function: &mut MirFunction, unconditional: &dyn Fn(&str) -> bool) {
    let mut blocks = std::mem::take(&mut function.blocks);
    lower_list(&mut blocks, unconditional, function);
    function.blocks = blocks;
}

/// Lower the named drops of the block list `list` and of its regions.
fn lower_list(
    list: &mut [MirBlock],
    unconditional: &dyn Fn(&str) -> bool,
    function: &mut MirFunction,
) {
    for block in list {
        let instructions = std::mem::take(&mut block.instrs);
        block.instrs = Vec::with_capacity(instructions.len());
        for mut instruction in instructions {
            for region in instruction_regions_mut(&mut instruction) {
                lower_list(region, unconditional, function);
            }
            match lowered(&instruction, unconditional, function) {
                Some(lowered) => block.instrs.extend(lowered),
                None => block.instrs.push(instruction),
            }
        }
    }
}

/// The instructions a named drop lowers to, or `None` for any other
/// instruction.
fn lowered(
    instruction: &MirInstr,
    unconditional: &dyn Fn(&str) -> bool,
    function: &mut MirFunction,
) -> Option<Vec<MirInstr>> {
    let (var, place, destructor) = match instruction {
        MirInstr::DropVar {
            var,
            destructor: Some(destructor),
        } => (*var, None, destructor),
        MirInstr::DropPlace {
            place,
            destructor: Some(destructor),
        } => (place.root, Some(place), destructor),
        _ => return None,
    };
    if !unconditional(destructor) {
        return None;
    }
    let ty = match place {
        Some(place) => place.ty.clone(),
        None => function.var_tys.get(&var).cloned(),
    }?;
    let span = death_span(function, var);
    let recv = fresh_register(function, ty, span.as_ref());
    let dest = fresh_register(function, TyKind::None.into(), span.as_ref());
    let call = MirInstr::MethodCall {
        dest,
        recv,
        method: "__deinit__".to_string(),
        resolved: Some(destructor.clone()),
        raises: None,
        reference_result: None,
        result_adapter: None,
        args: Vec::new(),
        kwargs: Vec::new(),
        recv_place: place.cloned().map(Box::new),
        recv_writes: true,
        arg_places: Vec::new(),
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
        instantiated_args: Vec::new(),
        spread: None,
    };
    Some(match place {
        Some(place) => vec![
            MirInstr::LoadPlace {
                dest: recv,
                place: place.clone(),
            },
            call,
            MirInstr::ConsumePlace {
                place: place.clone(),
                marker: recv,
            },
        ],
        None => vec![
            MirInstr::UseVar {
                dest: recv,
                var,
                mode: UseMode::Forward,
                constructor: None,
            },
            call,
        ],
    })
}

/// Where the slot `var` dies: the location of its last read, which a drop
/// of it follows.
fn death_span(function: &MirFunction, var: VarId) -> Option<SourceSpan> {
    function
        .spans
        .0
        .iter()
        .filter(|(_, (_, origin))| *origin == Some(var))
        .max_by_key(|(reg, _)| **reg)
        .map(|(_, (span, _))| span.clone())
}
