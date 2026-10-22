//! `mem-2-reg`: upstream's `Mem2Reg`, which promotes a stack allocation to
//! SSA values when every user is a load, a store, or a lifetime marker, and
//! `lower-arg-conventions`, the callee half of upstream's
//! `LowerArgConventions`, which stores each register-passable argument into
//! a temporary that `Mem2Reg` then promotes.
//!
//! A MIR variable slot is the allocation. One is promoted when its type is
//! trivially register-passable at a concrete size
//! ([`MirDeclarations::copy_is_identity`], never symbolic), and every
//! mention of it is a store (`var.store`), a read (`var.use`, or a
//! whole-root `place.load`), or a mention with no effect on such a type
//! once drops are elaborated (`var.declare`, `drop.var`, `var.consume`, and
//! a whole-root `ownership.mark_initialized`, which upstream's `LowerLIT`
//! erases before `Mem2Reg` runs).
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
//! its else region), init operands for a body the op enters too, and one
//! result per variant slot, which the region terminators define. Where the
//! slot holds nothing on an edge, the edge carries an `uninitmem` constant,
//! upstream's `currValueOrUndef`. A `try` carries no values, so a slot
//! stored inside a `try` part and allocated outside it keeps its slot,
//! where upstream carries it through `lit.try`'s results and except
//! arguments. A slot nothing stores or declares is a compile-time
//! parameter's binding, which the elaborator substitutes, and keeps its
//! slot.
//!
//! A lowered body, which the post-elaboration pipeline still runs this on,
//! is a CFG whose blocks take no arguments: a read is promoted when its one
//! reaching store lies on every path to it and the stored register's one
//! definition lies before the store in its block, and every mention of the
//! slot sits in one block list, so a slot whose reads merge two stores
//! keeps its slot. A list holding a lowered `comptime for` promotes
//! nothing: upstream's `Mem2Reg` meets every `comptime for` as its op, and
//! the elaborator unrolls none of a lowered body's.

use super::canonicalize::definition_counts;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::verify::is_structured_shape;
use mojito_mir::mir::{
    Const, MirBlock, MirBlockId, MirCase, MirDeclarations, MirElif, MirFunction, MirInstr, MirTerm,
    Reg, RegionFrame, UseMode, block_successors, cleanup_slots, count_register_reads,
    has_structured_control_flow, instruction_regions, instruction_regions_mut, instruction_slots,
    loop_continues_mut, match_nexts_mut, region_escape_targets, region_exit, renumber_block_slots,
    retire_slots, terminator_targets,
};
use mojito_types::types::{Ty, is_symbolic};
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

/// Promote each local slot of `function` that upstream's `Mem2Reg` would
/// promote to registers, and remove it from the function; returns whether
/// any was.
pub fn mem_2_reg(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    let (slots, erased_results, erased) = if is_structured_shape(function) {
        promote_in_regions(function, declarations)
    } else {
        let promotion = promotable(function, declarations, &BTreeSet::new());
        let mut erased_results = HashSet::new();
        let erased = if promotion.slots.is_empty() {
            0
        } else {
            rewrite_list(
                &mut function.blocks,
                &mut 0,
                &promotion,
                &mut erased_results,
            )
        };
        (promotion.slots, erased_results, erased)
    };
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

/// Store each argument of `function` that [`mem_2_reg`] would promote into
/// a fresh local at the entry, which every mention of the argument then
/// names; returns whether any was.
///
/// The argument is a `read` or owned parameter of a trivially
/// register-passable type, as upstream promotes one; a `mut` or `deinit`
/// one is never stored. Its own slot, the call ABI's, keeps the one read at
/// the entry. A body whose entry is a jump's target is left alone: the
/// store would run again there.
pub fn lower_arg_conventions(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    if has_structured_control_flow(function)
        || function
            .blocks
            .iter()
            .any(|block| block_successors(block).contains(&0))
    {
        return false;
    }
    let eligible: BTreeSet<VarId> = (0..function.n_params)
        .filter(|&param| {
            !function.ref_params.get(param).copied().unwrap_or(true)
                && !function.deinit_params.get(param).copied().unwrap_or(true)
                && function
                    .param_types
                    .get(param)
                    .is_some_and(|ty| promotable_type(ty, declarations))
        })
        .filter_map(|param| VarId::try_from(param).ok())
        .collect();
    if eligible.is_empty() {
        return false;
    }
    let stored: Vec<VarId> = promotable(function, declarations, &eligible)
        .slots
        .intersection(&eligible)
        .copied()
        .collect();
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
        if let Some((span, _)) = function
            .spans
            .0
            .values()
            .find(|(_, origin)| *origin == Some(param))
            .cloned()
        {
            function.spans.0.insert(value.0, (span, Some(param)));
        }
        entry.push(MirInstr::UseVar {
            dest: value,
            var: param,
            mode: UseMode::Copy,
            constructor: None,
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

/// A store, read, or kill site: the block list (numbered in preorder from
/// the function's own, 0), the block, and the instruction's index.
type Site = (usize, MirBlockId, usize);

/// The slots [`mem_2_reg`] promotes, and the register each of their reads
/// becomes a copy of.
struct Promotion {
    slots: BTreeSet<VarId>,
    reads: HashMap<Site, Reg>,
}

/// One mention of a candidate slot.
#[derive(Clone, Copy)]
enum Mention {
    /// A store of the register.
    Store(Reg),
    /// A read into the register; `ends` when it ends the slot.
    Read { dest: Reg, ends: bool },
    /// A declaration: the slot holds nothing after it.
    Declare,
    /// A drop or consumption: a lifetime marker, after which the slot holds
    /// nothing.
    Kill,
    /// A whole-slot `ownership.mark_initialized`, which upstream's
    /// `LowerLIT` has erased before `Mem2Reg` runs: it binds nothing.
    Mark,
}

/// A candidate's reaching store at a point of its list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reaching {
    /// No path reaches the point yet: the join's identity.
    Unreached,
    /// Some path reaches it with the slot holding nothing.
    Uninit,
    /// Every path reaches it past this store last.
    Store(usize),
    /// Paths reach it past different stores.
    Conflict,
}

impl Reaching {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unreached, other) | (other, Self::Unreached) => other,
            (a, b) if a == b => a,
            _ => Self::Conflict,
        }
    }
}

/// The census of one slot: the list holding its mentions, and whether some
/// mention keeps it.
#[derive(Default)]
struct Census {
    list: Option<usize>,
    /// The `try` part (or, in a lowered body, the block list) holding the
    /// mentions.
    context: Option<usize>,
    kept: bool,
    mentions: Vec<(Site, Mention)>,
}

/// The slots of `function` [`mem_2_reg`] would promote, with each read's
/// stored register. Each of the `entry_stored` parameters counts as stored
/// at the entry, by no register, and is a candidate as a local is.
fn promotable(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    entry_stored: &BTreeSet<VarId>,
) -> Promotion {
    let facts = Facts::of(function);
    let mut lists = Vec::new();
    collect_lists(&function.blocks, true, &mut lists);
    let mut census: HashMap<VarId, Census> = HashMap::new();
    for (list, (blocks, _)) in lists.iter().enumerate() {
        take_census((list, list), blocks, true, &mut census);
    }
    let mut candidates: Vec<Vec<VarId>> = vec![Vec::new(); lists.len()];
    for (&var, entry) in &census {
        if let Some(list) = entry.list
            && facts.candidate(function, declarations, entry_stored, var, entry)
        {
            candidates[list].push(var);
        }
    }
    let mut promotion = Promotion {
        slots: BTreeSet::new(),
        reads: HashMap::new(),
    };
    for (list, slots) in candidates.iter_mut().enumerate() {
        if slots.is_empty() {
            continue;
        }
        slots.sort_unstable();
        let (blocks, function_level) = lists[list];
        let entry: Vec<Reaching> = slots
            .iter()
            .map(|var| {
                if list == 0 && entry_stored.contains(var) {
                    Reaching::Store(usize::MAX)
                } else {
                    Reaching::Uninit
                }
            })
            .collect();
        solve(
            &ListFlow {
                list,
                blocks,
                function_level,
                slots,
                census: &census,
            },
            entry,
            &mut promotion,
        );
    }
    promotion
}

/// The register facts a candidate's mentions are judged by.
struct Facts {
    definitions: Vec<u32>,
    reads: Vec<u32>,
    /// The registers defined by a `closure.make` of reference captures.
    closures: HashSet<u32>,
}

impl Facts {
    fn of(function: &mut MirFunction) -> Self {
        let definitions = definition_counts(&mut function.blocks, function.n_regs);
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let mut closures = HashSet::new();
        reference_closures(&function.blocks, &mut closures);
        Self {
            definitions,
            reads,
            closures,
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
            Mention::Read { .. } | Mention::Declare | Mention::Kill | Mention::Mark => true,
        });
        (var as usize >= function.n_params || entry_stored.contains(&var))
            && !entry.kept
            && typed.is_some_and(|ty| promotable_type(ty, declarations) || stores_closures)
            && entry.mentions.iter().all(|(_, mention)| match *mention {
                Mention::Store(src) => {
                    function.reg_types.get(&src.0) == typed
                        && self.definitions.get(src.0 as usize) == Some(&1)
                        && self.reads.get(src.0 as usize) == Some(&1)
                }
                Mention::Read { dest, .. } => function.reg_types.get(&dest.0) == typed,
                Mention::Declare | Mention::Kill | Mention::Mark => true,
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
    let (slots, variants) = variance(function, declarations);
    if slots.is_empty() {
        return (slots, HashSet::new(), 0);
    }
    let mut blocks = std::mem::take(&mut function.blocks);
    let mut promoter = Promoter {
        var_tys: &function.var_tys,
        slots: &slots,
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

/// The slots of the structured body `function` [`mem_2_reg`] promotes, and
/// for each region op, by its preorder number, the promoted slots variant
/// across it, each list sorted.
///
/// A slot is allocated in the innermost region holding every mention of it,
/// where its declaration sits, as upstream's `pop.stack_allocation` does.
/// Its stores and declarations make it variant across each op between them
/// and that region (`userCrossesFunctionCFG`'s `enclosingNodes`); a store in
/// a `try` part of a slot allocated outside it keeps the slot, since a `try`
/// carries no values.
fn variance(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> (BTreeSet<VarId>, HashMap<usize, Vec<VarId>>) {
    let facts = Facts::of(function);
    let mut regions = Vec::new();
    let mut tries = Vec::new();
    collect_regions(&function.blocks, None, &mut regions, &mut tries);
    let mut census: HashMap<VarId, Census> = HashMap::new();
    for (list, region) in regions.iter().enumerate() {
        take_census((list, 0), region.blocks, false, &mut census);
    }
    let mut candidates: Vec<(VarId, &Census)> = census
        .iter()
        .filter(|&(&var, entry)| {
            facts.candidate(function, declarations, &BTreeSet::new(), var, entry)
        })
        .map(|(&var, entry)| (var, entry))
        .collect();
    candidates.sort_unstable_by_key(|&(var, _)| var);
    let mut slots = BTreeSet::new();
    let mut variants: HashMap<usize, Vec<VarId>> = HashMap::new();
    for (var, entry) in candidates {
        // A slot nothing stores or declares is a compile-time parameter's
        // binding, which the elaborator substitutes by name: upstream reads
        // one as a `kgen.param.constant` and allocates nothing.
        if !entry
            .mentions
            .iter()
            .any(|(_, mention)| matches!(mention, Mention::Store(_) | Mention::Declare))
        {
            continue;
        }
        let allocation = allocation_region(
            &regions,
            entry.mentions.iter().map(|&((list, _, _), _)| list),
        );
        let mut crossed = BTreeSet::new();
        for &((list, _, _), mention) in &entry.mentions {
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
        if crossed.iter().any(|&op| tries[op]) {
            continue;
        }
        slots.insert(var);
        for op in crossed {
            variants.entry(op).or_default().push(var);
        }
    }
    (slots, variants)
}

/// One region of a structured body, in preorder.
struct RegionInfo<'a> {
    blocks: &'a [MirBlock],
    /// The region holding the op that holds this one, and that op's
    /// preorder number.
    parent: Option<(usize, usize)>,
}

/// Every region of `blocks` in preorder, `blocks` first, numbered as
/// [`collect_lists`] numbers them; `tries` records, for each region op in
/// preorder, whether it is a `try`.
fn collect_regions<'a>(
    blocks: &'a [MirBlock],
    parent: Option<(usize, usize)>,
    regions: &mut Vec<RegionInfo<'a>>,
    tries: &mut Vec<bool>,
) {
    let list = regions.len();
    regions.push(RegionInfo { blocks, parent });
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        let held = instruction_regions(instruction);
        if held.is_empty() {
            continue;
        }
        let op = tries.len();
        tries.push(matches!(instruction, MirInstr::Try { .. }));
        for region in held {
            collect_regions(region, Some((list, op)), regions, tries);
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
            if let Some((var, mention)) = slot_use(&instruction)
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
                    // `StackAllocLifetimeEndOp` does.
                    Mention::Kill | Mention::Mark => {}
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
    /// terminator enters or leaves to its values (`insertVariants`).
    fn carry(&mut self, term: &mut MirTerm, out: &mut Vec<MirInstr>) {
        let frame = match region_exit(term) {
            Some((target, _)) => self
                .frames
                .iter()
                .rev()
                .find(|frame| frame.kind.is_some_and(|kind| target.names(kind))),
            None if matches!(
                term,
                MirTerm::Yield(_) | MirTerm::ElifCond { .. } | MirTerm::ComptimeYield(_)
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

/// Whether a slot of `ty` may be promoted: a trivially register-passable
/// value of a size the declaration fixes.
fn promotable_type(ty: &Ty, declarations: &MirDeclarations) -> bool {
    !is_symbolic(ty) && declarations.copy_is_identity(ty)
}

/// Every block list of `blocks` in preorder, `blocks` first, each with
/// whether it is the function's own.
fn collect_lists<'a>(
    blocks: &'a [MirBlock],
    function_level: bool,
    lists: &mut Vec<(&'a [MirBlock], bool)>,
) {
    lists.push((blocks, function_level));
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        for region in instruction_regions(instruction) {
            collect_lists(region, false, lists);
        }
    }
}

/// Record every slot mention of the list `list`'s own blocks (not of the
/// regions they hold), in the `try` part `context`, in `census`. A slot
/// mentioned in two contexts is kept. With `defined_before`, a store's
/// register must be defined before it in its block.
fn take_census(
    (list, context): (usize, usize),
    blocks: &[MirBlock],
    defined_before: bool,
    census: &mut HashMap<VarId, Census>,
) {
    for (at_block, block) in blocks.iter().enumerate() {
        let mut defined = HashSet::new();
        for (at, instruction) in block.instrs.iter().enumerate() {
            let site = (list, at_block, at);
            let slots = instruction_slots(instruction);
            // A stored register must be defined before the store in its block.
            let used = slot_use(instruction).filter(|(_, mention)| match mention {
                Mention::Store(src) => !defined_before || defined.contains(&src.0),
                _ => true,
            });
            match (used, slots.as_slice()) {
                (Some((var, mention)), [only]) if *only == var => {
                    let entry = census.entry(var).or_default();
                    entry.kept |= entry.context.is_some_and(|other| other != context);
                    entry.context = Some(context);
                    entry.list = Some(list);
                    entry.mentions.push((site, mention));
                }
                _ => keep(census, slots),
            }
            if let MirInstr::Try {
                handler, cleanup, ..
            } = instruction
            {
                keep(census, handler.iter().filter_map(|(slot, _)| *slot));
                keep(census, cleanup_slots(cleanup));
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            defined.extend(results.iter().map(|reg| reg.0));
        }
        if let MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } =
            &block.term
        {
            keep(census, cleanup_slots(cleanup));
        }
    }
}

/// Mark each of `slots` kept.
fn keep(census: &mut HashMap<VarId, Census>, slots: impl IntoIterator<Item = VarId>) {
    for var in slots {
        census.entry(var).or_default().kept = true;
    }
}

/// The slot `instruction` mentions as a promotable use, and how.
const fn slot_use(instruction: &MirInstr) -> Option<(VarId, Mention)> {
    match instruction {
        MirInstr::DefVar { var, src, .. } => Some((*var, Mention::Store(*src))),
        MirInstr::UseVar {
            dest, var, mode, ..
        } => match mode {
            UseMode::Copy => Some((
                *var,
                Mention::Read {
                    dest: *dest,
                    ends: false,
                },
            )),
            UseMode::Move | UseMode::Forward => Some((
                *var,
                Mention::Read {
                    dest: *dest,
                    ends: true,
                },
            )),
            UseMode::BorrowShared | UseMode::BorrowMut => None,
        },
        MirInstr::LoadPlace { dest, place } if place.proj.is_empty() && place.through.is_none() => {
            Some((
                place.root,
                Mention::Read {
                    dest: *dest,
                    ends: false,
                },
            ))
        }
        MirInstr::DeclareVar { var } => Some((*var, Mention::Declare)),
        MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } => Some((*var, Mention::Kill)),
        MirInstr::MarkInitialized { place } if place.proj.is_empty() && place.through.is_none() => {
            Some((place.root, Mention::Mark))
        }
        _ => None,
    }
}

/// One block list's candidates, as [`solve`] reads them.
struct ListFlow<'a> {
    list: usize,
    blocks: &'a [MirBlock],
    function_level: bool,
    slots: &'a [VarId],
    census: &'a HashMap<VarId, Census>,
}

/// One event of a block for the reaching-store flow, at its instruction
/// index.
enum Event {
    Store {
        slot: usize,
        store: usize,
    },
    Kill {
        slot: usize,
    },
    Read {
        slot: usize,
        at: usize,
    },
    /// A region op whose regions escape to these function-level blocks.
    Escape(Vec<MirBlockId>),
}

/// Solve the reaching stores of `flow`'s candidates from the `entry`
/// states, and add each candidate whose every read sees one store to
/// `promotion`; none when the list holds a lowered `comptime for`.
fn solve(flow: &ListFlow<'_>, entry: Vec<Reaching>, promotion: &mut Promotion) {
    let &ListFlow {
        list,
        blocks,
        function_level,
        slots,
        census,
    } = flow;
    if blocks
        .iter()
        .any(|block| matches!(block.term, MirTerm::ComptimeFor { .. }))
    {
        return;
    }
    // Store sites, by number: the block and the stored register.
    let mut stores: Vec<(MirBlockId, Reg)> = Vec::new();
    let mut events: Vec<Vec<(usize, Event)>> = (0..blocks.len()).map(|_| Vec::new()).collect();
    for (slot, var) in slots.iter().enumerate() {
        for &((_, block, at), mention) in &census[var].mentions {
            let event = match mention {
                Mention::Store(src) => {
                    stores.push((block, src));
                    Event::Store {
                        slot,
                        store: stores.len() - 1,
                    }
                }
                Mention::Read { ends, .. } => {
                    events[block].push((at, Event::Read { slot, at }));
                    if !ends {
                        continue;
                    }
                    Event::Kill { slot }
                }
                Mention::Declare | Mention::Kill => Event::Kill { slot },
                Mention::Mark => continue,
            };
            events[block].push((at, event));
        }
    }
    if function_level {
        for (at_block, block) in blocks.iter().enumerate() {
            for (at, instruction) in block.instrs.iter().enumerate() {
                let escapes = region_escape_targets(instruction);
                if !escapes.is_empty() {
                    events[at_block].push((at, Event::Escape(escapes)));
                }
            }
        }
    }
    // A read and the kill that ends it share an index: the read comes first.
    for block in &mut events {
        block.sort_by_key(|(at, event)| (*at, !matches!(event, Event::Read { .. })));
    }
    let mut states = vec![vec![Reaching::Unreached; slots.len()]; blocks.len()];
    if blocks.is_empty() {
        return;
    }
    states[0] = entry;
    let mut pending = vec![0];
    let flow_into = |target: MirBlockId,
                     state: &[Reaching],
                     states: &mut Vec<Vec<Reaching>>,
                     pending: &mut Vec<MirBlockId>| {
        let Some(into) = states.get_mut(target) else {
            return;
        };
        let mut changed = false;
        for (into, &from) in into.iter_mut().zip(state) {
            let joined = into.join(from);
            changed |= joined != *into;
            *into = joined;
        }
        if changed {
            pending.push(target);
        }
    };
    while let Some(block) = pending.pop() {
        let mut state = states[block].clone();
        for (_, event) in &events[block] {
            match event {
                Event::Store { slot, store } => state[*slot] = Reaching::Store(*store),
                Event::Kill { slot } => state[*slot] = Reaching::Uninit,
                Event::Read { .. } => {}
                Event::Escape(targets) => {
                    for &target in targets {
                        flow_into(target, &state, &mut states, &mut pending);
                    }
                }
            }
        }
        for target in terminator_targets(&blocks[block].term) {
            flow_into(target, &state, &mut states, &mut pending);
        }
    }
    // Each read's reaching store, replayed from the solved entry states.
    let mut promoted = vec![true; slots.len()];
    let mut reached: Vec<(usize, Site, usize)> = Vec::new();
    for (at_block, block_events) in events.iter().enumerate() {
        let mut state = states[at_block].clone();
        for (_, event) in block_events {
            match event {
                Event::Store { slot, store } => state[*slot] = Reaching::Store(*store),
                Event::Kill { slot } => state[*slot] = Reaching::Uninit,
                Event::Read { slot, at } => match state[*slot] {
                    Reaching::Store(store) => reached.push((*slot, (list, at_block, *at), store)),
                    _ => promoted[*slot] = false,
                },
                Event::Escape(_) => {}
            }
        }
    }
    for (slot, var) in slots.iter().enumerate() {
        if promoted[slot] {
            promotion.slots.insert(*var);
        }
    }
    for (slot, site, store) in reached {
        if promoted[slot]
            && let Some(&(_, src)) = stores.get(store)
        {
            promotion.reads.insert(site, src);
        }
    }
}

/// Rewrite the block list numbered `*next` and the regions below it: each
/// promoted read becomes a copy of its stored register, and each other
/// mention of a promoted slot goes, its results added to `erased_results`.
/// Returns how many instructions went.
fn rewrite_list(
    blocks: &mut [MirBlock],
    next: &mut usize,
    promotion: &Promotion,
    erased_results: &mut HashSet<u32>,
) -> usize {
    let list = *next;
    *next += 1;
    let mut erased = 0;
    for (at_block, block) in blocks.iter_mut().enumerate() {
        let instructions = std::mem::take(&mut block.instrs);
        for (at, mut instruction) in instructions.into_iter().enumerate() {
            for region in instruction_regions_mut(&mut instruction) {
                erased += rewrite_list(region, next, promotion, erased_results);
            }
            if let Some(&value) = promotion.reads.get(&(list, at_block, at)) {
                let mut results = Vec::new();
                instruction_result_regs(&instruction, &mut results);
                block.instrs.push(MirInstr::CopyValue {
                    dest: results[0],
                    value,
                    constructor: None,
                });
                continue;
            }
            if slot_use(&instruction).is_some_and(|(var, _)| promotion.slots.contains(&var)) {
                let mut results = Vec::new();
                instruction_result_regs(&instruction, &mut results);
                erased_results.extend(results.iter().map(|reg| reg.0));
                erased += 1;
                continue;
            }
            block.instrs.push(instruction);
        }
    }
    erased
}
