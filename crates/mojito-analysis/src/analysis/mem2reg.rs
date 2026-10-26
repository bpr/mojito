//! `mem-2-reg`: upstream's `Mem2Reg`, which promotes a stack allocation to
//! SSA values when every user is a load, a store, or a lifetime marker, and
//! `lower-arg-conventions`, the callee half of upstream's
//! `LowerArgConventions`, which stores each register-passable argument into
//! a temporary that `SROA` splits and `Mem2Reg` promotes, and
//! `imm-arguments`, its `read` half before elaboration, where upstream's
//! parser already passes such an argument as one value.
//!
//! A MIR variable slot is the allocation. One is promoted when its type is
//! trivially register-passable ([`MirDeclarations::copy_is_identity`]),
//! whatever binders it names (`SIMD[DType.int64, n]` in a body over `n`),
//! as upstream's `canPromote` has no type test, and every
//! mention of it is a store (`var.store`), a read (`var.use`, or a
//! whole-root `place.load`), or a mention with no effect on such a type
//! once drops are elaborated (`var.declare`, `drop.var`, and `var.consume`).
//! A whole-root `ref.make` of the slot whose register only `ref.read` and
//! `ref.write` take as their reference is the allocation's own address, as
//! an inlined `ref` parameter is upstream: the `ref.make` goes, each
//! `ref.read` is a read and each `ref.write` a store, at its own position.
//! A destructor or copy constructor would take the slot's address, so a
//! type with either is never one, as upstream's never is; a borrow, a
//! capture, a loan, or a projected place keeps the slot too. A slot every
//! store of which is a closure of reference captures is one too: that
//! closure is a struct of pointers, trivially register-passable as
//! upstream's is, and a call through the slot keeps it.
//!
//! A structured body is promoted as upstream's `processRegion` promotes
//! `hlcf`: one scoped walk, each region a scope, in which a store binds the
//! slot's current value, a read becomes a `value.copy` of it (which the
//! `canonicalize` after it forwards), and a declaration, drop, or
//! consumption leaves it, as a lifetime marker does. A slot is allocated in
//! the innermost region holding every mention of it, where its declaration
//! sits; a store or declaration inside a region op makes it variant across
//! that op, and its value is carried through the op's region values: one
//! argument per variant slot for each region a terminator enters (a loop's
//! or `comptime for`'s body, an elif's arm and later conditions, the else
//! region after elifs, a `match` case after one a `match.next` leaves, and
//! its else region, a `try`'s handler when a raise names the `try`, and its
//! `else` when its body completes), init operands for a body the op enters
//! too, and one result per variant slot, which the region terminators
//! define. Where the slot holds nothing on an edge, the edge carries an
//! `uninitmem` constant, upstream's `currValueOrUndef`. A slot nothing
//! stores or declares is a compile-time parameter's binding, which the
//! elaborator substitutes, and keeps its slot.

use super::canonicalize::definition_counts;
use super::pre_elaboration::BodyPass;
use super::sroa::splittable_parameters;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirCase, MirDeclarations, MirElif, MirFunction, MirInstr, MirPlace, MirTerm,
    Reg, RegionFrame, UseMode, count_register_reads, instruction_regions, instruction_regions_mut,
    instruction_slots, loop_continues_mut, match_nexts_mut, region_exit, renumber_block_slots,
    retire_slots, try_raises_mut,
};
use mojito_symbol::symbol::is_initializer_symbol;
use mojito_types::types::Ty;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The pass, under upstream's flag spelling.
pub const MEM_2_REG: BodyPass = BodyPass {
    name: "mem-2-reg",
    run: mem_2_reg,
};

/// The callee half of upstream's `LowerArgConventions`, which the
/// post-elaboration pipeline runs before [`MEM_2_REG`].
pub const LOWER_ARG_CONVENTIONS: BodyPass = BodyPass {
    name: "lower-arg-conventions",
    run: lower_arg_conventions,
};

/// [`imm_arguments`], which the pre-elaboration pipeline runs first, as
/// upstream's parser hands its generators `imm` arguments.
pub const IMM_ARGUMENTS: BodyPass = BodyPass {
    name: "imm-arguments",
    run: imm_arguments,
};

/// Promote each local slot of `function` that upstream's `Mem2Reg` would
/// promote to registers, and remove it from the function; returns whether
/// any was.
pub fn mem_2_reg(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    let (slots, erased_results, erased) = promote_in_regions(function, declarations);
    if slots.is_empty() {
        return false;
    }
    function
        .reg_types
        .retain(|reg, _| !erased_results.contains(reg));
    function
        .spans
        .0
        .retain(|reg, _| !erased_results.contains(reg));
    for (_, origin) in function.spans.0.values_mut() {
        if origin.is_some_and(|var| slots.contains(&var)) {
            *origin = None;
        }
    }
    retire_slots(function, &slots);
    timing::count("promoted_slots", slots.len() as u64);
    timing::count("erased_slot_instructions", erased as u64);
    true
}

/// Store each argument of `function` that [`mem_2_reg`] would promote, or
/// [`sroa`](super::sroa::sroa) split, into a fresh local at the entry.
///
/// Every mention of the argument then names the local; returns whether
/// any was stored. The argument is a `read` or owned parameter of a
/// register-passable type ([`MirDeclarations::register_passable`]), as
/// upstream's `lowerPointerType` lowers one and leaves a memory-only
/// struct behind its address; a `mut` or `deinit` one is never stored, nor
/// an initializer's `out self`, which upstream's `byref_result` makes the
/// function's result. Its own slot, the call ABI's, keeps the one read at
/// the entry.
pub fn lower_arg_conventions(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    store_arguments(name, function, declarations, |_| true)
}

/// Store each `read` argument of `function` that [`mem_2_reg`] would
/// promote, or [`sroa`](super::sroa::sroa) split, into a fresh local at
/// the entry, as [`lower_arg_conventions`] stores it.
///
/// Returns whether any was stored. Upstream's parser passes a `read`
/// argument of a register-passable type by its `imm` convention, one SSA
/// value from the parametric generator on, so its every read is that
/// value, and a field read a `kgen.struct.extract` of it; a parameter slot
/// read once at the entry is that value here. An owned argument stays in
/// its slot, as upstream's `owned_in_mem` one stays in memory until
/// `LowerArgConventions`.
pub fn imm_arguments(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let owned = function.owned_params.clone();
    store_arguments(name, function, declarations, |param| {
        !owned.get(param).copied().unwrap_or(true)
    })
}

/// Store each argument of `function`, the body `name`, that `admitted`
/// admits, of a `read` or owned convention and a register-passable type,
/// which [`mem_2_reg`] would promote or [`splittable_parameters`] answers,
/// into a fresh local at the entry; returns whether any was.
fn store_arguments(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    admitted: impl Fn(usize) -> bool,
) -> bool {
    let out_self = is_initializer_symbol(name);
    let eligible: BTreeSet<VarId> = (0..function.n_params)
        .filter(|&param| {
            admitted(param)
                && (param != 0 || !out_self)
                && !function.ref_params.get(param).copied().unwrap_or(true)
                && !function.deinit_params.get(param).copied().unwrap_or(true)
                && function
                    .param_types
                    .get(param)
                    .is_some_and(|ty| declarations.register_passable(ty))
        })
        .filter_map(|param| VarId::try_from(param).ok())
        .collect();
    if eligible.is_empty() {
        return false;
    }
    let mut stored = splittable_parameters(function, declarations, &eligible);
    stored.extend(
        variance(function, declarations, &eligible)
            .0
            .intersection(&eligible),
    );
    if stored.is_empty() {
        return false;
    }
    let mut entry = Vec::with_capacity(stored.len() * 2);
    for param in stored {
        let ty = function.param_types[param as usize].clone();
        let local = function.n_vars as VarId;
        function.n_vars += 1;
        function
            .var_names
            .push(function.var_names[param as usize].clone());
        function.var_tys.insert(local, ty.clone());
        renumber_block_slots(&mut function.blocks, &|var| {
            if var == param { local } else { var }
        });
        let value = Reg(function.n_regs);
        function.n_regs += 1;
        function.reg_types.insert(value.0, ty.clone());
        // The first register read from the parameter lends its location.
        if let Some(span) = function
            .spans
            .0
            .iter()
            .filter(|(_, (_, origin))| *origin == Some(param))
            .min_by_key(|(reg, _)| **reg)
            .map(|(_, (span, _))| span.clone())
        {
            function.spans.0.insert(value.0, (span, Some(param)));
        }
        // The argument's value, as upstream stores the lowered one: a type
        // whose copy runs a constructor is loaded, which runs none.
        entry.push(if declarations.copy_is_identity(&ty) {
            MirInstr::UseVar {
                dest: value,
                var: param,
                mode: UseMode::Copy,
                constructor: None,
            }
        } else {
            MirInstr::LoadPlace {
                dest: value,
                place: MirPlace::root(param, Some(ty.clone())),
            }
        });
        entry.push(MirInstr::DefVar {
            var: local,
            src: value,
            binding_ty: Some(ty),
            in_place: false,
        });
    }
    let block = &mut function.blocks[0];
    block.instrs.splice(0..0, entry);
    true
}

/// One mention of a candidate slot.
#[derive(Clone, Copy)]
enum Mention {
    /// A store of the register.
    Store(Reg),
    /// A read into the register.
    Read { dest: Reg },
    /// A declaration: the slot holds nothing after it.
    Declare,
    /// A drop or consumption: a lifetime marker, after which the slot holds
    /// nothing.
    Kill,
    /// A `ref.make` of the slot's address, which leaves its value.
    Address,
}

/// The registers each holding the address of a slot, by register number:
/// the dest of a whole-root `ref.make`, defined once, whose every read is
/// the reference of a `ref.read` or `ref.write`.
type Addresses = HashMap<u32, VarId>;

/// The census of one slot: whether some mention keeps it, and each
/// mention with the region holding it (numbered in preorder from the
/// function's own, 0).
#[derive(Default)]
struct Census {
    kept: bool,
    mentions: Vec<(usize, Mention)>,
}

/// The register facts a candidate's mentions are judged by.
struct Facts {
    definitions: Vec<u32>,
    reads: Vec<u32>,
    /// The registers defined by a `closure.make` of reference captures.
    closures: HashSet<u32>,
    addresses: Addresses,
}

impl Facts {
    fn of(function: &mut MirFunction) -> Self {
        let definitions = definition_counts(&mut function.blocks, function.n_regs);
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let mut closures = HashSet::new();
        reference_closures(&function.blocks, &mut closures);
        let mut made = HashMap::new();
        let mut through = vec![0_u32; function.n_regs as usize];
        slot_references(&function.blocks, &mut made, &mut through);
        let addresses = made
            .into_iter()
            .filter(|&(dest, _)| {
                let at = dest as usize;
                definitions.get(at) == Some(&1) && reads.get(at) == through.get(at)
            })
            .collect();
        Self {
            definitions,
            reads,
            closures,
            addresses,
        }
    }

    /// Whether `var`, whose census is `entry`, may be promoted: a local (or
    /// one of the `entry_stored` parameters) of a promotable type, or one
    /// every store of which is a closure of reference captures, kept by no
    /// mention, whose stores each store a register defined once and read
    /// only there, and whose reads read its type.
    fn candidate(
        &self,
        function: &MirFunction,
        declarations: &MirDeclarations,
        entry_stored: &BTreeSet<VarId>,
        var: VarId,
        entry: &Census,
    ) -> bool {
        let typed = function.var_tys.get(&var);
        let stores_closures = entry.mentions.iter().all(|(_, mention)| match *mention {
            Mention::Store(src) => self.closures.contains(&src.0),
            Mention::Read { .. } | Mention::Declare | Mention::Kill | Mention::Address => true,
        });
        (var as usize >= function.n_params || entry_stored.contains(&var))
            && !entry.kept
            && typed.is_some_and(|ty| declarations.copy_is_identity(ty) || stores_closures)
            && entry.mentions.iter().all(|(_, mention)| match *mention {
                Mention::Store(src) => {
                    function.reg_types.get(&src.0) == typed
                        && self.definitions.get(src.0 as usize) == Some(&1)
                        && self.reads.get(src.0 as usize) == Some(&1)
                }
                Mention::Read { dest, .. } => function.reg_types.get(&dest.0) == typed,
                Mention::Declare | Mention::Kill | Mention::Address => true,
            })
    }
}

/// Promote the slots of the structured body `function` that upstream's
/// `canPromote` accepts, as `processRegion` does: returns the slots, the
/// results of the instructions that went, and how many went.
fn promote_in_regions(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> (BTreeSet<VarId>, HashSet<u32>, usize) {
    let (slots, variants, addresses) = variance(function, declarations, &BTreeSet::new());
    if slots.is_empty() {
        return (slots, HashSet::new(), 0);
    }
    let mut blocks = std::mem::take(&mut function.blocks);
    let mut promoter = Promoter {
        var_tys: &function.var_tys,
        slots: &slots,
        addresses: &addresses,
        variants,
        scopes: Vec::new(),
        frames: Vec::new(),
        next_op: 0,
        n_regs: function.n_regs,
        fresh: Vec::new(),
        erased_results: HashSet::new(),
        erased: 0,
    };
    promoter.walk_region(&mut blocks, None, &Rc::from([]), &[]);
    let Promoter {
        n_regs,
        fresh,
        erased_results,
        erased,
        ..
    } = promoter;
    function.blocks = blocks;
    function.n_regs = n_regs;
    function.reg_types.extend(fresh);
    (slots, erased_results, erased)
}

/// The slots of the structured body `function` [`mem_2_reg`] promotes, for
/// each region op, by its preorder number, the promoted slots variant
/// across it, each list sorted, and the registers holding a slot's address.
///
/// A slot is allocated in the innermost region holding every mention of it,
/// where its declaration sits, as upstream's `pop.stack_allocation` does.
/// Its stores and declarations make it variant across each op between them
/// and that region (`userCrossesFunctionCFG`'s `enclosingNodes`), a `try`
/// as any other. Each of the `entry_stored` parameters counts as stored at
/// the entry, as [`promotable`] counts one.
fn variance(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    entry_stored: &BTreeSet<VarId>,
) -> (BTreeSet<VarId>, HashMap<usize, Vec<VarId>>, Addresses) {
    let facts = Facts::of(function);
    let mut regions = Vec::new();
    collect_regions(&function.blocks, None, &mut regions, &mut 0);
    let mut census: HashMap<VarId, Census> = HashMap::new();
    for (list, region) in regions.iter().enumerate() {
        take_census(list, region.blocks, &facts.addresses, &mut census);
    }
    let mut candidates: Vec<(VarId, &Census)> = census
        .iter()
        .filter(|&(&var, entry)| facts.candidate(function, declarations, entry_stored, var, entry))
        .map(|(&var, entry)| (var, entry))
        .collect();
    candidates.sort_unstable_by_key(|&(var, _)| var);
    let mut slots = BTreeSet::new();
    let mut variants: HashMap<usize, Vec<VarId>> = HashMap::new();
    for (var, entry) in candidates {
        // A slot nothing stores or declares is a compile-time parameter's
        // binding, which the elaborator substitutes by name: upstream reads
        // one as a `kgen.param.constant` and allocates nothing.
        let stored_at_entry = entry_stored.contains(&var);
        if !stored_at_entry
            && !entry
                .mentions
                .iter()
                .any(|(_, mention)| matches!(mention, Mention::Store(_) | Mention::Declare))
        {
            continue;
        }
        let allocation = allocation_region(
            &regions,
            entry
                .mentions
                .iter()
                .map(|&(list, _)| list)
                .chain(stored_at_entry.then_some(0)),
        );
        let mut crossed = BTreeSet::new();
        for &(list, mention) in &entry.mentions {
            if !matches!(mention, Mention::Store(_) | Mention::Declare) {
                continue;
            }
            let mut at = list;
            while at != allocation {
                let Some((holder, op)) = regions[at].parent else {
                    break;
                };
                crossed.insert(op);
                at = holder;
            }
        }
        slots.insert(var);
        for op in crossed {
            variants.entry(op).or_default().push(var);
        }
    }
    (slots, variants, facts.addresses)
}

/// One region of a structured body, in preorder.
struct RegionInfo<'a> {
    blocks: &'a [MirBlock],
    /// The region holding the op that holds this one, and that op's
    /// preorder number.
    parent: Option<(usize, usize)>,
}

/// Every region of `blocks` in preorder, `blocks` first, numbered as
/// [`collect_lists`] numbers them; `ops` counts the region ops numbered so
/// far, in preorder.
fn collect_regions<'a>(
    blocks: &'a [MirBlock],
    parent: Option<(usize, usize)>,
    regions: &mut Vec<RegionInfo<'a>>,
    ops: &mut usize,
) {
    let list = regions.len();
    regions.push(RegionInfo { blocks, parent });
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        let held = instruction_regions(instruction);
        if held.is_empty() {
            continue;
        }
        let op = *ops;
        *ops += 1;
        for region in held {
            collect_regions(region, Some((list, op)), regions, ops);
        }
    }
}

/// The innermost region holding each of `lists`: a slot's allocation
/// region.
fn allocation_region(regions: &[RegionInfo<'_>], mut lists: impl Iterator<Item = usize>) -> usize {
    let Some(first) = lists.next() else {
        return 0;
    };
    let mut chain = vec![first];
    while let Some((holder, _)) = regions[chain[chain.len() - 1]].parent {
        chain.push(holder);
    }
    for list in lists {
        let mut at = list;
        loop {
            if let Some(found) = chain.iter().position(|&region| region == at) {
                chain.drain(..found);
                break;
            }
            at = regions[at].parent.map_or(0, |(holder, _)| holder);
        }
    }
    chain[0]
}

/// One region the [`Promoter`] is walking: the frame a terminator leaving
/// it by name resolves to, and its op's variant slots.
struct Frame {
    kind: Option<RegionFrame>,
    variant: Rc<[VarId]>,
}

/// Upstream's `processRegion` over a structured body: one scoped walk that
/// replaces each promoted slot's stores, reads, and lifetime markers by the
/// slot's current value, carrying each value variant across a region op
/// through the op's region arguments, init operands, and results.
struct Promoter<'a> {
    var_tys: &'a HashMap<VarId, Ty>,
    slots: &'a BTreeSet<VarId>,
    addresses: &'a Addresses,
    /// The variant slots of each region op not yet walked, by preorder
    /// number.
    variants: HashMap<usize, Vec<VarId>>,
    /// Each open region's bindings, innermost last; `None` is undefined.
    scopes: Vec<HashMap<VarId, Option<Reg>>>,
    frames: Vec<Frame>,
    next_op: usize,
    n_regs: u32,
    /// The type of each register the walk defines.
    fresh: Vec<(u32, Ty)>,
    erased_results: HashSet<u32>,
    erased: usize,
}

impl Promoter<'_> {
    /// Walk the region `blocks`, entered with `args` bound to its variant
    /// slots `variant`.
    fn walk_region(
        &mut self,
        blocks: &mut [MirBlock],
        kind: Option<RegionFrame>,
        variant: &Rc<[VarId]>,
        args: &[Reg],
    ) {
        let scope = variant
            .iter()
            .zip(args)
            .map(|(&var, &arg)| (var, Some(arg)))
            .collect();
        self.scopes.push(scope);
        self.frames.push(Frame {
            kind,
            variant: Rc::clone(variant),
        });
        for block in blocks {
            self.walk_block(block);
        }
        self.frames.pop();
        self.scopes.pop();
    }

    fn walk_block(&mut self, block: &mut MirBlock) {
        let mut out = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            if let Some((var, mention)) = slot_use(&instruction, self.addresses)
                && self.slots.contains(&var)
            {
                match mention {
                    Mention::Store(src) => self.bind(var, Some(src)),
                    Mention::Declare => self.bind(var, None),
                    Mention::Read { dest, .. } => {
                        let value = self.current_or_undef(var, &mut out);
                        out.push(MirInstr::CopyValue {
                            dest,
                            value,
                            constructor: None,
                        });
                        continue;
                    }
                    // A lifetime marker leaves the value, as upstream's
                    // `StackAllocLifetimeEndOp` does, and so does naming
                    // the address.
                    Mention::Kill | Mention::Address => {}
                }
                let mut results = Vec::new();
                instruction_result_regs(&instruction, &mut results);
                self.erased_results.extend(results.iter().map(|reg| reg.0));
                self.erased += 1;
                continue;
            }
            if !instruction_regions(&instruction).is_empty() {
                self.walk_op(&mut instruction, &mut out);
            }
            out.push(instruction);
        }
        self.carry(&mut block.term, &mut out);
        block.instrs = out;
    }

    /// Walk the regions of the region op `instruction`, giving each region
    /// a terminator enters one argument per variant slot, and the op one
    /// result per variant slot, which holds the slot after it.
    fn walk_op(&mut self, instruction: &mut MirInstr, out: &mut Vec<MirInstr>) {
        let op = self.next_op;
        self.next_op += 1;
        let variant: Rc<[VarId]> = self.variants.remove(&op).unwrap_or_default().into();
        let results = match instruction {
            MirInstr::If {
                results,
                then,
                elifs,
                else_args,
                orelse,
                ..
            } => {
                self.walk_if(&variant, then, elifs, else_args, orelse);
                results
            }
            MirInstr::Loop {
                label,
                operands,
                args,
                results,
                body,
            } => {
                let continued = !variant.is_empty() && {
                    let mut found = false;
                    loop_continues_mut(body, *label, &mut |_, _| found = true);
                    found
                };
                let args = self.entry_arguments(&variant, continued, operands, args, out);
                let frame = Some(RegionFrame::Loop(*label));
                self.walk_region(body, frame, &variant, &args);
                results
            }
            MirInstr::ComptimeIf {
                results,
                arms,
                orelse,
                ..
            } => {
                for arm in arms.iter_mut().chain(std::iter::once(orelse)) {
                    self.walk_region(arm, None, &variant, &[]);
                }
                results
            }
            // A `comptime for`'s operands and results agree in number, so
            // its body takes the variant slots even when no continue
            // resumes it.
            MirInstr::ComptimeFor {
                operands,
                args,
                results,
                body,
                orelse,
                ..
            } => {
                let carried = !variant.is_empty();
                let args = self.entry_arguments(&variant, carried, operands, args, out);
                self.walk_region(body, Some(RegionFrame::ComptimeFor), &variant, &args);
                self.walk_region(orelse, None, &variant, &[]);
                results
            }
            MirInstr::Match {
                results,
                cases,
                else_args,
                orelse,
            } => {
                self.walk_match(&variant, cases, else_args, orelse);
                results
            }
            MirInstr::Try {
                label,
                results,
                body,
                handler_args,
                handler,
                else_args,
                orelse,
                ..
            } => {
                let carried = !variant.is_empty();
                let raised = carried && {
                    let mut found = false;
                    try_raises_mut(body, *label, &mut |_| found = true);
                    found
                };
                let completes = carried
                    && matches!(&body[..], [.., last] if matches!(last.term, MirTerm::TryYield(_)));
                self.walk_region(body, Some(RegionFrame::Try(*label)), &variant, &[]);
                if let Some(blocks) = handler {
                    let args = self.arguments(&variant, raised, handler_args);
                    self.walk_region(blocks, None, &variant, &args);
                }
                if let Some(blocks) = orelse {
                    let args = self.arguments(&variant, completes, else_args);
                    self.walk_region(blocks, None, &variant, &args);
                }
                results
            }
            _ => {
                for region in instruction_regions_mut(instruction) {
                    self.walk_region(region, None, &variant, &[]);
                }
                return;
            }
        };
        for &var in variant.iter() {
            let result = self.fresh_register(var);
            results.push(result);
            self.bind(var, Some(result));
        }
    }

    /// An `if`'s regions: each elif's arm, each elif condition after the
    /// first, and, after elifs, the else region are entered by an
    /// `if.elifcond.yield`, so they take the variant slots.
    fn walk_if(
        &mut self,
        variant: &Rc<[VarId]>,
        then: &mut [MirBlock],
        elifs: &mut [MirElif],
        else_args: &mut Vec<Reg>,
        orelse: &mut [MirBlock],
    ) {
        let carried = !variant.is_empty();
        self.walk_region(then, None, variant, &[]);
        for (index, elif) in elifs.iter_mut().enumerate() {
            let args = self.arguments(variant, carried && index > 0, &mut elif.cond_args);
            self.walk_region(&mut elif.cond, None, variant, &args);
            let args = self.arguments(variant, carried, &mut elif.then_args);
            self.walk_region(&mut elif.then, None, variant, &args);
        }
        let args = self.arguments(variant, carried && !elifs.is_empty(), else_args);
        self.walk_region(orelse, None, variant, &args);
    }

    /// A `match`'s regions: each case after one a `match.next` leaves, and
    /// the else region after the last, take the variant slots.
    fn walk_match(
        &mut self,
        variant: &Rc<[VarId]>,
        cases: &mut [MirCase],
        else_args: &mut Vec<Reg>,
        orelse: &mut [MirBlock],
    ) {
        let carried = !variant.is_empty();
        let mut entered = false;
        for case in cases {
            let args = self.arguments(variant, carried && entered, &mut case.args);
            entered = false;
            match_nexts_mut(&mut case.body, &mut |_, _| entered = true);
            let frame = Some(RegionFrame::MatchCase);
            self.walk_region(&mut case.body, frame, variant, &args);
        }
        let args = self.arguments(variant, carried && entered, else_args);
        self.walk_region(orelse, None, variant, &args);
    }

    /// A loop body's or `comptime for` body's arguments when `carried`:
    /// the variant slots' current values become the op's init operands,
    /// materialized before it in `out`.
    fn entry_arguments(
        &mut self,
        variant: &[VarId],
        carried: bool,
        operands: &mut Vec<Reg>,
        args: &mut Vec<Reg>,
        out: &mut Vec<MirInstr>,
    ) -> Vec<Reg> {
        if carried {
            for &var in variant {
                let value = self.current_or_undef(var, out);
                operands.push(value);
            }
        }
        self.arguments(variant, carried, args)
    }

    /// One fresh argument per variant slot when `carried`, appended to the
    /// region's `args`.
    fn arguments(&mut self, variant: &[VarId], carried: bool, args: &mut Vec<Reg>) -> Vec<Reg> {
        if !carried {
            return Vec::new();
        }
        let added: Vec<Reg> = variant
            .iter()
            .map(|&var| self.fresh_register(var))
            .collect();
        args.extend(&added);
        added
    }

    /// Append the current value of each variant slot of the op a region
    /// terminator enters or leaves to its values (`insertVariants`): a
    /// `try.raise` enters the handler of the `try` it names.
    fn carry(&mut self, term: &mut MirTerm, out: &mut Vec<MirInstr>) {
        let target = match term {
            MirTerm::TryRaise { label, .. } => Some(RegionFrame::Try(*label)),
            _ => region_exit(term).map(|(target, _)| target),
        };
        let frame = match target {
            Some(target) => self
                .frames
                .iter()
                .rev()
                .find(|frame| frame.kind.is_some_and(|kind| target.names(kind))),
            None if matches!(
                term,
                MirTerm::Yield(_)
                    | MirTerm::ElifCond { .. }
                    | MirTerm::ComptimeYield(_)
                    | MirTerm::TryYield(_)
            ) =>
            {
                self.frames.last()
            }
            None => None,
        };
        let Some(variant) = frame.map(|frame| Rc::clone(&frame.variant)) else {
            return;
        };
        let carried: Vec<Reg> = variant
            .iter()
            .map(|&var| self.current_or_undef(var, out))
            .collect();
        if let Some(values) = edge_values_mut(term) {
            values.extend(carried);
        }
    }

    /// Bind `var` to `value` in the innermost scope (upstream's `forWrite`).
    fn bind(&mut self, var: VarId, value: Option<Reg>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(var, value);
        }
    }

    /// The current value of `var`, or an `uninitmem` constant pushed onto
    /// `out` and bound in the innermost scope when it holds nothing
    /// (`currValueOrUndef`).
    fn current_or_undef(&mut self, var: VarId, out: &mut Vec<MirInstr>) -> Reg {
        if let Some(value) = self
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&var))
            .copied()
            .flatten()
        {
            return value;
        }
        let dest = self.fresh_register(var);
        out.push(MirInstr::Const {
            dest,
            k: Const::UninitMem,
        });
        self.bind(var, Some(dest));
        dest
    }

    /// A fresh register of `var`'s type.
    fn fresh_register(&mut self, var: VarId) -> Reg {
        let reg = Reg(self.n_regs);
        self.n_regs += 1;
        if let Some(ty) = self.var_tys.get(&var) {
            self.fresh.push((reg.0, ty.clone()));
        }
        reg
    }
}

/// The values a region terminator carries along its edge.
const fn edge_values_mut(term: &mut MirTerm) -> Option<&mut Vec<Reg>> {
    match term {
        MirTerm::Yield(values)
        | MirTerm::TryYield(values)
        | MirTerm::TryRaise { values, .. }
        | MirTerm::ComptimeYield(values)
        | MirTerm::MatchNext(values)
        | MirTerm::MatchComplete(values)
        | MirTerm::ComptimeBreak(values)
        | MirTerm::ComptimeContinue(values)
        | MirTerm::ElifCond { values, .. }
        | MirTerm::Break { values, .. }
        | MirTerm::Continue { values, .. } => Some(values),
        _ => None,
    }
}

/// The registers `blocks`, its regions included, defines by a
/// `closure.make` of reference captures: a struct of pointers, trivially
/// register-passable as upstream's closure of borrows is.
fn reference_closures(blocks: &[MirBlock], closures: &mut HashSet<u32>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        for region in instruction_regions(instruction) {
            reference_closures(region, closures);
        }
        if let MirInstr::MakeClosure { dest, .. } = instruction
            && instruction.would_be_trivially_dead()
        {
            closures.insert(dest.0);
        }
    }
}

/// Record every slot mention of the region `list`'s own blocks (not of the
/// regions they hold) in `census`, a `ref.read` or `ref.write` through one
/// of the `addresses` mentioning its slot.
fn take_census(
    list: usize,
    blocks: &[MirBlock],
    addresses: &Addresses,
    census: &mut HashMap<VarId, Census>,
) {
    for block in blocks {
        for instruction in &block.instrs {
            let slots = instruction_slots(instruction);
            match slot_use(instruction, addresses) {
                Some((var, mention)) if slots.iter().all(|&slot| slot == var) => {
                    census
                        .entry(var)
                        .or_default()
                        .mentions
                        .push((list, mention));
                }
                _ => keep(census, slots),
            }
        }
    }
}

/// Mark each of `slots` kept.
fn keep(census: &mut HashMap<VarId, Census>, slots: impl IntoIterator<Item = VarId>) {
    for var in slots {
        census.entry(var).or_default().kept = true;
    }
}

/// The slot `instruction` mentions as a promotable use, and how; a
/// `ref.read` or `ref.write` through one of the `addresses` mentions the
/// slot it addresses.
fn slot_use(instruction: &MirInstr, addresses: &Addresses) -> Option<(VarId, Mention)> {
    match instruction {
        MirInstr::DefVar { var, src, .. } => Some((*var, Mention::Store(*src))),
        MirInstr::UseVar {
            dest, var, mode, ..
        } => match mode {
            UseMode::Copy | UseMode::Move | UseMode::Forward => {
                Some((*var, Mention::Read { dest: *dest }))
            }
            UseMode::BorrowShared | UseMode::BorrowMut => None,
        },
        MirInstr::LoadPlace { dest, place } if place.proj.is_empty() && place.through.is_none() => {
            Some((place.root, Mention::Read { dest: *dest }))
        }
        MirInstr::MakeRef { dest, place } if addresses.contains_key(&dest.0) => {
            Some((place.root, Mention::Address))
        }
        MirInstr::ReadRef { dest, reference } => addresses
            .get(&reference.0)
            .map(|&var| (var, Mention::Read { dest: *dest })),
        MirInstr::WriteRef { reference, value } => addresses
            .get(&reference.0)
            .map(|&var| (var, Mention::Store(*value))),
        MirInstr::DeclareVar { var } => Some((*var, Mention::Declare)),
        MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } => Some((*var, Mention::Kill)),
        _ => None,
    }
}

/// Record in `made` each whole-root `ref.make` of `blocks`, its regions
/// included, by its dest, and count in `through` each register's reads as
/// the reference of a `ref.read` or `ref.write`.
fn slot_references(blocks: &[MirBlock], made: &mut Addresses, through: &mut [u32]) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        for region in instruction_regions(instruction) {
            slot_references(region, made, through);
        }
        match instruction {
            MirInstr::MakeRef { dest, place }
                if place.proj.is_empty() && place.through.is_none() =>
            {
                made.insert(dest.0, place.root);
            }
            MirInstr::ReadRef { reference, .. } | MirInstr::WriteRef { reference, .. } => {
                if let Some(count) = through.get_mut(reference.0 as usize) {
                    *count += 1;
                }
            }
            _ => {}
        }
    }
}
