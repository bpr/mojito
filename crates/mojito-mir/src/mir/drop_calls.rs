//! Drops lowered to calls of their destructor.
//!
//! Upstream's destructor insertion leaves, at each point a value dies, a
//! plain call of its type's `__del__` consuming the storage, a field's
//! included, which it calls on the field's address. Drop elaboration
//! decides every drop where it places it: a value live at a point is
//! initialized there, since the move scan rejects every use of one only
//! some paths initialize, so a `drop.var` or `drop.place` naming its
//! destructor destroys unconditionally. This pass writes the call, as
//! `copy_calls.rs` writes a copy's, as a call of the named destructor on
//! the `deinit self` receiver. A dying memory-only slot or part becomes a
//! mutable `ref.make` of its storage, the call's `DeinitMem` operand, whose
//! call consumes the referenced storage as upstream's destructor call
//! consumes the dead variable's or field's address. A dying
//! register-passable slot becomes a forwarding use of it, which ends it and
//! hands its value over as Mojo's owned argument convention does, and a
//! register-passable part is moved out of its storage into the owned value
//! the call takes. A dying register is the call's receiver itself, as
//! upstream's destructor call consumes an owned SSA value.

use super::legacy_places::{LegacyCursor, LegacyOperandTables};
use super::{
    MirBlock, MirFunction, MirInstr, MirPlace, Reg, UseMode, VarId, fresh_register,
    instruction_regions_mut,
};
use mojito_common::token::SourceSpan;
use mojito_types::types::{Ty, TyKind};

/// Lower every named drop of `function`, in its regions too, to a call of
/// its destructor.
///
/// A destructor `unconditional` does not hold of, one available only where
/// a `where` clause holds, stays a drop for the elaborator to name per
/// instance. `register_passable` judges a dying slot's or part's type,
/// which decides whether the call takes it by address or as a value.
pub fn lower_drops(
    function: &mut MirFunction,
    unconditional: &dyn Fn(&str) -> bool,
    register_passable: &dyn Fn(&Ty) -> bool,
) {
    let judge = Judge {
        unconditional,
        register_passable,
    };
    let mut blocks = std::mem::take(&mut function.body);
    lower_list(&mut blocks, &judge, function);
    function.body = blocks;
}

/// Pass the receiver of every `deinit`-receiver call of `functions` that
/// names its consumed storage by its receiver place, as MIR text before
/// schema 1.200 does and the reader keeps in `legacy`, as the operand
/// upstream's call consumes.
///
/// A memory-only receiver becomes a mutable `ref.make` of the storage, and
/// a register-passable one the owned value moved out of it
/// ([`MirInstr::take_place`]). The load defining the receiver becomes that
/// operand; where none precedes the call in its block, a fresh one does.
pub fn upgrade_deinit_receivers(
    functions: &mut [(String, MirFunction)],
    declarations: &super::MirDeclarations,
    legacy: &LegacyOperandTables,
) {
    for (index, (_, function)) in functions.iter_mut().enumerate() {
        let mut body = std::mem::take(&mut function.body);
        upgrade_deinit_receivers_in(&mut body, function, declarations, &mut legacy.cursor(index));
        function.body = body;
    }
}

/// The predicates [`lower_drops`] lowers by.
struct Judge<'p> {
    unconditional: &'p dyn Fn(&str) -> bool,
    register_passable: &'p dyn Fn(&Ty) -> bool,
}

/// Lower the named drops of the block list `list` and of its regions.
fn lower_list(block: &mut MirBlock, judge: &Judge<'_>, function: &mut MirFunction) {
    let instructions = std::mem::take(&mut block.instrs);
    block.instrs = Vec::with_capacity(instructions.len());
    for mut instruction in instructions {
        for region in instruction_regions_mut(&mut instruction) {
            lower_list(region, judge, function);
        }
        match lowered(&instruction, judge, function) {
            Some(lowered) => block.instrs.extend(lowered),
            None => block.instrs.push(instruction),
        }
    }
}

/// The instructions a named drop lowers to, or `None` for any other
/// instruction.
fn lowered(
    instruction: &MirInstr,
    judge: &Judge<'_>,
    function: &mut MirFunction,
) -> Option<Vec<MirInstr>> {
    let (dying, destructor) = match instruction {
        MirInstr::DropVar {
            var,
            destructor: Some(destructor),
        } => (Dying::Var(*var), destructor),
        MirInstr::DropPlace {
            place,
            destructor: Some(destructor),
        } => (Dying::Place(place), destructor),
        MirInstr::Drop {
            reg,
            destructor: Some(destructor),
        } => (Dying::Reg(*reg), destructor),
        _ => return None,
    };
    if !(judge.unconditional)(destructor) {
        return None;
    }
    let (recv, load) = match dying {
        Dying::Reg(reg) => (reg, None),
        Dying::Var(var) => {
            let ty = function.var_tys.get(&var).cloned()?;
            let span = death_span(function, var);
            if (judge.register_passable)(&ty) {
                let recv = fresh_register(function, ty, span.as_ref());
                let load = MirInstr::UseVar {
                    dest: recv,
                    var,
                    mode: UseMode::Forward,
                    constructor: None,
                };
                (recv, Some(load))
            } else {
                let place = MirPlace::root(var, Some(ty.clone()));
                consumed_by_address(function, place, ty, span.as_ref())
            }
        }
        Dying::Place(place) => {
            let ty = place.ty.clone()?;
            let span = death_span(function, place.root);
            if (judge.register_passable)(&ty) {
                let recv = fresh_register(function, ty, span.as_ref());
                (recv, Some(MirInstr::take_place(recv, place.clone())))
            } else {
                consumed_by_address(function, place.clone(), ty, span.as_ref())
            }
        }
    };
    let span = function.spans.0.get(&recv.0).map(|(span, _)| span.clone());
    let dest = fresh_register(function, TyKind::None.into(), span.as_ref());
    let call = MirInstr::MethodCall {
        conventions: crate::mir::MirArgConventions::default(),
        dest,
        recv,
        method: "__deinit__".to_string(),
        resolved: Some(destructor.clone()),
        raises: None,
        reference_result: None,
        result_adapter: None,
        args: Vec::new(),
        kwargs: Vec::new(),
        recv_writes: true,
        receiver_convention: Some(mojito_ast::ast::ArgConvention::Deinit),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
        instantiated_args: Vec::new(),
        spread: None,
        refined_bound: None,
    };
    Some(load.into_iter().chain([call]).collect())
}

/// The register holding the mutable `ref.make` of `place`, of the
/// memory-only type `ty`, that a `deinit` call consumes it through: the
/// call's `DeinitMem` operand.
fn consumed_by_address(
    function: &mut MirFunction,
    place: MirPlace,
    ty: Ty,
    span: Option<&SourceSpan>,
) -> (Reg, Option<MirInstr>) {
    let handle = consumed_place_handle(&place, function.binds_reference(place.root), ty);
    let recv = fresh_register(function, handle, span);
    (recv, Some(MirInstr::MakeRef { dest: recv, place }))
}

/// The type of the mutable reference a `deinit` call consumes `place`,
/// of type `ty`, through; `binds_root` says whether its root is a `ref`
/// binding.
fn consumed_place_handle(place: &MirPlace, binds_root: bool, ty: Ty) -> Ty {
    super::mir_place_handle_ty(
        place,
        binds_root,
        Some(mojito_types::origin::Mutability::Mutable),
    )
    .unwrap_or_else(|| {
        TyKind::Ref(mojito_types::origin::RefTy {
            referent: ty,
            origin: mojito_types::origin::Origin::Untracked { mutable: true },
            mutability: mojito_types::origin::Mutability::Mutable,
            address_space: mojito_types::types::AddressSpaceSlot::GENERIC,
        })
        .into()
    })
}

/// [`upgrade_deinit_receivers`] over `block` and its regions.
fn upgrade_deinit_receivers_in(
    block: &mut MirBlock,
    function: &mut MirFunction,
    declarations: &super::MirDeclarations,
    cursor: &mut LegacyCursor<'_>,
) {
    let mut index = 0;
    while index < block.instrs.len() {
        for region in instruction_regions_mut(&mut block.instrs[index]) {
            upgrade_deinit_receivers_in(region, function, declarations, cursor);
        }
        let instruction = &block.instrs[index];
        let place = cursor
            .visit(instruction)
            .filter(|_| instruction.deinit_receiver())
            .and_then(|places| places.receiver.clone());
        let (Some(place), MirInstr::MethodCall { recv, .. }) = (place, instruction) else {
            index += 1;
            continue;
        };
        let register = *recv;
        let ty = place
            .ty
            .clone()
            .or_else(|| function.reg_types.get(&register.0).cloned());
        let register_passable = ty
            .as_ref()
            .is_some_and(|ty| declarations.register_passable(ty));
        let loaded = block.instrs[..index].iter().rposition(|instruction| {
            matches!(
                instruction,
                MirInstr::LoadPlace { dest, place: read }
                    if *dest == register
                        && super::call_addresses::same_storage(read, &place)
                        && read.through == place.through
            )
        });
        let receiver = match (loaded, register_passable) {
            (Some(at), true) => {
                block.instrs[at] = MirInstr::take_place(register, place);
                register
            }
            (Some(at), false) => {
                let handle = consumed_place_handle(
                    &place,
                    function.binds_reference(place.root),
                    ty.unwrap_or_else(Ty::error),
                );
                function.reg_types.insert(register.0, handle);
                block.instrs[at] = MirInstr::MakeRef {
                    dest: register,
                    place,
                };
                register
            }
            (None, true) => {
                let taken = fresh_register(function, ty.unwrap_or_else(Ty::error), None);
                block
                    .instrs
                    .insert(index, MirInstr::take_place(taken, place));
                index += 1;
                taken
            }
            (None, false) => {
                let handle = consumed_place_handle(
                    &place,
                    function.binds_reference(place.root),
                    ty.unwrap_or_else(Ty::error),
                );
                let address = fresh_register(function, handle, None);
                block.instrs.insert(
                    index,
                    MirInstr::MakeRef {
                        dest: address,
                        place,
                    },
                );
                index += 1;
                address
            }
        };
        if let MirInstr::MethodCall { recv, .. } = &mut block.instrs[index] {
            *recv = receiver;
        }
        index += 1;
    }
}

/// What a named drop destroys.
enum Dying<'i> {
    Var(VarId),
    Place(&'i MirPlace),
    Reg(Reg),
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
