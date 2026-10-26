//! `sroa`: upstream's `SROA` (`ReplaceStructs`), which replaces a struct
//! allocation by one allocation per field, so that `mem-2-reg` can promote
//! each field a body reads and writes through field places.
//!
//! A MIR variable slot is the allocation. One is split when it is a local
//! (an argument `imm-arguments` or `lower-arg-conventions` stored into one
//! included, [`splittable_parameters`]) of a declared struct type whose
//! fields its arguments resolve, none of them a pointer or a `ref`,
//! whatever its fields' types and whatever its drop runs, as upstream
//! tests only that the element types resolve, and every
//! mention of it is one of:
//! - a whole store (`var.store`, or `place.store` on the whole slot), which
//!   becomes one `field.get` of the stored value and one `var.store` per
//!   field, as upstream's whole store becomes a `kgen.struct.extract` and a
//!   store per element;
//! - a whole read that runs no constructor (`var.use`, or `place.load` of
//!   the whole slot), which becomes one read per field and a
//!   `struct.create` of them;
//! - a lifetime marker (`var.declare`, `drop.var`, or `consume.var`),
//!   which marks each field's slot instead, but for a field some instruction ends whole (a field
//!   move), which the slot's drop or consumption does not end again;
//! - an instruction each of whose places on the slot projects a declared
//!   field first, which then addresses that field's slot: upstream replaces
//!   each `struct_gep` by its element's allocation, whatever its users, so
//!   a borrowed field keeps only its own slot. A `place.store` left on a
//!   whole field slot becomes a `var.store`, a `drop.place` a `drop.var`,
//!   and a `place.move` a `forward` read, so a nested struct's field slot
//!   splits in the next round.
//!
//! Any other mention keeps the slot whole: a borrow, a capture, a loan
//! interior, a `through` place, or a whole read that names a
//! constructor, as any other user of upstream's allocation does. So does
//! an address user of a memory-only slot, as `getRegisterPassability`
//! passes a memory-only value to and from a call by address
//! ([`MirDeclarations::register_passable`]): a store of a call's result, a
//! whole read a call takes, such as a destructor's `deinit self`, which
//! upstream's `emitDestructorCall` passes by reference, so the slot splits
//! once `inline-parametric` has inlined the destructor. Each field
//! slot is typed at the slot type's arguments, as upstream's element
//! allocation has the `!kgen.struct` element type, so a generic body splits
//! a slot of its own struct application (`Arr[n]`). A slot
//! nothing stores, declares, or marks is a compile-time parameter's
//! binding, which the elaborator substitutes, and is never split.
//!
//! The pass never reads control flow, so it splits structured and lowered
//! bodies alike. It repeats until nothing splits, at most ten times
//! (upstream's `loopLimit`), so a nested struct splits one level a round.

use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::{
    instruction_operand_regs, instruction_places, instruction_places_mut, instruction_result_regs,
};
use mojito_mir::mir::{
    MirBlock, MirCaptureMode, MirDeclarations, MirFunction, MirInstr, MirPlace, MirTerm, Proj, Reg,
    UseMode, fresh_register, instruction_regions, instruction_regions_mut, instruction_slots,
    retire_slots, terminator_regs,
};
use mojito_types::types::{Ty, TyKind};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The pass, under upstream's flag spelling.
pub const SROA: BodyPass = BodyPass {
    name: "sroa",
    run: sroa,
};

/// Split each local struct slot of `function` that upstream's `SROA` would
/// split into one slot per field, and remove it from the function; returns
/// whether any was.
pub fn sroa(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    let mut retired = BTreeSet::new();
    let mut field_slots = 0;
    for _ in 0..LOOP_LIMIT {
        let candidates = splittable_slots(function, declarations);
        if candidates.is_empty() {
            break;
        }
        let mut split = HashMap::new();
        for Candidate { var, fields, ended } in candidates {
            let base = function.var_names[var as usize].clone();
            let slots: Rc<[FieldSlot]> = fields
                .into_iter()
                .map(|(name, ty)| {
                    let slot = function.n_vars as VarId;
                    function.n_vars += 1;
                    function.var_names.push(format!("{base}.{name}"));
                    function.var_tys.insert(slot, ty.clone());
                    let ended = ended.contains(&name);
                    FieldSlot {
                        name,
                        slot,
                        ty,
                        ended,
                    }
                })
                .collect();
            field_slots += slots.len();
            split.insert(var, slots);
            retired.insert(var);
        }
        let mut blocks = std::mem::take(&mut function.blocks);
        Splitter { split, function }.rewrite(&mut blocks);
        function.blocks = blocks;
    }
    if retired.is_empty() {
        return false;
    }
    for (_, origin) in function.spans.0.values_mut() {
        if origin.is_some_and(|var| retired.contains(&var)) {
            *origin = None;
        }
    }
    retire_slots(function, &retired);
    timing::count("split_slots", retired.len() as u64);
    timing::count("split_field_slots", field_slots as u64);
    true
}

/// The parameters among `params` of `function` whose slot [`sroa`] would
/// split were it a local the entry stores whole, as
/// `lower-arg-conventions` stores one: upstream's `SROA` splits the stack
/// allocation `LowerArgConventions` stores a register-passable argument
/// into, whatever reads it.
pub fn splittable_parameters(
    function: &MirFunction,
    declarations: &MirDeclarations,
    params: &BTreeSet<VarId>,
) -> BTreeSet<VarId> {
    let fields = judged_slots(function, declarations, |var| params.contains(&var));
    unkept_census(function, &fields)
        .into_keys()
        .filter(|var| fields.contains_key(var))
        .collect()
}

/// Upstream's `loopLimit`: the most rounds one run splits.
const LOOP_LIMIT: usize = 10;

/// One field's slot of a split slot.
struct FieldSlot {
    name: String,
    slot: VarId,
    ty: Ty,
    /// Whether some instruction ends the field whole (a field move), so
    /// the slot's drop or consumption, which ends only what is left, does
    /// not end it again.
    ended: bool,
}

/// A whole mention of a slot, which a split slot expands per field.
#[derive(Clone, Copy)]
enum Whole {
    /// A store of the register.
    Store(Reg),
    /// A read into the register, by `var.use` in this mode, or by a
    /// `place.load` when `None`.
    Read {
        dest: Reg,
        mode: Option<UseMode>,
    },
    Declare,
    Drop,
    Consume,
}

/// A slot a round splits.
struct Candidate {
    var: VarId,
    /// Its struct's declared fields.
    fields: Vec<(String, Ty)>,
    /// The fields some instruction ends whole.
    ended: BTreeSet<String>,
}

/// A struct's fields at a slot type's arguments, each a name and a type.
type Fields<'a> = Cow<'a, [(String, Ty)]>;

/// The census of one candidate slot.
#[derive(Default)]
struct Census {
    kept: bool,
    /// Whether some mention stores, declares, or marks it.
    defined: bool,
    /// The fields some instruction ends whole.
    ended: BTreeSet<String>,
    /// The registers its whole stores store.
    stores: Vec<Reg>,
    /// The registers its whole reads define.
    reads: Vec<Reg>,
}

/// A candidate slot's fields, and whether its type is memory-only
/// ([`MirDeclarations::register_passable`] answers no).
type Judged<'a> = (Fields<'a>, bool);

/// How the instruction defining a register defines it, as far as a
/// memory-only slot's whole store asks.
#[derive(Clone, Copy)]
enum Def {
    /// A whole read of a slot that runs no constructor, a `struct.create`,
    /// or a `field.get`, none of which passes its operand by address.
    Value,
    /// Anything else: a call result, which upstream returns into its
    /// destination's address, an operator, a region result, a reference
    /// read.
    Other,
}

/// The registers of a body as the address rules ask about them.
#[derive(Default)]
struct Registers {
    /// Each register's definition, by number.
    defs: HashMap<u32, Def>,
    /// The registers some instruction or terminator takes other than as a
    /// store's source, a `struct.create` field, a `field.get` base, or a
    /// `yield`, `break`, `continue`, or `return` value.
    taken: HashSet<u32>,
}

impl Registers {
    /// Record how each instruction of `blocks` and the regions they hold
    /// defines and takes its registers.
    fn record(&mut self, blocks: &[MirBlock]) {
        let mut regs = Vec::new();
        for block in blocks {
            for instruction in &block.instrs {
                for region in instruction_regions(instruction) {
                    self.record(region);
                }
                regs.clear();
                instruction_result_regs(instruction, &mut regs);
                let def = match (instruction, whole_mention(instruction)) {
                    (_, Some((_, Whole::Read { .. })))
                    | (MirInstr::MakeStruct { .. } | MirInstr::GetField { .. }, _) => Def::Value,
                    _ => Def::Other,
                };
                self.defs.extend(regs.iter().map(|reg| (reg.0, def)));
                regs.clear();
                instruction_operand_regs(instruction, &mut regs);
                let exempt: &[Reg] = match instruction {
                    MirInstr::DefVar { src, .. } | MirInstr::Store { src, .. } => {
                        std::slice::from_ref(src)
                    }
                    MirInstr::MakeStruct { fields, .. } => fields,
                    MirInstr::GetField { base, .. } => std::slice::from_ref(base),
                    _ => &[],
                };
                for reg in exempt {
                    if let Some(at) = regs.iter().position(|taken| taken == reg) {
                        regs.swap_remove(at);
                    }
                }
                self.taken.extend(regs.iter().map(|reg| reg.0));
            }
            if !matches!(
                block.term,
                MirTerm::Return(_)
                    | MirTerm::Yield(_)
                    | MirTerm::Break { .. }
                    | MirTerm::Continue { .. }
            ) {
                self.taken
                    .extend(terminator_regs(&block.term).iter().map(|reg| reg.0));
            }
        }
    }
}

/// The slots of `function` a round splits, each with its fields, sorted:
/// never its error slot, which a raise out of it hands out whole.
fn splittable_slots(function: &MirFunction, declarations: &MirDeclarations) -> Vec<Candidate> {
    let fields = judged_slots(function, declarations, |var| {
        var as usize >= function.n_params && function.error_slot != Some(var)
    });
    let mut slots: Vec<Candidate> = unkept_census(function, &fields)
        .into_iter()
        .filter(|(_, entry)| entry.defined)
        .filter_map(|(var, entry)| {
            Some(Candidate {
                var,
                fields: fields.get(&var)?.0.to_vec(),
                ended: entry.ended,
            })
        })
        .collect();
    slots.sort_unstable_by_key(|candidate| candidate.var);
    slots
}

/// The slots of `function` that `admitted` admits and whose type
/// [`splittable_type`] splits, each with its fields and whether its type
/// is memory-only.
fn judged_slots<'a>(
    function: &MirFunction,
    declarations: &'a MirDeclarations,
    admitted: impl Fn(VarId) -> bool,
) -> HashMap<VarId, Judged<'a>> {
    let mut judged: HashMap<&Ty, Option<Judged<'_>>> = HashMap::new();
    function
        .var_tys
        .iter()
        .filter(|&(&var, _)| admitted(var))
        .filter_map(|(&var, ty)| {
            let fields = judged.entry(ty).or_insert_with(|| {
                splittable_type(ty, declarations)
                    .map(|fields| (fields, !declarations.register_passable(ty)))
            });
            Some((var, fields.clone()?))
        })
        .collect()
}

/// The census of each slot `fields` names that `function` mentions and no
/// mention keeps, an address user of a memory-only one included
/// ([`keep_address_users`]).
fn unkept_census(
    function: &MirFunction,
    fields: &HashMap<VarId, Judged<'_>>,
) -> HashMap<VarId, Census> {
    if fields.is_empty() {
        return HashMap::new();
    }
    let mut census = HashMap::new();
    take_census(&function.blocks, fields, &mut census);
    if fields.values().any(|(_, memory_only)| *memory_only) {
        let mut registers = Registers::default();
        registers.record(&function.blocks);
        keep_address_users(&mut census, fields, &registers);
    }
    census.retain(|_, entry| !entry.kept);
    census
}

/// The fields of a slot of `ty` [`sroa`] may split, at its arguments
/// ([`MirDeclarations::field_types`]): any declared struct whose fields
/// they resolve, as upstream's only test is that the element types are,
/// whatever its fields' types and whatever its drop runs. A struct with a
/// pointer or `ref` field is never one: the VM reads a place on
/// a whole slot that holds a reference handle through the handle, and a
/// place past a `ref` field's own slot would need that slot as its
/// `through`, which the rewrite does not give it, so the field's own slot
/// would not read as the field did.
fn splittable_type<'a>(ty: &Ty, declarations: &'a MirDeclarations) -> Option<Fields<'a>> {
    if !matches!(ty.kind(), TyKind::Struct(..)) {
        return None;
    }
    let fields = declarations.field_types(ty)?;
    fields
        .iter()
        .all(|(_, field)| !matches!(field.kind(), TyKind::Pointer { .. } | TyKind::Ref(_)))
        .then_some(fields)
}

/// Keep each memory-only candidate in `census` that upstream passes by
/// address to an op other than a field address, load, or store, as
/// `getRegisterPassability` makes a memory-only value's call operand and
/// result an address (`OwnedMem`, `ImmMem`, `DeinitMem`, `ByRefResult`):
/// - a whole store of a register any instruction but a constructor-less
///   whole read, a `struct.create`, or a `field.get` defines, such as a
///   call result, which upstream's callee initializes through the slot's
///   address;
/// - a whole read whose register an instruction takes ([`Registers`]), such
///   as a call argument or a `__deinit__` receiver.
fn keep_address_users(
    census: &mut HashMap<VarId, Census>,
    fields: &HashMap<VarId, Judged<'_>>,
    registers: &Registers,
) {
    let memory_only = |var: &VarId| fields.get(var).is_some_and(|(_, memory_only)| *memory_only);
    for (var, entry) in census.iter_mut() {
        if memory_only(var) {
            entry.kept |= entry
                .stores
                .iter()
                .any(|src| matches!(registers.defs.get(&src.0), None | Some(Def::Other)))
                || entry
                    .reads
                    .iter()
                    .any(|dest| registers.taken.contains(&dest.0));
        }
    }
}

/// Record every mention of the candidate slots `fields` names in `blocks`
/// and the regions they hold in `census`.
fn take_census(
    blocks: &[MirBlock],
    fields: &HashMap<VarId, Judged<'_>>,
    census: &mut HashMap<VarId, Census>,
) {
    for block in blocks {
        for instruction in &block.instrs {
            for region in instruction_regions(instruction) {
                take_census(region, fields, census);
            }
            let slots = instruction_slots(instruction);
            if let Some((var, whole)) = whole_mention(instruction)
                && slots == [var]
            {
                let entry = census.entry(var).or_default();
                entry.defined |= matches!(whole, Whole::Store(_) | Whole::Declare);
                match whole {
                    Whole::Store(src) => entry.stores.push(src),
                    Whole::Read { dest, .. } => entry.reads.push(dest),
                    _ => {}
                }
                continue;
            }
            let places = instruction_places(instruction);
            let distinct: BTreeSet<VarId> = slots.iter().copied().collect();
            for var in distinct {
                let Some((declared, _)) = fields.get(&var) else {
                    continue;
                };
                let mentions = slots.iter().filter(|&&slot| slot == var).count();
                let projected = places
                    .iter()
                    .filter(|place| place.root == var && first_field(place, declared).is_some())
                    .count();
                let entry = census.entry(var).or_default();
                entry.kept |= mentions != projected;
                entry.ended.extend(
                    ended_places(instruction)
                        .filter(|place| {
                            place.root == var
                                && place.proj.get(1..).is_some_and(|rest| {
                                    rest.iter().all(|step| matches!(step, Proj::Rebind))
                                })
                        })
                        .filter_map(|place| first_field(place, declared))
                        .map(String::from),
                );
            }
        }
    }
}

/// The places `instruction` ends: a place move or drop, the receiver
/// place of a call whose `deinit` receiver consumes it in place, as
/// upstream's call consumes a `deinit` operand's address, and a closure's
/// move captures.
fn ended_places(instruction: &MirInstr) -> impl Iterator<Item = &MirPlace> {
    let (single, captures) = match instruction {
        MirInstr::MovePlace { place, .. } | MirInstr::DropPlace { place, .. } => {
            (Some(place), &[][..])
        }
        MirInstr::MethodCall { .. } => (instruction.consumed_receiver(), &[][..]),
        MirInstr::MakeClosure { captures, .. } => (None, captures.as_slice()),
        _ => (None, &[][..]),
    };
    single.into_iter().chain(
        captures
            .iter()
            .filter(|capture| capture.mode == MirCaptureMode::Move)
            .map(|capture| &capture.place),
    )
}

/// The declared field `place` projects first, directly on its root.
fn first_field<'a>(place: &MirPlace, declared: &'a [(String, Ty)]) -> Option<&'a str> {
    let Some(Proj::Field(field)) = place.proj.first() else {
        return None;
    };
    if place.through.is_some() {
        return None;
    }
    declared
        .iter()
        .find(|(name, _)| name == field)
        .map(|(name, _)| name.as_str())
}

/// The slot `instruction` mentions whole, and how, when a split slot
/// expands it per field.
fn whole_mention(instruction: &MirInstr) -> Option<(VarId, Whole)> {
    let whole = |place: &MirPlace| place.proj.is_empty() && place.through.is_none();
    match instruction {
        MirInstr::DefVar { var, src, .. } => Some((*var, Whole::Store(*src))),
        MirInstr::Store { place, src } if whole(place) => Some((place.root, Whole::Store(*src))),
        MirInstr::UseVar {
            dest,
            var,
            mode: mode @ (UseMode::Copy | UseMode::Move | UseMode::Forward),
            constructor: None,
        } => Some((
            *var,
            Whole::Read {
                dest: *dest,
                mode: Some(*mode),
            },
        )),
        MirInstr::LoadPlace { dest, place } if whole(place) => Some((
            place.root,
            Whole::Read {
                dest: *dest,
                mode: None,
            },
        )),
        MirInstr::DeclareVar { var } => Some((*var, Whole::Declare)),
        MirInstr::DropVar {
            var,
            destructor: None,
        } => Some((*var, Whole::Drop)),
        MirInstr::ConsumeVar { var } => Some((*var, Whole::Consume)),
        _ => None,
    }
}

/// One round's rewrite: each mention of a split slot becomes its fields'.
struct Splitter<'a> {
    split: HashMap<VarId, Rc<[FieldSlot]>>,
    /// The function, its blocks taken out.
    function: &'a mut MirFunction,
}

impl Splitter<'_> {
    fn rewrite(&mut self, blocks: &mut [MirBlock]) {
        for block in blocks {
            let mut out = Vec::with_capacity(block.instrs.len());
            for mut instruction in std::mem::take(&mut block.instrs) {
                for region in instruction_regions_mut(&mut instruction) {
                    self.rewrite(region);
                }
                if let Some((var, whole)) = whole_mention(&instruction)
                    && let Some(fields) = self.split.get(&var).cloned()
                {
                    self.expand(whole, &fields, &mut out);
                    continue;
                }
                self.reroot(&mut instruction);
                out.push(instruction);
            }
            block.instrs = out;
        }
    }

    /// The per-field instructions of the whole mention `whole` of a slot
    /// split into `fields`, pushed onto `out`.
    fn expand(&mut self, whole: Whole, fields: &[FieldSlot], out: &mut Vec<MirInstr>) {
        match whole {
            Whole::Store(src) => {
                for field in fields {
                    let value = self.fresh_register(&field.ty, src);
                    out.push(MirInstr::GetField {
                        dest: value,
                        base: src,
                        field: field.name.clone(),
                    });
                    out.push(MirInstr::DefVar {
                        var: field.slot,
                        src: value,
                        binding_ty: Some(field.ty.clone()),
                        in_place: false,
                    });
                }
            }
            Whole::Read { dest, mode } => {
                let values = fields
                    .iter()
                    .map(|field| {
                        let value = self.fresh_register(&field.ty, dest);
                        out.push(match mode {
                            Some(mode) => MirInstr::UseVar {
                                dest: value,
                                var: field.slot,
                                mode,
                                constructor: None,
                            },
                            None => MirInstr::LoadPlace {
                                dest: value,
                                place: MirPlace::root(field.slot, Some(field.ty.clone())),
                            },
                        });
                        value
                    })
                    .collect();
                out.push(MirInstr::MakeStruct {
                    dest,
                    fields: values,
                });
            }
            Whole::Declare => out.extend(
                fields
                    .iter()
                    .map(|field| MirInstr::DeclareVar { var: field.slot }),
            ),
            Whole::Drop => out.extend(fields.iter().filter(|field| !field.ended).map(|field| {
                MirInstr::DropVar {
                    var: field.slot,
                    destructor: None,
                }
            })),
            Whole::Consume => out.extend(
                fields
                    .iter()
                    .filter(|field| !field.ended)
                    .map(|field| MirInstr::ConsumeVar { var: field.slot }),
            ),
        }
    }

    /// Readdress each place of `instruction` on a split slot to the slot of
    /// the field it projects first; a store left on a whole field slot
    /// becomes a `var.store`, a drop a `drop.var`, and a move a `forward`
    /// read, which, as the move, runs no move initializer and vacates the
    /// slot.
    fn reroot(&self, instruction: &mut MirInstr) {
        let mut rerooted = false;
        for place in instruction_places_mut(instruction) {
            let Some(fields) = self.split.get(&place.root) else {
                continue;
            };
            let Some(Proj::Field(name)) = place.proj.first() else {
                continue;
            };
            let Some(field) = fields.iter().find(|field| field.name == *name) else {
                continue;
            };
            place.root = field.slot;
            place.root_ty = Some(field.ty.clone());
            place.proj.remove(0);
            if !place.projection_tys.is_empty() {
                place.projection_tys.remove(0);
            }
            rerooted = true;
        }
        if !rerooted {
            return;
        }
        match instruction {
            MirInstr::Store { place, src } if place.proj.is_empty() => {
                *instruction = MirInstr::DefVar {
                    var: place.root,
                    src: *src,
                    binding_ty: place.root_ty.clone(),
                    in_place: false,
                };
            }
            MirInstr::DropPlace { place, destructor } if place.proj.is_empty() => {
                *instruction = MirInstr::DropVar {
                    var: place.root,
                    destructor: destructor.take(),
                };
            }
            MirInstr::MovePlace { dest, place } if place.proj.is_empty() => {
                *instruction = MirInstr::UseVar {
                    dest: *dest,
                    var: place.root,
                    mode: UseMode::Forward,
                    constructor: None,
                };
            }
            _ => {}
        }
    }

    /// A fresh register of type `ty`, at the span of the register `at`.
    fn fresh_register(&mut self, ty: &Ty, at: Reg) -> Reg {
        let span = self
            .function
            .spans
            .0
            .get(&at.0)
            .map(|(span, _)| span.clone());
        fresh_register(self.function, ty.clone(), span.as_ref())
    }
}
