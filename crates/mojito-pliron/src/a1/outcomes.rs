//! Outcome normalization, and its checked inverse.
//!
//! Structured `try` becomes a flat control-flow graph with invoke edges,
//! ordered cleanup blocks, one pending-outcome entry per `finally`, and
//! dispatch after it.
//!
//! Both directions rebuild a function from the operations it currently
//! holds. The inverse reads the region layout only to learn which block
//! belongs where, and rejects a graph the layout does not describe.

use std::collections::HashMap;

use pliron::basic_block::BasicBlock;
use pliron::context::{Context, Ptr};
use pliron::linked_list::ContainsLinkedList;
use pliron::location::Located;
use pliron::operation::Operation;
use pliron::region::Region;
use pliron::r#type::{TypeHandle, Typed};
use pliron::value::Value;

use super::attrs::{
    BlockCategory, BlockRecord, CoreLifecycle, CoreRole, CoreStorage, ExitAttr, IdentityAttr,
    LayoutAttr, LifecycleAttr, OutcomeAttr, OutcomeKind, ProvenanceAttr, SlotAttr, StoreAttr, Text,
    TryAttr,
};
use super::import::{annotate, set};
use super::inventory::{CoreOpKind, try_path};
use super::lifecycle::derive_contract;
use super::ops::{
    self, KEY_CONTRACT, KEY_EXIT, KEY_IDENTITY, KEY_LAYOUT, KEY_LIFECYCLE, KEY_OUTCOME, KEY_SLOT,
    KEY_STORE, KEY_TRY,
};
use super::types::{EffectType, ErrorType, OutcomeType};
use super::verify::{attr, describe, invoked_kind, raised_by};
use super::{A1Error, A1ErrorKind};

/// Normalize every function of a bridge-stage module in place.
pub fn normalize(ctx: &mut Context, module: Ptr<Operation>) -> Result<(), A1Error> {
    for func in functions(ctx, module) {
        let rebuilt = Normalizer::run(ctx, func)?;
        replace(ctx, func, rebuilt);
    }
    Ok(())
}

/// Rebuild the structured `try` regions of every function of an
/// executable-core module in place, checking the layout against the graph.
pub fn denormalize(ctx: &mut Context, module: Ptr<Operation>) -> Result<(), A1Error> {
    for func in functions(ctx, module) {
        let plan = FnPlan::build(ctx, func)?;
        let rebuilt = Denormalizer::run(ctx, func, &plan)?;
        replace(ctx, func, rebuilt);
    }
    Ok(())
}

/// The functions of `module`, in order.
pub fn functions(ctx: &Context, module: Ptr<Operation>) -> Vec<Ptr<Operation>> {
    module
        .deref(ctx)
        .regions()
        .flat_map(|region| region.deref(ctx).iter(ctx).collect::<Vec<_>>())
        .flat_map(|block| block.deref(ctx).iter(ctx).collect::<Vec<_>>())
        .collect()
}

/// The operations of `block`, in order.
pub fn block_ops(ctx: &Context, block: Ptr<BasicBlock>) -> Vec<Ptr<Operation>> {
    block.deref(ctx).iter(ctx).collect()
}

/// The blocks of a function's region, in order.
pub fn function_blocks(ctx: &Context, func: Ptr<Operation>) -> Vec<Ptr<BasicBlock>> {
    func.deref(ctx).get_region(0).deref(ctx).iter(ctx).collect()
}

/// A function's validated region structure.
pub struct FnPlan {
    root: RegionPlan,
}

impl FnPlan {
    /// Read the structure the layout declares and check every boundary
    /// of the graph against it.
    pub fn build(ctx: &Context, func: Ptr<Operation>) -> Result<Self, A1Error> {
        let layout: LayoutAttr = attr(ctx, func, &KEY_LAYOUT)
            .ok_or_else(|| invalid(ctx, func, "a normalized function carries its region layout"))?;
        let blocks = function_blocks(ctx, func);
        if blocks.len() != layout.0.len() {
            return Err(invalid(
                ctx,
                func,
                "the region layout records every block of the function, once",
            ));
        }
        let mut index = HashMap::new();
        for (block, record) in blocks.iter().zip(&layout.0) {
            if index.insert(record.clone(), *block).is_some() {
                return Err(invalid(ctx, func, "the region layout names a block twice"));
            }
        }
        let first = layout.0.first().map(|record| record.category);
        if first != Some(BlockCategory::Entry) {
            return Err(invalid(ctx, func, "a function begins at its entry block"));
        }
        let builder = PlanBuilder {
            ctx,
            func,
            records: blocks
                .iter()
                .copied()
                .zip(layout.0.iter().cloned())
                .collect(),
            index,
        };
        let exits = PlanExits {
            normal: None,
            error: None,
            resume: None,
        };
        let root = builder.region("", &exits)?;
        let covered = root.block_count();
        let propagate = layout
            .0
            .iter()
            .filter(|record| record.category == BlockCategory::Propagate)
            .count();
        if covered + propagate + 1 != blocks.len() {
            return Err(invalid(
                ctx,
                func,
                "a block of the function belongs to no region the graph reaches",
            ));
        }
        for block in builder.category_blocks(BlockCategory::Propagate) {
            builder.propagate(block)?;
        }
        Ok(Self { root })
    }
}

struct RegionPlan {
    path: String,
    blocks: Vec<BlockPlan>,
}

impl RegionPlan {
    fn block_count(&self) -> usize {
        self.blocks
            .iter()
            .map(|block| {
                block
                    .segments
                    .iter()
                    .map(|segment| match &segment.link {
                        Link::Try(plan) => 1 + plan.block_count(),
                        Link::Invoke | Link::End(_) => 1,
                    })
                    .sum::<usize>()
            })
            .sum()
    }
}

struct BlockPlan {
    segments: Vec<Segment>,
}

struct Segment {
    block: Ptr<BasicBlock>,
    link: Link,
}

/// How a segment hands control on.
enum Link {
    /// To the next segment, through a raising call's normal edge.
    Invoke,
    /// To the next segment, through a structured try.
    Try(Box<TryPlan>),
    End(End),
}

enum End {
    /// A terminator that exports as itself.
    Own,
    /// A region's normal exit.
    FallOff,
}

struct TryPlan {
    parts: TryAttr,
    cleanup: Vec<u32>,
    body: RegionPlan,
    handler: Option<RegionPlan>,
    orelse: Option<RegionPlan>,
    finalbody: Option<RegionPlan>,
    /// The blocks normalization synthesized for this try.
    synthesized: usize,
}

impl TryPlan {
    fn block_count(&self) -> usize {
        let part = |plan: &Option<RegionPlan>| plan.as_ref().map_or(0, RegionPlan::block_count);
        self.synthesized
            + self.body.block_count()
            + part(&self.handler)
            + part(&self.orelse)
            + part(&self.finalbody)
    }
}

/// Where control may leave a region.
#[derive(Clone)]
struct PlanExits {
    /// The target of a normal exit by branch.
    normal: Option<Ptr<BasicBlock>>,
    /// The target of an error edge; `None` hands the error out of the
    /// function.
    error: Option<Ptr<BasicBlock>>,
    /// The continuation and error target of a `finally` region's resume.
    resume: Option<(Ptr<BasicBlock>, Option<Ptr<BasicBlock>>)>,
}

/// What one cleanup block holds.
struct CleanupBlock {
    /// The owners it drops, in order.
    owners: Vec<u32>,
    /// The slot a caught error is bound to.
    caught: Option<u32>,
    exit: Ptr<Operation>,
}

struct PlanBuilder<'a> {
    ctx: &'a Context,
    func: Ptr<Operation>,
    records: HashMap<Ptr<BasicBlock>, BlockRecord>,
    index: HashMap<BlockRecord, Ptr<BasicBlock>>,
}

impl PlanBuilder<'_> {
    fn lookup(
        &self,
        category: BlockCategory,
        region: &str,
        block: u64,
        segment: u64,
    ) -> Option<Ptr<BasicBlock>> {
        self.index
            .get(&BlockRecord {
                category,
                region: region.into(),
                block,
                segment,
            })
            .copied()
    }

    fn category_blocks(&self, category: BlockCategory) -> Vec<Ptr<BasicBlock>> {
        let mut blocks: Vec<_> = self
            .records
            .iter()
            .filter(|(_, record)| record.category == category)
            .map(|(block, record)| (record.region.clone(), *block))
            .collect();
        blocks.sort_by(|left, right| left.0.cmp(&right.0));
        blocks.into_iter().map(|(_, block)| block).collect()
    }

    fn required(
        &self,
        category: BlockCategory,
        region: &str,
        anchor: Ptr<Operation>,
    ) -> Result<Ptr<BasicBlock>, A1Error> {
        self.lookup(category, region, 0, 0).ok_or_else(|| {
            invalid(
                self.ctx,
                anchor,
                &format!("try `{region}` has no {category:?} block"),
            )
        })
    }

    fn terminator(&self, block: Ptr<BasicBlock>) -> Result<Ptr<Operation>, A1Error> {
        block
            .deref(self.ctx)
            .get_tail()
            .ok_or_else(|| invalid(self.ctx, self.func, "a block without a terminator"))
    }

    fn successors(&self, op: Ptr<Operation>) -> Vec<Ptr<BasicBlock>> {
        op.deref(self.ctx).successors().collect()
    }

    /// An error edge lands on the region's error target; outside every
    /// try it lands on a block that hands the error out of the function.
    fn error_edge(
        &self,
        op: Ptr<Operation>,
        target: Ptr<BasicBlock>,
        exits: &PlanExits,
    ) -> Result<(), A1Error> {
        let lands = match exits.error {
            Some(expected) => target == expected,
            None => self
                .records
                .get(&target)
                .is_some_and(|record| record.category == BlockCategory::Propagate),
        };
        if lands {
            return Ok(());
        }
        Err(invalid(
            self.ctx,
            op,
            "an error edge bypasses the cleanup its region declares",
        ))
    }

    fn region(&self, path: &str, exits: &PlanExits) -> Result<RegionPlan, A1Error> {
        let ctx = self.ctx;
        let mut blocks = Vec::new();
        let mut tries = 0usize;
        let mut block = 0u64;
        while let Some(first) = self.lookup(BlockCategory::Body, path, block, 0) {
            let mut segments = Vec::new();
            let mut current = first;
            let mut segment = 0u64;
            loop {
                let term = self.terminator(current)?;
                let next = self.lookup(BlockCategory::Body, path, block, segment + 1);
                let successors = self.successors(term);
                let kind = CoreOpKind::of(ctx, term)
                    .ok_or_else(|| invalid(ctx, term, "an operation outside the registry"))?;
                let link = match kind {
                    CoreOpKind::Invoke => {
                        if next != successors.first().copied() || next.is_none() {
                            return Err(invalid(
                                ctx,
                                term,
                                "an invoke continues in the next segment of its block",
                            ));
                        }
                        self.error_edge(term, successors[1], exits)?;
                        Link::Invoke
                    }
                    CoreOpKind::Br => {
                        let target = successors[0];
                        let entered = self.records.get(&target).filter(|record| {
                            record.category == BlockCategory::Body
                                && record.block == 0
                                && record.segment == 0
                                && record.region.as_str()
                                    == format!("{}.body", try_path(path, tries))
                        });
                        if entered.is_some() {
                            let plan = self.try_plan(&try_path(path, tries), term, next, exits)?;
                            tries += 1;
                            Link::Try(Box::new(plan))
                        } else if Some(target) == exits.normal {
                            Link::End(End::FallOff)
                        } else if self.in_region(target, path) {
                            Link::End(End::Own)
                        } else {
                            return Err(invalid(
                                ctx,
                                term,
                                "a branch leaves its region outside the exits it declares",
                            ));
                        }
                    }
                    CoreOpKind::CondBr => {
                        if !successors
                            .iter()
                            .all(|target| self.in_region(*target, path))
                        {
                            return Err(invalid(
                                ctx,
                                term,
                                "a conditional branch leaves its region",
                            ));
                        }
                        Link::End(End::Own)
                    }
                    CoreOpKind::Return => {
                        if !path.is_empty() {
                            return Err(invalid(ctx, term, "a return crosses out of a try region"));
                        }
                        Link::End(End::Own)
                    }
                    CoreOpKind::Raise => {
                        match (successors.as_slice(), exits.error) {
                            ([], None) => {}
                            ([target], _) => self.error_edge(term, *target, exits)?,
                            _ => {
                                return Err(invalid(
                                    ctx,
                                    term,
                                    "a raise inside a try bypasses the cleanup its region declares",
                                ));
                            }
                        }
                        Link::End(End::Own)
                    }
                    CoreOpKind::Resume => {
                        let Some((after, error)) = exits.resume else {
                            return Err(invalid(
                                ctx,
                                term,
                                "a resume outside the finally body it dispatches for",
                            ));
                        };
                        if successors.first() != Some(&after) {
                            return Err(invalid(
                                ctx,
                                term,
                                "a resume continues after the try it belongs to",
                            ));
                        }
                        if let Some(target) = successors.get(1) {
                            let resume_exits = PlanExits {
                                error,
                                ..exits.clone()
                            };
                            self.error_edge(term, *target, &resume_exits)?;
                        }
                        Link::End(End::FallOff)
                    }
                    _ => {
                        return Err(invalid(ctx, term, "a block ends in no terminator"));
                    }
                };
                let ended = matches!(link, Link::End(_));
                segments.push(Segment {
                    block: current,
                    link,
                });
                if ended {
                    if next.is_some() {
                        return Err(invalid(ctx, term, "a segment follows the end of its block"));
                    }
                    break;
                }
                segment += 1;
                current =
                    next.ok_or_else(|| invalid(ctx, term, "a block's next segment is missing"))?;
            }
            blocks.push(BlockPlan { segments });
            block += 1;
        }
        if blocks.is_empty() {
            return Err(invalid(
                ctx,
                self.func,
                &format!("region `{path}` has no block"),
            ));
        }
        Ok(RegionPlan {
            path: path.to_string(),
            blocks,
        })
    }

    fn in_region(&self, target: Ptr<BasicBlock>, path: &str) -> bool {
        self.records.get(&target).is_some_and(|record| {
            record.category == BlockCategory::Body
                && record.region.as_str() == path
                && record.segment == 0
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one try's boundaries are checked together"
    )]
    fn try_plan(
        &self,
        path: &str,
        enter: Ptr<Operation>,
        after: Option<Ptr<BasicBlock>>,
        outer: &PlanExits,
    ) -> Result<TryPlan, A1Error> {
        let ctx = self.ctx;
        let parts: TryAttr = attr(ctx, enter, &KEY_TRY)
            .ok_or_else(|| invalid(ctx, enter, "the branch into a try carries its parts"))?;
        let after = after.ok_or_else(|| {
            invalid(
                ctx,
                enter,
                "a try continues in the next segment of its block",
            )
        })?;
        let unwind = self.required(BlockCategory::Unwind, path, enter)?;
        let done = self.required(BlockCategory::BodyDone, path, enter)?;
        let pending = if parts.finalbody {
            Some((
                self.required(BlockCategory::PendingNormal, path, enter)?,
                self.required(BlockCategory::PendingError, path, enter)?,
                self.required(BlockCategory::FinallyEntry, path, enter)?,
            ))
        } else {
            None
        };
        let normal = pending.map_or(after, |(normal, _, _)| normal);
        let part_error = pending.map_or(outer.error, |(_, error, _)| Some(error));
        let part_exits = PlanExits {
            normal: Some(normal),
            error: part_error,
            resume: None,
        };
        let body = self.region(
            &format!("{path}.body"),
            &PlanExits {
                normal: Some(done),
                error: Some(unwind),
                resume: None,
            },
        )?;
        let part = |present: bool, name: &str, exits: &PlanExits| {
            present
                .then(|| self.region(&format!("{path}.{name}"), exits))
                .transpose()
        };
        let handler = part(parts.handler, "handler", &part_exits)?;
        let orelse = part(parts.orelse, "else", &part_exits)?;
        let finalbody = part(
            parts.finalbody,
            "finally",
            &PlanExits {
                normal: None,
                error: outer.error,
                resume: Some((after, outer.error)),
            },
        )?;
        let entry = |plan: &RegionPlan| plan.blocks[0].segments[0].block;

        let CleanupBlock {
            owners: cleanup,
            caught,
            exit: unwind_exit,
        } = self.cleanup_block(unwind, true)?;
        if caught != parts.error_var.filter(|_| parts.handler) {
            return Err(invalid(
                ctx,
                unwind_exit,
                "the error edge binds the caught error to the slot its handler declares",
            ));
        }
        let unwind_target = self.successors(unwind_exit);
        match &handler {
            Some(handler) if unwind_target == [entry(handler)] => {}
            Some(_) => {
                return Err(invalid(
                    ctx,
                    unwind_exit,
                    "the error edge enters its handler after cleanup",
                ));
            }
            None => self.error_edge(
                unwind_exit,
                unwind_target[0],
                &PlanExits {
                    error: part_error,
                    ..outer.clone()
                },
            )?,
        }
        let normal_edge = self.cleanup_block(done, false)?;
        let done_exit = normal_edge.exit;
        if normal_edge.owners != cleanup {
            return Err(invalid(
                ctx,
                done_exit,
                "the normal edge and the error edge run one cleanup list",
            ));
        }
        let onward = orelse.as_ref().map_or(normal, entry);
        if self.successors(done_exit) != [onward] {
            return Err(invalid(
                ctx,
                done_exit,
                "the normal edge continues to `else`, `finally`, or past the try",
            ));
        }
        let mut synthesized = 2;
        if let Some((pending_normal, pending_error, finally_entry)) = pending {
            synthesized += 3;
            let finalbody = finalbody
                .as_ref()
                .ok_or_else(|| invalid(ctx, enter, "a try's finally region is missing"))?;
            self.pending_block(pending_normal, OutcomeKind::Normal, finally_entry)?;
            self.pending_block(pending_error, OutcomeKind::Error, finally_entry)?;
            let ops: Vec<_> = finally_entry.deref(ctx).iter(ctx).collect();
            let enters = ops.len() == 1
                && CoreOpKind::of(ctx, ops[0]) == Some(CoreOpKind::Br)
                && self.successors(ops[0]) == [entry(finalbody)];
            if !enters {
                return Err(invalid(
                    ctx,
                    enter,
                    "the single finally entry branches to the finally body",
                ));
            }
        }
        Ok(TryPlan {
            parts,
            cleanup,
            body,
            handler,
            orelse,
            finalbody,
            synthesized,
        })
    }

    /// The owners a cleanup block drops in order, the slot a caught error
    /// is bound to, and the block's exit.
    fn cleanup_block(
        &self,
        block: Ptr<BasicBlock>,
        error_edge: bool,
    ) -> Result<CleanupBlock, A1Error> {
        let ctx = self.ctx;
        let ops: Vec<_> = block.deref(ctx).iter(ctx).collect();
        let Some((exit, body)) = ops.split_last() else {
            return Err(invalid(ctx, self.func, "an empty cleanup block"));
        };
        if CoreOpKind::of(ctx, *exit) != Some(CoreOpKind::Br) {
            return Err(invalid(ctx, *exit, "a cleanup block ends in a branch"));
        }
        let mut owners = Vec::new();
        let mut caught = None;
        for op in body {
            let identity: IdentityAttr = attr(ctx, *op, &KEY_IDENTITY)
                .ok_or_else(|| invalid(ctx, *op, "an operation without identity"))?;
            let position = owners.len() as u64;
            let expected = if error_edge {
                CoreRole::UnwindDrop(position)
            } else {
                CoreRole::DoneDrop(position)
            };
            let lifecycle = attr::<LifecycleAttr>(ctx, *op, &KEY_LIFECYCLE);
            match (CoreOpKind::of(ctx, *op), lifecycle) {
                (Some(CoreOpKind::Drop), Some(lifecycle))
                    if caught.is_none()
                        && identity.role == expected
                        && lifecycle.kind == CoreLifecycle::DropVar =>
                {
                    owners.push(lifecycle.owner);
                }
                (Some(CoreOpKind::Store), _)
                    if error_edge
                        && caught.is_none()
                        && attr::<StoreAttr>(ctx, *op, &KEY_STORE) == Some(StoreAttr::Caught) =>
                {
                    let slot = op
                        .deref(ctx)
                        .operands()
                        .next()
                        .and_then(|place| place.defining_op())
                        .and_then(|slot| attr::<SlotAttr>(ctx, slot, &KEY_SLOT))
                        .filter(|slot| slot.storage == CoreStorage::Variable)
                        .ok_or_else(|| invalid(ctx, *op, "a caught error bound to no variable"))?;
                    caught = Some(slot.id);
                }
                _ => {
                    return Err(invalid(
                        ctx,
                        *op,
                        "a cleanup block holds its ordered drops, then the caught-error binding",
                    ));
                }
            }
        }
        Ok(CleanupBlock {
            owners,
            caught,
            exit: *exit,
        })
    }

    fn pending_block(
        &self,
        block: Ptr<BasicBlock>,
        kind: OutcomeKind,
        finally_entry: Ptr<BasicBlock>,
    ) -> Result<(), A1Error> {
        let ctx = self.ctx;
        let ops: Vec<_> = block.deref(ctx).iter(ctx).collect();
        let shaped = ops.len() == 2
            && attr::<OutcomeAttr>(ctx, ops[0], &KEY_OUTCOME) == Some(OutcomeAttr { kind })
            && CoreOpKind::of(ctx, ops[1]) == Some(CoreOpKind::Br)
            && self.successors(ops[1]) == [finally_entry];
        if shaped {
            return Ok(());
        }
        Err(invalid(
            ctx,
            ops.first().copied().unwrap_or(self.func),
            "an entry to finally records its pending outcome and enters the one finally entry",
        ))
    }

    fn propagate(&self, block: Ptr<BasicBlock>) -> Result<(), A1Error> {
        let ctx = self.ctx;
        let ops: Vec<_> = block.deref(ctx).iter(ctx).collect();
        let shaped = ops.len() == 1
            && CoreOpKind::of(ctx, ops[0]) == Some(CoreOpKind::Raise)
            && self.successors(ops[0]).is_empty();
        if shaped {
            return Ok(());
        }
        Err(invalid(
            ctx,
            self.func,
            "a propagation block raises its error out of the function",
        ))
    }
}

fn invalid(ctx: &Context, op: Ptr<Operation>, message: &str) -> A1Error {
    A1Error::new(
        A1ErrorKind::Verification,
        format!("{message} ({})", describe(ctx, op)),
    )
}

/// Insert `rebuilt` where `func` stands and erase `func`.
fn replace(ctx: &mut Context, func: Ptr<Operation>, rebuilt: Ptr<Operation>) {
    rebuilt.insert_before(ctx, func);
    Operation::erase(func, ctx);
}

/// A function rebuilt operation by operation, values remapped.
struct Rebuild {
    func: Ptr<Operation>,
    region: Ptr<Region>,
    name: Text,
    values: HashMap<Value, Value>,
    blocks: HashMap<Ptr<BasicBlock>, Ptr<BasicBlock>>,
    /// Each variable's slot in the rebuilt function.
    variables: HashMap<u32, Value>,
    effect: TypeHandle,
}

impl Rebuild {
    /// Start the rebuilt function: its attributes, its entry block, and
    /// its slots.
    fn start(ctx: &mut Context, old: Ptr<Operation>) -> Result<(Self, Ptr<BasicBlock>), A1Error> {
        let identity: IdentityAttr = attr(ctx, old, &KEY_IDENTITY)
            .ok_or_else(|| invalid(ctx, old, "a function without identity"))?;
        let func = ops::build(ctx, CoreOpKind::Func, vec![], vec![], vec![], 1);
        let attributes = old.deref(ctx).attributes.clone();
        func.deref_mut(ctx).attributes = attributes;
        let location = old.deref(ctx).loc();
        func.deref_mut(ctx).set_loc(location);
        let region = func.deref(ctx).get_region(0);
        let old_entry = old
            .deref(ctx)
            .get_region(0)
            .deref(ctx)
            .get_entry_block()
            .ok_or_else(|| invalid(ctx, old, "a function without an entry block"))?;
        let mut rebuild = Self {
            func,
            region,
            name: identity.function,
            values: HashMap::new(),
            blocks: HashMap::new(),
            variables: HashMap::new(),
            effect: EffectType::get(ctx).into(),
        };
        let entry = rebuild.mirror_block(ctx, old_entry);
        for op in block_ops(ctx, old_entry) {
            let Some(slot) = attr::<SlotAttr>(ctx, op, &KEY_SLOT) else {
                continue;
            };
            let copy = rebuild.copy(ctx, op, None, &[])?;
            copy.insert_at_back(entry, ctx);
            if slot.storage == CoreStorage::Variable {
                rebuild
                    .variables
                    .insert(slot.id, copy.deref(ctx).get_result(0));
            }
        }
        Ok((rebuild, entry))
    }

    /// A new block in the rebuilt region with `old`'s arguments.
    fn mirror_block(&mut self, ctx: &mut Context, old: Ptr<BasicBlock>) -> Ptr<BasicBlock> {
        let arguments: Vec<TypeHandle> = old
            .deref(ctx)
            .arguments()
            .map(|value| value.get_type(ctx))
            .collect();
        let block = self.block(ctx, arguments);
        let pairs: Vec<(Value, Value)> = old
            .deref(ctx)
            .arguments()
            .zip(block.deref(ctx).arguments())
            .collect();
        self.values.extend(pairs);
        self.blocks.insert(old, block);
        let location = old.deref(ctx).loc();
        block.deref_mut(ctx).set_loc(location);
        block
    }

    fn block(&self, ctx: &mut Context, arguments: Vec<TypeHandle>) -> Ptr<BasicBlock> {
        let block = BasicBlock::new(ctx, None, arguments);
        block.insert_at_back(self.region, ctx);
        block
    }

    fn value(&self, ctx: &Context, user: Ptr<Operation>, old: Value) -> Result<Value, A1Error> {
        self.values
            .get(&old)
            .copied()
            .ok_or_else(|| invalid(ctx, user, "an operand defined outside the rebuilt function"))
    }

    fn variable(&self, ctx: &Context, user: Ptr<Operation>, var: u32) -> Result<Value, A1Error> {
        self.variables
            .get(&var)
            .copied()
            .ok_or_else(|| invalid(ctx, user, "a cleanup of an undeclared variable"))
    }

    /// An unlinked copy of `old` as `kind` (its own kind when `None`),
    /// with `old`'s attributes, location, and remapped operands, and with
    /// `successors`. Results are remapped when the copy keeps them all.
    fn copy(
        &mut self,
        ctx: &mut Context,
        old: Ptr<Operation>,
        kind: Option<(CoreOpKind, Vec<TypeHandle>)>,
        successors: &[Ptr<BasicBlock>],
    ) -> Result<Ptr<Operation>, A1Error> {
        let own = CoreOpKind::of(ctx, old)
            .ok_or_else(|| invalid(ctx, old, "an operation outside the registry"))?;
        let old_results: Vec<Value> = old.deref(ctx).results().collect();
        let (kind, results) = kind.unwrap_or_else(|| {
            let types = old_results
                .iter()
                .map(|value| value.get_type(ctx))
                .collect();
            (own, types)
        });
        let mut operands: Vec<Value> = old.deref(ctx).operands().collect();
        if kind != own && matches!(kind, CoreOpKind::Br | CoreOpKind::RegionExit) {
            // A branch or region exit standing for another operation
            // forwards the effect token alone.
            operands.drain(..operands.len().saturating_sub(1));
        }
        let operands = operands
            .into_iter()
            .map(|value| self.value(ctx, old, value))
            .collect::<Result<Vec<_>, _>>()?;
        let copy = ops::build(ctx, kind, results, operands, successors.to_vec(), 0);
        let attributes = old.deref(ctx).attributes.clone();
        copy.deref_mut(ctx).attributes = attributes;
        let location = old.deref(ctx).loc();
        copy.deref_mut(ctx).set_loc(location);
        let new_results: Vec<Value> = copy.deref(ctx).results().collect();
        if new_results.len() == old_results.len() {
            self.values.extend(old_results.into_iter().zip(new_results));
        }
        Ok(copy)
    }

    /// A synthesized operation of this function, annotated and linked.
    #[allow(clippy::too_many_arguments, reason = "one operation, fully described")]
    fn synthesize(
        &self,
        ctx: &mut Context,
        block: Ptr<BasicBlock>,
        kind: CoreOpKind,
        results: Vec<TypeHandle>,
        operands: Vec<Value>,
        successors: Vec<Ptr<BasicBlock>>,
        region: &str,
        role: CoreRole,
        derived_from: &IdentityAttr,
        reason: &str,
    ) -> Ptr<Operation> {
        let op = ops::build(ctx, kind, results, operands, successors, 0);
        annotate(
            ctx,
            op,
            IdentityAttr {
                function: self.name.clone(),
                region: region.into(),
                block: 0,
                ordinal: 0,
                role,
            },
            ProvenanceAttr {
                span: None,
                origin: None,
                derived_from: vec![derived_from.key().into()],
                reason: reason.into(),
            },
        );
        op.insert_at_back(block, ctx);
        op
    }
}

/// The target of a normal region exit.
#[derive(Clone)]
enum NormalExit {
    /// The function body has none.
    None,
    Branch(Ptr<BasicBlock>),
    /// A `finally` body dispatches on its pending outcome.
    Resume {
        pending: Value,
        after: Ptr<BasicBlock>,
        /// The block a pending error continues to, when one can be pending.
        error: Option<Ptr<BasicBlock>>,
    },
}

/// The target of an error edge.
#[derive(Clone, Copy)]
enum ErrorExit {
    /// Out of the function.
    Propagate,
    Block(Ptr<BasicBlock>),
}

#[derive(Clone)]
struct Exits {
    normal: NormalExit,
    error: ErrorExit,
}

struct Normalizer {
    rebuild: Rebuild,
    layout: Vec<BlockRecord>,
    /// The error type each lazily typed error block takes.
    error_types: HashMap<Ptr<BasicBlock>, TypeHandle>,
    propagate: HashMap<TypeHandle, Ptr<BasicBlock>>,
    outcome: TypeHandle,
    function: IdentityAttr,
}

impl Normalizer {
    fn run(ctx: &mut Context, old: Ptr<Operation>) -> Result<Ptr<Operation>, A1Error> {
        let function: IdentityAttr = attr(ctx, old, &KEY_IDENTITY)
            .ok_or_else(|| invalid(ctx, old, "a function without identity"))?;
        let (rebuild, entry) = Rebuild::start(ctx, old)?;
        let mut normalizer = Self {
            rebuild,
            layout: vec![record(BlockCategory::Entry, "", 0, 0)],
            error_types: HashMap::new(),
            propagate: HashMap::new(),
            outcome: OutcomeType::get(ctx).into(),
            function,
        };
        let old_region = old.deref(ctx).get_region(0);
        let exits = Exits {
            normal: NormalExit::None,
            error: ErrorExit::Propagate,
        };
        let first = normalizer.region(ctx, old_region, "", &exits)?;
        let old_entry = old_region
            .deref(ctx)
            .get_entry_block()
            .and_then(|block| block.deref(ctx).get_tail())
            .ok_or_else(|| invalid(ctx, old, "a function entry without its branch"))?;
        let branch = normalizer.rebuild.copy(ctx, old_entry, None, &[first])?;
        branch.insert_at_back(entry, ctx);
        let func = normalizer.rebuild.func;
        set(ctx, func, &KEY_LAYOUT, LayoutAttr(normalizer.layout));
        let contract = derive_contract(ctx, func)?;
        set(ctx, func, &KEY_CONTRACT, contract);
        Ok(func)
    }

    fn new_block(
        &mut self,
        ctx: &mut Context,
        arguments: Vec<TypeHandle>,
        layout: BlockRecord,
    ) -> Ptr<BasicBlock> {
        self.layout.push(layout);
        self.rebuild.block(ctx, arguments)
    }

    /// The block an error of type `ty` leaves through.
    fn error_block(
        &mut self,
        ctx: &mut Context,
        user: Ptr<Operation>,
        exit: ErrorExit,
        ty: TypeHandle,
    ) -> Result<Ptr<BasicBlock>, A1Error> {
        match exit {
            ErrorExit::Block(block) => {
                let recorded = *self.error_types.entry(block).or_insert(ty);
                if recorded != ty {
                    return Err(A1Error::new(
                        A1ErrorKind::UnsupportedForm,
                        format!(
                            "a try reached by errors of two types has no core form ({})",
                            describe(ctx, user)
                        ),
                    ));
                }
                Ok(block)
            }
            ErrorExit::Propagate => {
                if let Some(block) = self.propagate.get(&ty) {
                    return Ok(*block);
                }
                let index = self.propagate.len() as u64;
                let block = self.new_block(
                    ctx,
                    vec![ty, self.rebuild.effect],
                    record(BlockCategory::Propagate, "", index, 0),
                );
                let operands = block.deref(ctx).arguments().collect();
                let raise = self.rebuild.synthesize(
                    ctx,
                    block,
                    CoreOpKind::Raise,
                    vec![],
                    operands,
                    vec![],
                    &format!("propagate{index}"),
                    CoreRole::Propagate,
                    &self.function.clone(),
                    "error leaves the function",
                );
                set(
                    ctx,
                    raise,
                    &ops::KEY_DEAD_TERM,
                    super::attrs::DeadTermAttr::Return,
                );
                self.propagate.insert(ty, block);
                Ok(block)
            }
        }
    }

    /// Emit the blocks of `old` after its synthesized entry; the result
    /// is the block standing for the region's block 0.
    fn region(
        &mut self,
        ctx: &mut Context,
        old: Ptr<Region>,
        path: &str,
        exits: &Exits,
    ) -> Result<Ptr<BasicBlock>, A1Error> {
        let blocks: Vec<Ptr<BasicBlock>> = old.deref(ctx).iter(ctx).skip(1).collect();
        let mut firsts = Vec::new();
        for (index, block) in blocks.iter().enumerate() {
            self.layout
                .push(record(BlockCategory::Body, path, index as u64, 0));
            firsts.push(self.rebuild.mirror_block(ctx, *block));
        }
        let Some(first) = firsts.first().copied() else {
            return Err(invalid(ctx, self.rebuild.func, "a region without a block"));
        };
        let mut tries = 0usize;
        for (index, block) in blocks.iter().enumerate() {
            let mut current = firsts[index];
            let mut segment = 0u64;
            for op in block_ops(ctx, *block) {
                let kind = CoreOpKind::of(ctx, op)
                    .ok_or_else(|| invalid(ctx, op, "an operation outside the registry"))?;
                match kind {
                    CoreOpKind::TryBridge => {
                        segment += 1;
                        let after = self.new_block(
                            ctx,
                            vec![self.rebuild.effect],
                            record(BlockCategory::Body, path, index as u64, segment),
                        );
                        let try_path = try_path(path, tries);
                        tries += 1;
                        self.structured_try(ctx, op, &try_path, current, after, exits)?;
                        let token = op.deref(ctx).get_result(0);
                        self.rebuild
                            .values
                            .insert(token, after.deref(ctx).get_argument(0));
                        current = after;
                    }
                    CoreOpKind::Call | CoreOpKind::Index | CoreOpKind::MultiSet
                        if raised_by(ctx, op).is_some() =>
                    {
                        segment += 1;
                        let results: Vec<Value> = op.deref(ctx).results().collect();
                        let types = results.iter().map(|value| value.get_type(ctx)).collect();
                        let next = self.new_block(
                            ctx,
                            types,
                            record(BlockCategory::Body, path, index as u64, segment),
                        );
                        let raised = raised_by(ctx, op)
                            .ok_or_else(|| invalid(ctx, op, "a raising call names its error"))?;
                        let error = self.error_block(ctx, op, exits.error, raised)?;
                        let invoke = self.rebuild.copy(
                            ctx,
                            op,
                            Some((CoreOpKind::Invoke, vec![])),
                            &[next, error],
                        )?;
                        invoke.insert_at_back(current, ctx);
                        let arguments: Vec<Value> = next.deref(ctx).arguments().collect();
                        self.rebuild
                            .values
                            .extend(results.into_iter().zip(arguments));
                        current = next;
                    }
                    CoreOpKind::RegionExit => {
                        let copy = match &exits.normal {
                            NormalExit::Branch(target) => self.rebuild.copy(
                                ctx,
                                op,
                                Some((CoreOpKind::Br, vec![])),
                                &[*target],
                            )?,
                            NormalExit::Resume {
                                pending,
                                after,
                                error,
                            } => {
                                let successors: Vec<_> =
                                    std::iter::once(*after).chain(*error).collect();
                                let resume = self.rebuild.copy(
                                    ctx,
                                    op,
                                    Some((CoreOpKind::Resume, vec![])),
                                    &successors,
                                )?;
                                Operation::insert_operand(resume, ctx, 0, *pending);
                                resume
                            }
                            NormalExit::None => {
                                return Err(invalid(
                                    ctx,
                                    op,
                                    "a region exit outside a structured region",
                                ));
                            }
                        };
                        copy.insert_at_back(current, ctx);
                    }
                    CoreOpKind::Raise => {
                        let raised = op.deref(ctx).get_operand(0).get_type(ctx);
                        let successors = match exits.error {
                            ErrorExit::Propagate => Vec::new(),
                            ErrorExit::Block(_) => {
                                vec![self.error_block(ctx, op, exits.error, raised)?]
                            }
                        };
                        let copy = self.rebuild.copy(ctx, op, None, &successors)?;
                        copy.insert_at_back(current, ctx);
                    }
                    CoreOpKind::Br | CoreOpKind::CondBr => {
                        let successors =
                            op.deref(ctx)
                                .successors()
                                .map(|target| {
                                    self.rebuild.blocks.get(&target).copied().ok_or_else(|| {
                                        invalid(ctx, op, "a branch out of its region")
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                        let copy = self.rebuild.copy(ctx, op, None, &successors)?;
                        copy.insert_at_back(current, ctx);
                    }
                    CoreOpKind::Return if !path.is_empty() => {
                        return Err(A1Error::new(
                            A1ErrorKind::UnsupportedForm,
                            format!(
                                "a return crossing out of a try region has no core form ({})",
                                describe(ctx, op)
                            ),
                        ));
                    }
                    _ => {
                        let copy = self.rebuild.copy(ctx, op, None, &[])?;
                        copy.insert_at_back(current, ctx);
                    }
                }
            }
        }
        Ok(first)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one try's edges are laid out together"
    )]
    fn structured_try(
        &mut self,
        ctx: &mut Context,
        op: Ptr<Operation>,
        path: &str,
        current: Ptr<BasicBlock>,
        after: Ptr<BasicBlock>,
        outer: &Exits,
    ) -> Result<(), A1Error> {
        let parts: TryAttr = attr(ctx, op, &KEY_TRY)
            .ok_or_else(|| invalid(ctx, op, "a structured try carries its parts"))?;
        let identity: IdentityAttr = attr(ctx, op, &KEY_IDENTITY)
            .ok_or_else(|| invalid(ctx, op, "an operation without identity"))?;
        let operands: Vec<Value> = op.deref(ctx).operands().collect();
        let token = self.rebuild.value(ctx, op, operands[operands.len() - 1])?;
        let cleanup = operands[..operands.len() - 1]
            .iter()
            .map(|slot| {
                slot.defining_op()
                    .and_then(|slot| attr::<SlotAttr>(ctx, slot, &KEY_SLOT))
                    .map(|slot| slot.id)
                    .ok_or_else(|| invalid(ctx, op, "a cleanup operand that is no slot"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let regions: Vec<Ptr<Region>> = op.deref(ctx).regions().collect();
        let effect = self.rebuild.effect;

        let unwind = self.new_block(ctx, vec![], record(BlockCategory::Unwind, path, 0, 0));
        let done = self.new_block(
            ctx,
            vec![effect],
            record(BlockCategory::BodyDone, path, 0, 0),
        );
        let pending = if parts.finalbody {
            let normal = self.new_block(
                ctx,
                vec![effect],
                record(BlockCategory::PendingNormal, path, 0, 0),
            );
            let error =
                self.new_block(ctx, vec![], record(BlockCategory::PendingError, path, 0, 0));
            let entry = self.new_block(
                ctx,
                vec![self.outcome, effect],
                record(BlockCategory::FinallyEntry, path, 0, 0),
            );
            Some((normal, error, entry))
        } else {
            None
        };
        let part_exits = Exits {
            normal: NormalExit::Branch(pending.map_or(after, |(normal, _, _)| normal)),
            error: pending.map_or(outer.error, |(_, error, _)| ErrorExit::Block(error)),
        };

        let body = self.region(
            ctx,
            regions[0],
            &format!("{path}.body"),
            &Exits {
                normal: NormalExit::Branch(done),
                error: ErrorExit::Block(unwind),
            },
        )?;
        let enter = self
            .rebuild
            .copy(ctx, op, Some((CoreOpKind::Br, vec![])), &[body])?;
        enter.insert_at_back(current, ctx);
        debug_assert_eq!(enter.deref(ctx).get_operand(0), token);

        let handler = if parts.handler {
            Some(self.region(ctx, regions[1], &format!("{path}.handler"), &part_exits)?)
        } else {
            None
        };
        let orelse = if parts.orelse {
            Some(self.region(ctx, regions[2], &format!("{path}.else"), &part_exits)?)
        } else {
            None
        };

        // The error edge: ordered cleanup, then the handler or the way out.
        let raised = self
            .error_types
            .get(&unwind)
            .copied()
            .unwrap_or_else(|| ErrorType::get(ctx).into());
        BasicBlock::push_argument(unwind, ctx, raised);
        BasicBlock::push_argument(unwind, ctx, effect);
        let error = unwind.deref(ctx).get_argument(0);
        let mut chain = unwind.deref(ctx).get_argument(1);
        chain = self.cleanup(ctx, unwind, path, &cleanup, chain, true, &identity, op)?;
        if let (Some(var), true) = (parts.error_var, parts.handler) {
            let slot = self.rebuild.variable(ctx, op, var)?;
            let store = self.rebuild.synthesize(
                ctx,
                unwind,
                CoreOpKind::Store,
                vec![effect],
                vec![slot, error, chain],
                vec![],
                path,
                CoreRole::Caught,
                &identity,
                "caught error binding",
            );
            set(ctx, store, &KEY_STORE, StoreAttr::Caught);
            chain = store.deref(ctx).get_result(0);
        }
        let (target, operands) = match handler {
            Some(handler) => (handler, vec![chain]),
            None => (
                self.error_block(ctx, op, part_exits.error, raised)?,
                vec![error, chain],
            ),
        };
        self.rebuild.synthesize(
            ctx,
            unwind,
            CoreOpKind::Br,
            vec![],
            operands,
            vec![target],
            path,
            CoreRole::UnwindExit,
            &identity,
            "exceptional cleanup exit",
        );

        // The normal edge: the same cleanup, then `else` or the way on.
        let mut chain = done.deref(ctx).get_argument(0);
        chain = self.cleanup(ctx, done, path, &cleanup, chain, false, &identity, op)?;
        let NormalExit::Branch(onward) = part_exits.normal else {
            return Err(invalid(ctx, op, "a try part without a normal exit"));
        };
        self.rebuild.synthesize(
            ctx,
            done,
            CoreOpKind::Br,
            vec![],
            vec![chain],
            vec![orelse.unwrap_or(onward)],
            path,
            CoreRole::DoneExit,
            &identity,
            "normal cleanup exit",
        );

        let Some((pending_normal, pending_error, finally_entry)) = pending else {
            return Ok(());
        };
        let chain = pending_normal.deref(ctx).get_argument(0);
        self.pending(
            ctx,
            pending_normal,
            finally_entry,
            path,
            OutcomeKind::Normal,
            vec![chain],
            &identity,
        );
        let reaches = self.error_types.get(&pending_error).copied();
        let raised = reaches.unwrap_or_else(|| ErrorType::get(ctx).into());
        BasicBlock::push_argument(pending_error, ctx, raised);
        BasicBlock::push_argument(pending_error, ctx, effect);
        let operands = pending_error.deref(ctx).arguments().collect();
        self.pending(
            ctx,
            pending_error,
            finally_entry,
            path,
            OutcomeKind::Error,
            operands,
            &identity,
        );
        let error = match reaches {
            Some(raised) => Some(self.error_block(ctx, op, outer.error, raised)?),
            None => None,
        };
        let exits = Exits {
            normal: NormalExit::Resume {
                pending: finally_entry.deref(ctx).get_argument(0),
                after,
                error,
            },
            error: outer.error,
        };
        let finalbody = self.region(ctx, regions[3], &format!("{path}.finally"), &exits)?;
        let chain = finally_entry.deref(ctx).get_argument(1);
        self.rebuild.synthesize(
            ctx,
            finally_entry,
            CoreOpKind::Br,
            vec![],
            vec![chain],
            vec![finalbody],
            path,
            CoreRole::FinallyEntry,
            &identity,
            "single finally entry",
        );
        Ok(())
    }

    /// The ordered cleanup drops of one edge; the result is the effect
    /// token after them.
    #[allow(clippy::too_many_arguments, reason = "one edge, fully described")]
    fn cleanup(
        &self,
        ctx: &mut Context,
        block: Ptr<BasicBlock>,
        path: &str,
        cleanup: &[u32],
        mut chain: Value,
        error_edge: bool,
        identity: &IdentityAttr,
        op: Ptr<Operation>,
    ) -> Result<Value, A1Error> {
        for (position, var) in cleanup.iter().enumerate() {
            let slot = self.rebuild.variable(ctx, op, *var)?;
            let position = position as u64;
            let (role, edge) = if error_edge {
                (CoreRole::UnwindDrop(position), "exceptional")
            } else {
                (CoreRole::DoneDrop(position), "normal")
            };
            let drop = self.rebuild.synthesize(
                ctx,
                block,
                CoreOpKind::Drop,
                vec![self.rebuild.effect],
                vec![slot, chain],
                vec![],
                path,
                role,
                identity,
                &format!("{edge} cleanup slot {position}"),
            );
            set(
                ctx,
                drop,
                &KEY_LIFECYCLE,
                LifecycleAttr {
                    kind: CoreLifecycle::DropVar,
                    owner: *var,
                    path: Vec::new(),
                },
            );
            chain = drop.deref(ctx).get_result(0);
        }
        Ok(chain)
    }

    /// Record the pending outcome of one entry to `finally` and enter it.
    #[allow(clippy::too_many_arguments, reason = "one edge, fully described")]
    fn pending(
        &self,
        ctx: &mut Context,
        block: Ptr<BasicBlock>,
        finally_entry: Ptr<BasicBlock>,
        path: &str,
        kind: OutcomeKind,
        operands: Vec<Value>,
        identity: &IdentityAttr,
    ) {
        let (role, exit, reason) = match kind {
            OutcomeKind::Normal => (
                CoreRole::PendingNormal,
                CoreRole::PendingNormalExit,
                "pending normal outcome",
            ),
            OutcomeKind::Error => (
                CoreRole::PendingError,
                CoreRole::PendingErrorExit,
                "pending error outcome",
            ),
        };
        let outcome = self.rebuild.synthesize(
            ctx,
            block,
            CoreOpKind::Outcome,
            vec![self.outcome, self.rebuild.effect],
            operands,
            vec![],
            path,
            role,
            identity,
            reason,
        );
        set(ctx, outcome, &KEY_OUTCOME, OutcomeAttr { kind });
        let results = outcome.deref(ctx).results().collect();
        self.rebuild.synthesize(
            ctx,
            block,
            CoreOpKind::Br,
            vec![],
            results,
            vec![finally_entry],
            path,
            exit,
            identity,
            reason,
        );
    }
}

fn record(category: BlockCategory, region: &str, block: u64, segment: u64) -> BlockRecord {
    BlockRecord {
        category,
        region: region.into(),
        block,
        segment,
    }
}

struct Denormalizer {
    rebuild: Rebuild,
}

impl Denormalizer {
    fn run(
        ctx: &mut Context,
        old: Ptr<Operation>,
        plan: &FnPlan,
    ) -> Result<Ptr<Operation>, A1Error> {
        let (rebuild, entry) = Rebuild::start(ctx, old)?;
        let mut denormalizer = Self { rebuild };
        let func = denormalizer.rebuild.func;
        let region = denormalizer.rebuild.region;
        let old_entry = old
            .deref(ctx)
            .get_region(0)
            .deref(ctx)
            .get_entry_block()
            .and_then(|block| block.deref(ctx).get_tail())
            .ok_or_else(|| invalid(ctx, old, "a function entry without its branch"))?;
        let first = denormalizer.region(ctx, region, &plan.root)?;
        let branch = denormalizer.rebuild.copy(ctx, old_entry, None, &[first])?;
        branch.insert_at_back(entry, ctx);
        let mut attributes = func.deref(ctx).attributes.clone();
        attributes
            .0
            .retain(|key, _| *key != *KEY_LAYOUT && *key != *KEY_CONTRACT);
        func.deref_mut(ctx).attributes = attributes;
        Ok(func)
    }

    /// Emit the blocks of `plan` into `region`; the result stands for the
    /// region's block 0.
    fn region(
        &mut self,
        ctx: &mut Context,
        region: Ptr<Region>,
        plan: &RegionPlan,
    ) -> Result<Ptr<BasicBlock>, A1Error> {
        let effect = self.rebuild.effect;
        let mut targets = Vec::new();
        for block in &plan.blocks {
            let target = BasicBlock::new(ctx, None, vec![effect]);
            target.insert_at_back(region, ctx);
            let first = block.segments[0].block;
            self.rebuild.values.insert(
                first.deref(ctx).get_argument(0),
                target.deref(ctx).get_argument(0),
            );
            self.rebuild.blocks.insert(first, target);
            let location = first.deref(ctx).loc();
            target.deref_mut(ctx).set_loc(location);
            targets.push(target);
        }
        for (block, target) in plan.blocks.iter().zip(&targets) {
            for (position, segment) in block.segments.iter().enumerate() {
                let ops: Vec<_> = segment.block.deref(ctx).iter(ctx).collect();
                let Some((term, body)) = ops.split_last() else {
                    return Err(invalid(ctx, self.rebuild.func, "an empty block"));
                };
                for op in body {
                    let copy = self.rebuild.copy(ctx, *op, None, &[])?;
                    copy.insert_at_back(*target, ctx);
                }
                let copy = match &segment.link {
                    Link::Invoke => {
                        let next = term.deref(ctx).get_successor(0);
                        let arguments: Vec<Value> = next.deref(ctx).arguments().collect();
                        let types = arguments.iter().map(|value| value.get_type(ctx)).collect();
                        let invoked = invoked_kind(ctx, *term).ok_or_else(|| {
                            invalid(ctx, *term, "an invoke without the facts of its call")
                        })?;
                        let call = self.rebuild.copy(ctx, *term, Some((invoked, types)), &[])?;
                        let results: Vec<Value> = call.deref(ctx).results().collect();
                        self.rebuild
                            .values
                            .extend(arguments.into_iter().zip(results));
                        call
                    }
                    Link::Try(plan) => {
                        let op = self.structured_try(ctx, *term, plan)?;
                        let after = block.segments.get(position + 1).ok_or_else(|| {
                            invalid(ctx, *term, "a try without its continuation segment")
                        })?;
                        self.rebuild.values.insert(
                            after.block.deref(ctx).get_argument(0),
                            op.deref(ctx).get_result(0),
                        );
                        op
                    }
                    Link::End(End::FallOff) => {
                        let exit = self.rebuild.copy(
                            ctx,
                            *term,
                            Some((CoreOpKind::RegionExit, vec![])),
                            &[],
                        )?;
                        if attr::<ExitAttr>(ctx, exit, &KEY_EXIT).is_none() {
                            return Err(invalid(
                                ctx,
                                *term,
                                "a normal region exit carries its exit kind",
                            ));
                        }
                        exit
                    }
                    Link::End(End::Own) => {
                        let raise = CoreOpKind::of(ctx, *term) == Some(CoreOpKind::Raise);
                        let successors = if raise {
                            Vec::new()
                        } else {
                            term.deref(ctx)
                                .successors()
                                .map(|successor| {
                                    self.rebuild.blocks.get(&successor).copied().ok_or_else(|| {
                                        invalid(ctx, *term, "a branch out of its region")
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?
                        };
                        self.rebuild.copy(ctx, *term, None, &successors)?
                    }
                };
                copy.insert_at_back(*target, ctx);
            }
        }
        targets
            .first()
            .copied()
            .ok_or_else(|| invalid(ctx, self.rebuild.func, "a region without a block"))
    }

    /// The structured try the branch `enter` stands for; the continuation
    /// segment's token becomes the try's result.
    fn structured_try(
        &mut self,
        ctx: &mut Context,
        enter: Ptr<Operation>,
        plan: &TryPlan,
    ) -> Result<Ptr<Operation>, A1Error> {
        let effect = self.rebuild.effect;
        let mut operands = plan
            .cleanup
            .iter()
            .map(|var| self.rebuild.variable(ctx, enter, *var))
            .collect::<Result<Vec<_>, _>>()?;
        let token = self
            .rebuild
            .value(ctx, enter, enter.deref(ctx).get_operand(0))?;
        operands.push(token);
        let op = ops::build(
            ctx,
            CoreOpKind::TryBridge,
            vec![effect],
            operands,
            vec![],
            4,
        );
        let attributes = enter.deref(ctx).attributes.clone();
        op.deref_mut(ctx).attributes = attributes;
        let location = enter.deref(ctx).loc();
        op.deref_mut(ctx).set_loc(location);
        let identity: IdentityAttr = attr(ctx, enter, &KEY_IDENTITY)
            .ok_or_else(|| invalid(ctx, enter, "an operation without identity"))?;
        debug_assert_eq!(
            attr::<TryAttr>(ctx, op, &KEY_TRY).as_ref(),
            Some(&plan.parts)
        );
        let parts = [
            Some(&plan.body),
            plan.handler.as_ref(),
            plan.orelse.as_ref(),
            plan.finalbody.as_ref(),
        ];
        for (index, part) in parts.into_iter().enumerate() {
            let Some(part) = part else { continue };
            let region = op.deref(ctx).get_region(index);
            let entry = BasicBlock::new(ctx, None, vec![effect]);
            entry.insert_at_back(region, ctx);
            let first = self.region(ctx, region, part)?;
            let chain = entry.deref(ctx).get_argument(0);
            let branch = ops::build(ctx, CoreOpKind::Br, vec![], vec![chain], vec![first], 0);
            let (identity, provenance) = entry_annotation(identity.function.clone(), &part.path);
            annotate(ctx, branch, identity, provenance);
            branch.insert_at_back(entry, ctx);
        }
        Ok(op)
    }
}

/// The identity and provenance of the branch out of a region's
/// synthesized entry.
pub fn entry_annotation(function: Text, region: &str) -> (IdentityAttr, ProvenanceAttr) {
    let identity = IdentityAttr {
        function,
        region: region.into(),
        block: 0,
        ordinal: 0,
        role: CoreRole::Entry,
    };
    let primary = IdentityAttr {
        role: CoreRole::Primary,
        ..identity.clone()
    };
    let provenance = ProvenanceAttr {
        span: None,
        origin: None,
        derived_from: vec![primary.key().into()],
        reason: "region-entry".into(),
    };
    (identity, provenance)
}
