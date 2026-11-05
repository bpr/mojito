//! By-address call operands.
//!
//! Upstream's `lit.call` takes an argument its callee receives by address —
//! a `mut` or `ref` parameter, or a `read` one of a memory-only type — as a
//! `!lit.ref` of the caller's storage, and loads nothing. A direct `call`
//! here passes such an argument as a `ref.make` of its retained place
//! ([`address_call_operands`]), and the call's `arg_places` entry names the
//! same storage.
//!
//! A register-passable `read` argument is passed by value, as upstream's
//! `LowerArgConventions` caller half passes it: one an instance's
//! substitution makes register-passable (a `read x: T` at `T = Int`) is read
//! through its reference once elaborated ([`load_register_passable_operands`]).

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, Proj, Reg,
    count_register_reads, instruction_regions, instruction_regions_mut, mir_place_handle_ty,
};
use mojito_ast::ast::ArgConvention;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_symbol::symbol::is_initializer_symbol;
use mojito_types::origin::Mutability;
use mojito_types::types::{Ty, TyKind};
use std::collections::HashMap;

/// The regular parameter each operand of a direct call of `declaration`
/// binds.
///
/// One entry per positional operand, then one per keyword operand,
/// `None` for an operand a collector takes. `None` overall when the call
/// does not bind, or when the callee takes a receiver the call passes
/// among its operands (a method named as a function), whose slots the
/// declaration's parameter lists do not count.
#[must_use]
pub fn call_operand_parameters(
    callee: &str,
    declaration: &MirFunctionDeclaration,
    positional: usize,
    kwargs: &[(String, Reg)],
) -> Option<Vec<Option<usize>>> {
    if declaration.has_receiver && !is_initializer_symbol(callee) {
        return None;
    }
    let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let matched = match_call_slots(
        &declaration.param_names,
        &declaration.required,
        declaration.positional_only,
        declaration.keyword_only,
        positional,
        &keywords,
        CallVariadics {
            positional: declaration.variadic.is_some(),
            keyword: declaration.kw_variadic.is_some(),
        },
    )
    .ok()?;
    let mut parameters = vec![None; positional + kwargs.len()];
    for (parameter, slot) in matched.slots.iter().enumerate() {
        match slot {
            ArgSlot::Positional(operand) => parameters[*operand] = Some(parameter),
            ArgSlot::Keyword(operand) => parameters[positional + *operand] = Some(parameter),
            ArgSlot::Default => {}
        }
    }
    Some(parameters)
}

/// How a callee receives its regular parameter `parameter` by address.
///
/// The answer is the mutability of the reference its operand is, `None`
/// when it is received by value (an owned, `deinit`, or register-passable
/// `read` one).
#[must_use]
pub fn by_address_parameter(
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
    parameter: usize,
) -> Option<Mutability> {
    if declaration.ref_params.get(parameter).copied()? {
        return Some(Mutability::Mutable);
    }
    match declaration.param_conventions.get(parameter).copied()? {
        None | Some(ArgConvention::Imm) => {}
        Some(_) => return None,
    }
    let ty = declaration.param_types.get(parameter)?;
    (!declarations.register_passable(ty)).then_some(Mutability::Immutable)
}

/// The value an operand of `ty` passes: the referent of a reference.
#[must_use]
pub fn operand_value_ty(ty: &Ty) -> &Ty {
    match ty.kind() {
        TyKind::Ref(reference) => &reference.referent,
        _ => ty,
    }
}

/// Pass each by-address operand of every direct call in `functions` as a
/// `ref.make` of its retained place, rather than a whole load of it.
///
/// The operand is rewritten when its one reader is the call and its
/// definition reads the storage the call's `arg_places` entry names: a
/// `place.load` of it, or a `ref.read` of a `ref.make` of it, which goes
/// too when nothing else reads it. A place ending in a `ref` field, whose
/// load reads through the stored handle, keeps its load.
pub fn address_call_operands(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    let callees = Callees::of(declarations);
    for (_, function) in functions.iter_mut() {
        address_function_operands(function, declarations, &callees);
    }
}

/// Read a call's by-address operand through its reference where the
/// callee takes the parameter by value.
///
/// `function` is an elaborated instance, and the read is a `ref.read`, as
/// upstream's `LowerArgConventions` loads a register-passable argument at
/// the call. Returns whether any operand was read.
pub fn load_register_passable_operands(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let callees = Callees::of(declarations);
    let mut next = function.n_regs;
    let mut types = Vec::new();
    let changed = load_in_blocks(
        &mut function.blocks,
        &function.reg_types,
        &mut LoadContext {
            declarations,
            callees: &callees,
            next: &mut next,
            types: &mut types,
        },
    );
    function.n_regs = next;
    for (register, ty, from) in types {
        function.reg_types.insert(register, ty);
        if let Some(span) = function.spans.0.get(&from).cloned() {
            function.spans.0.insert(register, span);
        }
    }
    changed
}

/// The declarations of the functions a program calls, by lowered name:
/// its own, then an imported package's.
struct Callees<'a> {
    declarations: &'a MirDeclarations,
    local: HashMap<&'a str, &'a MirFunctionDeclaration>,
}

impl<'a> Callees<'a> {
    fn of(declarations: &'a MirDeclarations) -> Self {
        Self {
            declarations,
            local: declarations
                .functions
                .iter()
                .map(|declaration| (declaration.lowered_name.as_str(), declaration))
                .collect(),
        }
    }

    fn get(&self, name: &str) -> Option<&'a MirFunctionDeclaration> {
        self.local.get(name).copied().or_else(|| {
            self.declarations
                .imported
                .as_ref()
                .and_then(|imported| imported.function(name))
        })
    }
}

/// What one operand's definition reads.
enum Definition {
    Load(MirPlace),
    Read(Reg),
    Address(MirPlace),
}

/// One rewrite of an operand's definition.
struct Rewrite {
    place: MirPlace,
    mutability: Mutability,
    /// The `ref.make` a rewritten `ref.read` read, which goes with it.
    retired: Option<Reg>,
}

fn address_function_operands(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) {
    let mut definitions = HashMap::new();
    let mut operands = Vec::new();
    collect(
        &function.blocks,
        callees,
        declarations,
        &mut definitions,
        &mut operands,
    );
    if operands.is_empty() {
        return;
    }
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.blocks, &mut reads);
    let read_once = |register: Reg| reads.get(register.0 as usize) == Some(&1);
    let mut rewrites: HashMap<u32, Rewrite> = HashMap::new();
    for (operand, place, mutability) in operands {
        if !read_once(operand) || rewrites.contains_key(&operand.0) {
            continue;
        }
        let rewrite = match definitions.get(&operand.0) {
            Some(Definition::Load(loaded))
                if same_storage(loaded, &place) && loaded.through == place.through =>
            {
                Rewrite {
                    place,
                    mutability,
                    retired: None,
                }
            }
            Some(Definition::Read(reference)) => match definitions.get(&reference.0) {
                Some(Definition::Address(addressed)) if same_storage(addressed, &place) => {
                    Rewrite {
                        place: addressed.clone(),
                        mutability,
                        retired: read_once(*reference).then_some(*reference),
                    }
                }
                _ => continue,
            },
            _ => continue,
        };
        rewrites.insert(operand.0, rewrite);
    }
    if rewrites.is_empty() {
        return;
    }
    for (register, rewrite) in &rewrites {
        let handle =
            mir_place_handle_ty(&rewrite.place, Some(rewrite.mutability)).unwrap_or_else(Ty::error);
        function.reg_types.insert(*register, handle);
    }
    let retired: Vec<u32> = rewrites
        .values()
        .filter_map(|rewrite| rewrite.retired.map(|register| register.0))
        .collect();
    rewrite_blocks(&mut function.blocks, &rewrites, &retired);
}

/// Whether two places name the same storage, whatever local reference
/// either is reached through.
fn same_storage(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.root_ty == right.root_ty
        && left.ty == right.ty
        && left.proj.len() == right.proj.len()
        && left.proj.iter().zip(&right.proj).all(|pair| match pair {
            (Proj::Field(left), Proj::Field(right)) => left == right,
            (Proj::Index(left), Proj::Index(right)) => left == right,
            (Proj::ConstIndex(left), Proj::ConstIndex(right)) => left == right,
            (Proj::Variant(left), Proj::Variant(right)) => left == right,
            (Proj::UninitPayload, Proj::UninitPayload) | (Proj::Rebind, Proj::Rebind) => true,
            _ => false,
        })
}

fn collect(
    blocks: &[MirBlock],
    callees: &Callees<'_>,
    declarations: &MirDeclarations,
    definitions: &mut HashMap<u32, Definition>,
    operands: &mut Vec<(Reg, MirPlace, Mutability)>,
) {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::LoadPlace { dest, place } => {
                    definitions.insert(dest.0, Definition::Load(place.clone()));
                }
                MirInstr::ReadRef { dest, reference } => {
                    definitions.insert(dest.0, Definition::Read(*reference));
                }
                MirInstr::MakeRef { dest, place } => {
                    definitions.insert(dest.0, Definition::Address(place.clone()));
                }
                MirInstr::Call {
                    func,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: None,
                    ..
                } => {
                    let Some(declaration) = callees.get(&func.0) else {
                        continue;
                    };
                    let Some(parameters) =
                        call_operand_parameters(&func.0, declaration, args.len(), kwargs)
                    else {
                        continue;
                    };
                    let registers = args
                        .iter()
                        .chain(kwargs.iter().map(|(_, register)| register));
                    let places = arg_places.iter().chain(kwarg_places);
                    for ((register, place), parameter) in registers.zip(places).zip(parameters) {
                        let (Some(place), Some(parameter)) = (place, parameter) else {
                            continue;
                        };
                        if matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)) | None) {
                            continue;
                        }
                        if let Some(mutability) =
                            by_address_parameter(declaration, declarations, parameter)
                        {
                            operands.push((*register, place.clone(), mutability));
                        }
                    }
                }
                _ => {}
            }
            for region in instruction_regions(instruction) {
                collect(region, callees, declarations, definitions, operands);
            }
        }
    }
}

fn rewrite_blocks(blocks: &mut [MirBlock], rewrites: &HashMap<u32, Rewrite>, retired: &[u32]) {
    for block in blocks {
        block.instrs.retain(|instruction| {
            !matches!(instruction, MirInstr::MakeRef { dest, .. } if retired.contains(&dest.0))
        });
        for instruction in &mut block.instrs {
            let dest = match instruction {
                MirInstr::LoadPlace { dest, .. } | MirInstr::ReadRef { dest, .. } => Some(*dest),
                _ => None,
            };
            if let Some(dest) = dest
                && let Some(rewrite) = rewrites.get(&dest.0)
            {
                *instruction = MirInstr::MakeRef {
                    dest,
                    place: rewrite.place.clone(),
                };
            }
            for region in instruction_regions_mut(instruction) {
                rewrite_blocks(region, rewrites, retired);
            }
        }
    }
}

struct LoadContext<'a, 'b> {
    declarations: &'a MirDeclarations,
    callees: &'a Callees<'b>,
    next: &'a mut u32,
    types: &'a mut Vec<(u32, Ty, u32)>,
}

fn load_in_blocks(
    blocks: &mut [MirBlock],
    reg_types: &HashMap<u32, Ty>,
    context: &mut LoadContext<'_, '_>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            for region in instruction_regions_mut(&mut block.instrs[index]) {
                changed |= load_in_blocks(region, reg_types, context);
            }
            let mut loads = Vec::new();
            if let MirInstr::Call {
                func,
                args,
                kwargs,
                spread: None,
                ..
            } = &mut block.instrs[index]
                && let Some(declaration) = context.callees.get(&func.0)
                && let Some(parameters) =
                    call_operand_parameters(&func.0, declaration, args.len(), kwargs)
            {
                let registers = args
                    .iter_mut()
                    .chain(kwargs.iter_mut().map(|(_, register)| register));
                for (register, parameter) in registers.zip(parameters) {
                    let Some(parameter) = parameter else {
                        continue;
                    };
                    let Some(TyKind::Ref(reference)) = reg_types.get(&register.0).map(Ty::kind)
                    else {
                        continue;
                    };
                    let by_value = !declaration
                        .ref_params
                        .get(parameter)
                        .copied()
                        .unwrap_or(true)
                        && by_address_parameter(declaration, context.declarations, parameter)
                            .is_none()
                        && !matches!(
                            declaration.param_types.get(parameter).map(Ty::kind),
                            Some(TyKind::Ref(_))
                        );
                    if !by_value {
                        continue;
                    }
                    let loaded = Reg(*context.next);
                    *context.next += 1;
                    context
                        .types
                        .push((loaded.0, reference.referent.clone(), register.0));
                    loads.push(MirInstr::ReadRef {
                        dest: loaded,
                        reference: *register,
                    });
                    *register = loaded;
                }
            }
            let inserted = loads.len();
            changed |= inserted > 0;
            block.instrs.splice(index..index, loads);
            index += inserted + 1;
        }
    }
    changed
}
