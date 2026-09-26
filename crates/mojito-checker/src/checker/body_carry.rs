//! Carrying a body's facts from one checker pass to the next.
//!
//! A body's inference is a function of its syntax, the declarations around
//! it, and the committed transfer and call-through effects it reads. A
//! transfer pass re-checks the same elaborated program, and a discovery
//! round re-checks one that only gained declarations, so a body whose reads
//! are still current would record byte-identical facts. Each body site
//! (a module-level `def`, a struct method) therefore records the log ranges
//! it wrote into every fact store ([`crate::checker::Checker`]'s
//! [`FactMap`]/[`FactSet`]/[`FactVec`] fields), its exact effect reads, and
//! a hash of its syntax; the next pass copies the logged entries instead of
//! inferring when the record is clean and every read still equals the
//! committed entry.
//!
//! The copy is exact: every writer of a fact store is a logging method, and
//! the fresh pass redoes everything outside the body window (declaration
//! phases, signatures, conformance) as it always did. Owner identities stay
//! unique because each pass starts its counter where the previous one
//! stopped. The catalog's template derivation is not consulted for a
//! carried body: a copy is the inference's own result, which derivation is
//! only verified against.

use super::Checker;
use crate::explicit_destroy::CheckedDeletability;
use mojito_ast::ast::{Expr, ExprKind, Method, Stmt, StmtKind, Type};
use mojito_ast::visit::{Visitor, walk_block, walk_expr};
use mojito_checked::checked::{
    CallThroughEffect, DiscoveryResult, ExplicitDestroyInfo, TransferEffect,
};
use mojito_checked::fact_store::{FactMap, FactSet, FactVec};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use mojito_types::origin::OwnerId;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// What one checker pass leaves for the next: its fact stores, one record
/// per body site, and the owner-id watermark.
#[derive(Debug)]
pub struct PassCarry {
    result: DiscoveryResult,
    internal: InternalStores,
    records: HashMap<SourceSpan, BodyRecord>,
    /// The start of every identity range a record holds, sorted, for the
    /// ceiling of a body inferred again inside its own range.
    range_starts: Vec<u32>,
    next_owner: u32,
    /// Whether a later discovery round (another elaboration) is the reader:
    /// a site then also proves its syntax unchanged.
    cross_round: bool,
}

impl PassCarry {
    /// The facts of the pass, for the driver's request queries.
    pub const fn result(&self) -> &DiscoveryResult {
        &self.result
    }

    pub fn into_result(self) -> DiscoveryResult {
        self.result
    }

    /// Hand the carry to the next discovery round: every site must then
    /// also match its syntax hash, and the sites `dirty` names are inferred
    /// again whatever else they match.
    #[must_use]
    pub fn for_next_round(mut self, dirty: &HashSet<SourceSpan>) -> Self {
        self.cross_round = true;
        for (key, record) in &mut self.records {
            record.dirty |= dirty.contains(key);
        }
        self
    }

    /// The body sites of the pass, each with the entries it recorded in
    /// the stores a request is harvested from, for the driver's dirtiness
    /// marking.
    pub fn sites(&self) -> impl Iterator<Item = CarriedSite<'_>> {
        self.records.iter().map(|(key, record)| CarriedSite {
            key,
            carry: self,
            record,
        })
    }

    /// The committed effect maps, the next pass's seeds.
    pub(crate) fn seeds(
        &self,
    ) -> (
        HashMap<String, Vec<TransferEffect>>,
        HashMap<String, Vec<CallThroughEffect>>,
    ) {
        (
            (*self.internal.transfer_effects).clone(),
            (*self.internal.call_through_effects).clone(),
        )
    }

    pub(crate) const fn next_owner(&self) -> u32 {
        self.next_owner
    }

    /// The first identity above `start` that some record holds, or the
    /// pass's watermark: what a body inferred again from `start` must not
    /// reach.
    fn owner_ceiling(&self, start: u32) -> u32 {
        let above = self.range_starts.partition_point(|range| *range <= start);
        self.range_starts
            .get(above)
            .copied()
            .unwrap_or(self.next_owner)
    }
}

/// One body site of a carried pass, seen through its record.
pub struct CarriedSite<'a> {
    key: &'a SourceSpan,
    carry: &'a PassCarry,
    record: &'a BodyRecord,
}

impl CarriedSite<'_> {
    pub const fn key(&self) -> &SourceSpan {
        self.key
    }

    /// The generic-struct applications the body reached.
    pub fn struct_instantiations(&self) -> &[mojito_checked::checked::StructInstantiation] {
        self.carry
            .result
            .struct_instantiations
            .logged(self.record.range(|marks| marks.struct_instantiations))
    }

    pub fn hash_leaf_types(&self) -> &[mojito_types::types::Ty] {
        self.carry
            .result
            .hash_leaf_types
            .logged(self.record.range(|marks| marks.hash_leaf_types))
    }

    /// The generic instantiations the body recorded, by callee.
    pub fn instantiated_callees(&self) -> impl Iterator<Item = &str> {
        let result = &self.carry.result;
        result
            .generic_instantiations
            .logged(self.record.range(|marks| marks.generic_instantiations))
            .iter()
            .filter_map(|span| result.generic_instantiations.get(span))
            .map(|instantiation| instantiation.callee.as_str())
    }

    /// The generic method instantiations the body recorded, by owner and
    /// method.
    pub fn instantiated_methods(&self) -> impl Iterator<Item = (&str, &str)> {
        let result = &self.carry.result;
        result
            .method_instantiations
            .logged(self.record.range(|marks| marks.method_instantiations))
            .iter()
            .filter_map(|span| result.method_instantiations.get(span))
            .map(|instantiation| (instantiation.owner.as_str(), instantiation.method.as_str()))
    }

    /// Every type the body recorded for an expression, a place, or a
    /// binding.
    pub fn recorded_types(&self) -> impl Iterator<Item = &mojito_types::types::Ty> {
        let result = &self.carry.result;
        let expressions = result
            .expression_types
            .logged(self.record.range(|marks| marks.expression_types))
            .iter()
            .filter_map(|span| result.expression_types.get(span));
        let places = result
            .expression_place_types
            .logged(self.record.range(|marks| marks.expression_place_types))
            .iter()
            .filter_map(|span| result.expression_place_types.get(span));
        let bindings = result
            .binding_types
            .logged(self.record.range(|marks| marks.binding_types))
            .iter()
            .filter_map(|span| result.binding_types.get(span));
        expressions.chain(places).chain(bindings)
    }
}

/// What one body site recorded in one pass.
#[derive(Debug)]
pub struct BodyRecord {
    display: String,
    start: StoreMarks,
    end: StoreMarks,
    /// Each callee whose effect summary the body read, with the entry as
    /// read; the copy stands only while every entry is still that.
    reads: Vec<(String, ObservedEffects)>,
    /// The binding identities the body allocated.
    owners: Range<u32>,
    /// The fresh block it spilled into when inferred again inside a range
    /// it outgrew.
    spill: Option<Range<u32>>,
    syntax: u64,
    /// Set by the driver when a request the body recorded was served since:
    /// its facts would now differ.
    dirty: bool,
}

impl BodyRecord {
    fn range(&self, mark: impl Fn(&StoreMarks) -> usize) -> Range<usize> {
        mark(&self.start)..mark(&self.end)
    }
}

/// One effect entry as a body read it: absent and empty are the same read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedEffects {
    Transfers(Vec<TransferEffect>),
    CallThroughs(Vec<CallThroughEffect>),
}

/// Why a site was not carried, for the timing notes.
#[derive(Debug, Clone, Copy)]
enum CarryRefusal {
    NoPrevious,
    NoRecord,
    Dirty,
    SyntaxChanged,
    StaleRead,
}

impl CarryRefusal {
    const fn describe(self) -> &'static str {
        match self {
            Self::NoPrevious => "no previous pass",
            Self::NoRecord => "no record in the previous pass",
            Self::Dirty => "a request it recorded was served since",
            Self::SyntaxChanged => "its syntax changed",
            Self::StaleRead => "an effect entry it read changed",
        }
    }
}

/// The fact stores a body writes that the checked handoff does not carry,
/// moved out of the checker with the rest of its facts.
#[derive(Debug)]
pub struct InternalStores {
    transfer_effects: FactMap<String, Vec<TransferEffect>>,
    call_through_effects: FactMap<String, Vec<CallThroughEffect>>,
    transferred_origins: FactMap<OwnerId, Vec<mojito_types::origin::Origin>>,
    construction_immutable_binders: FactMap<SourceSpan, Vec<super::ImmutableOriginBinder>>,
    call_result_origins: FactMap<SourceSpan, Vec<super::CallResultOrigin>>,
    tuple_unpack_sources: FactMap<SourceSpan, super::TupleUnpackSource>,
    comprehension_iterables: FactMap<SourceSpan, Vec<SourceSpan>>,
    nested_def_params: FactMap<SourceSpan, Vec<OwnerId>>,
    view_result_interiors: FactMap<SourceSpan, Vec<String>>,
    call_parameters: FactMap<SourceSpan, Vec<super::CallParameter>>,
    with_desugars: FactMap<SourceSpan, super::with_stmt::WithDesugar>,
    rebind_assertions: FactMap<SourceSpan, mojito_checked::templates::RebindAssertion>,
    hash_leaf_demands: FactVec<mojito_types::types::Ty>,
    copyable_reference_result_reads: FactSet<SourceSpan>,
    discarded_reference_results: FactSet<SourceSpan>,
    borrowed_reference_receivers: FactSet<SourceSpan>,
    unconsumed_temporaries: FactSet<SourceSpan>,
    linear_temporaries: FactSet<SourceSpan>,
    no_verdict_bodies: FactSet<SourceSpan>,
    deletability: CheckedDeletability,
}

/// Every store a body site is measured and copied over: where the carry
/// holds it (`result` is the checked handoff, `internal` the rest), how it
/// logs (`map`, `set`, `vec`), its field in the carry, and its field in the
/// checker. The deletability sets are handled by hand beside them.
macro_rules! carried_stores {
    ($apply:ident) => {
        $apply! {
            result map overload_targets = overload_targets;
            result map contextual_bases = contextual_bases;
            result map generic_instantiations = generic_instantiations;
            result map method_instantiations = method_instantiations;
            result vec struct_instantiations = struct_instantiations;
            result vec hash_leaf_types = hash_leaf_types;
            result map call_transfers = call_transfers;
            result map implicit_conversions = implicit_conversions;
            result map implicit_conversion_types = implicit_conversion_types;
            result map conversion_source_borrows = conversion_source_borrows;
            result map conversion_raises = implicit_conversion_raises;
            result map checked_types = declaration_types;
            result map generic_parameters = generic_parameters;
            result map expression_types = expression_types;
            result map expression_bindings = expression_bindings;
            result map statement_bindings = statement_bindings;
            result map declaration_captures = declaration_captures;
            result map comprehension_bindings = comprehension_bindings;
            result map expression_place_types = expression_place_types;
            result map binding_types = binding_types;
            result map expression_effects = expression_effects;
            result map selected_calls = selected_calls;
            result map subscript_descriptors = subscript_descriptors;
            result map iteration_protocols = iteration_protocols;
            result map simd_constructions = simd_constructions;
            result map operation_adjustments = operation_adjustments;
            result map parameterized_method_calls = parameterized_method_calls;
            result map tuple_unpack_plans = tuple_unpack_plans;
            result map interior_references = interior_references;
            result map interior_invalidations = interior_invalidations;
            result set explicit_destroy_calls = explicit_destroy_calls;
            result map reference_value_uses = reference_value_uses;
            result set copy_place_value_uses = copy_place_value_uses;
            result set call_place_uses = call_place_uses;
            result set borrowed_read_call_places = borrowed_read_call_places;
            result set read_temporary_arguments = read_temporary_arguments;
            result set implicitly_copied_consuming_receivers = implicitly_copied_consuming_receivers;
            result set truthiness_conditions = truthiness_conditions;
            result map declaration_effects = declaration_effects;
            internal map transfer_effects = transfer_effects;
            internal map call_through_effects = call_through_effects;
            internal map transferred_origins = transferred_origins;
            internal map construction_immutable_binders = construction_immutable_binders;
            internal map call_result_origins = call_result_origins;
            internal map tuple_unpack_sources = tuple_unpack_sources;
            internal map comprehension_iterables = comprehension_iterables;
            internal map nested_def_params = nested_def_params;
            internal map view_result_interiors = view_result_interiors;
            internal map call_parameters = call_parameters;
            internal map with_desugars = with_desugars;
            internal map rebind_assertions = rebind_assertions;
            internal vec hash_leaf_demands = hash_leaf_demands;
            internal set copyable_reference_result_reads = copyable_reference_result_reads;
            internal set discarded_reference_results = discarded_reference_results;
            internal set borrowed_reference_receivers = borrowed_reference_receivers;
            internal set unconsumed_temporaries = unconsumed_temporaries;
            internal set linear_temporaries = linear_temporaries;
            internal set no_verdict_bodies = no_verdict_bodies;
        }
    };
}

macro_rules! define_marks {
    ($($loc:ident $kind:ident $carry:ident = $field:ident;)*) => {
        /// The log position of every store at one moment.
        #[derive(Debug, Clone, Copy)]
        pub struct StoreMarks {
            $($carry: usize,)*
            deletable_declarations: usize,
            deletable_bindings: usize,
            linear_declarations: usize,
            linear_bindings: usize,
            /// The identity cursor, its ceiling, and the fresh cursor.
            owner: u32,
            ceiling: Option<u32>,
            fresh: u32,
        }
    };
}
carried_stores!(define_marks);

macro_rules! copy_store {
    (map $keys:expr, $from:expr, $to:expr) => {
        copy_map($keys, $from, $to)
    };
    (set $keys:expr, $from:expr, $to:expr) => {
        copy_set($keys, $from, $to)
    };
    (vec $keys:expr, $from:expr, $to:expr) => {
        $to.extend($keys.iter().cloned())
    };
}

/// Where a store lives in the carry.
macro_rules! carried {
    ($carry:expr, result, $name:ident) => {
        $carry.result.$name
    };
    ($carry:expr, internal, $name:ident) => {
        $carry.internal.$name
    };
}

/// The checker's own store, borrowed mutably; `no_verdict_bodies` is the
/// one plain field.
macro_rules! own_store {
    ($checker:expr, no_verdict_bodies) => {
        $checker.no_verdict_bodies
    };
    ($checker:expr, $field:ident) => {
        *$checker.$field.borrow_mut()
    };
}

macro_rules! own_mark {
    ($checker:expr, no_verdict_bodies) => {
        $checker.no_verdict_bodies.mark()
    };
    ($checker:expr, $field:ident) => {
        $checker.$field.borrow().mark()
    };
}

macro_rules! define_carry_ops {
    ($($loc:ident $kind:ident $carry:ident = $field:ident;)*) => {
        impl Checker {
            fn store_marks(&self) -> StoreMarks {
                let deletability = self.explicit_destroy_deletability.borrow();
                StoreMarks {
                    $($carry: own_mark!(self, $field),)*
                    deletable_declarations: deletability.declarations.mark(),
                    deletable_bindings: deletability.bindings.mark(),
                    linear_declarations: deletability.linear_declarations.mark(),
                    linear_bindings: deletability.linear_bindings.mark(),
                    owner: self.next_owner.get(),
                    ceiling: self.owner_ceiling.get(),
                    fresh: self.fresh_owner_cursor.get(),
                }
            }

            /// Copy every entry `record` logged in `previous` into this
            /// checker's stores.
            fn copy_recorded_facts(&mut self, previous: &PassCarry, record: &BodyRecord) {
                $(
                    copy_store!(
                        $kind
                        carried!(previous, $loc, $carry)
                            .logged(record.range(|marks| marks.$carry)),
                        &carried!(previous, $loc, $carry),
                        &mut own_store!(self, $field)
                    );
                )*
                let from = &previous.internal.deletability;
                let mut deletability = self.explicit_destroy_deletability.borrow_mut();
                copy_set(
                    from.declarations
                        .logged(record.range(|marks| marks.deletable_declarations)),
                    &from.declarations,
                    &mut deletability.declarations,
                );
                copy_set(
                    from.bindings
                        .logged(record.range(|marks| marks.deletable_bindings)),
                    &from.bindings,
                    &mut deletability.bindings,
                );
                copy_set(
                    from.linear_declarations
                        .logged(record.range(|marks| marks.linear_declarations)),
                    &from.linear_declarations,
                    &mut deletability.linear_declarations,
                );
                copy_set(
                    from.linear_bindings
                        .logged(record.range(|marks| marks.linear_bindings)),
                    &from.linear_bindings,
                    &mut deletability.linear_bindings,
                );
            }

            /// Move this pass's facts into a carry, with `statements` as
            /// the checked tree and `explicit_destroy_types` as the
            /// destruction facts the pass established (empty for a pass
            /// that stopped at the transfer fixpoint).
            pub(crate) fn into_carry(
                self,
                statements: Vec<Stmt>,
                explicit_destroy_types: HashMap<String, ExplicitDestroyInfo>,
            ) -> PassCarry {
                let result = DiscoveryResult {
                    statements,
                    explicit_destroy_types,
                    ..carried_result(&self)
                };
                let internal = InternalStores {
                    transfer_effects: self.transfer_effects.into_inner(),
                    call_through_effects: self.call_through_effects.into_inner(),
                    transferred_origins: self.transferred_origins.into_inner(),
                    construction_immutable_binders: self.construction_immutable_binders.into_inner(),
                    call_result_origins: self.call_result_origins.into_inner(),
                    tuple_unpack_sources: self.tuple_unpack_sources.into_inner(),
                    comprehension_iterables: self.comprehension_iterables.into_inner(),
                    nested_def_params: self.nested_def_params.into_inner(),
                    view_result_interiors: self.view_result_interiors.into_inner(),
                    call_parameters: self.call_parameters.into_inner(),
                    with_desugars: self.with_desugars.into_inner(),
                    rebind_assertions: self.rebind_assertions.into_inner(),
                    hash_leaf_demands: self.hash_leaf_demands.into_inner(),
                    copyable_reference_result_reads: self.copyable_reference_result_reads.into_inner(),
                    discarded_reference_results: self.discarded_reference_results.into_inner(),
                    borrowed_reference_receivers: self.borrowed_reference_receivers.into_inner(),
                    unconsumed_temporaries: self.unconsumed_temporaries.into_inner(),
                    linear_temporaries: self.linear_temporaries.into_inner(),
                    no_verdict_bodies: self.no_verdict_bodies,
                    deletability: self.explicit_destroy_deletability.into_inner(),
                };
                let records = self.body_records.into_inner();
                let mut range_starts: Vec<u32> = records
                    .values()
                    .flat_map(|record| {
                        std::iter::once(record.owners.start)
                            .chain(record.spill.as_ref().map(|spill| spill.start))
                    })
                    .collect();
                range_starts.sort_unstable();
                PassCarry {
                    result,
                    internal,
                    records,
                    range_starts,
                    next_owner: self.fresh_owner_cursor.get().max(self.next_owner.get()),
                    cross_round: false,
                }
            }
        }
    };
}
carried_stores!(define_carry_ops);

impl Checker {
    /// Record that the body being checked read `callee`'s effect entry.
    pub(super) fn note_body_effect_read(&self, callee: &str, observed: ObservedEffects) {
        if let Some(reads) = self.site_reads.borrow_mut().as_mut() {
            reads.push((callee.to_string(), observed));
        }
    }

    /// Serve the body site `key` from the previous pass, if its record is
    /// clean and every effect entry it read is still what it read. The
    /// facts are copied and a fresh record is left for the next pass.
    pub(super) fn carry_body(&mut self, key: &SourceSpan, display: &str, syntax: u64) -> bool {
        if let Some(refusal) = self.carry_refusal(key, syntax).err() {
            if !matches!(refusal, CarryRefusal::NoPrevious) {
                timing::count("body_facts.carry_refused", 1);
                timing::note("body_facts.carry_refused", || {
                    format!("{display}: {}", refusal.describe())
                });
            }
            return false;
        }
        let previous = self
            .previous
            .take()
            .expect("carry_refusal proved a previous pass");
        let record = previous
            .records
            .get(key)
            .expect("carry_refusal proved a record");
        let start = self.enter_body_site(key);
        self.copy_recorded_facts(&previous, record);
        for (callee, observed) in &record.reads {
            match observed {
                ObservedEffects::Transfers(effects) => {
                    self.effect_observations
                        .borrow_mut()
                        .entry(callee.clone())
                        .or_insert_with(|| effects.clone());
                }
                ObservedEffects::CallThroughs(throughs) => {
                    self.call_through_observations
                        .borrow_mut()
                        .entry(callee.clone())
                        .or_insert_with(|| throughs.clone());
                }
            }
        }
        *self.site_reads.borrow_mut() = Some(record.reads.clone());
        let (owners, spill) = (record.owners.clone(), record.spill.clone());
        self.leave_body_site(key.clone(), display, &start, syntax);
        let mut records = self.body_records.borrow_mut();
        let recorded = records.get_mut(key).expect("the site was just recorded");
        recorded.owners = owners;
        recorded.spill = spill;
        drop(records);
        self.previous = Some(previous);
        timing::count("body_facts.carried", 1);
        timing::note("body_facts.carried", || display.to_string());
        self.template_catalog
            .borrow_mut()
            .stats_mut()
            .carried
            .push(display.to_string());
        true
    }

    /// Open a body site: the store marks and reads frame the record needs.
    /// A site the previous pass recorded is inferred inside the identity
    /// range it had, so an unchanged prefix of its bindings keeps its
    /// identities (a request's type may name one through an origin).
    pub(super) fn enter_body_site(&self, key: &SourceSpan) -> StoreMarks {
        *self.site_reads.borrow_mut() = Some(Vec::new());
        self.owner_range_split.set(false);
        if let Some(previous) = &self.previous
            && let Some(record) = previous.records.get(key)
        {
            self.next_owner.set(record.owners.start);
            self.owner_ceiling
                .set(Some(previous.owner_ceiling(record.owners.start)));
        }
        self.store_marks()
    }

    /// Close the body site opened at `start`, recording what it wrote, and
    /// return the cursor to the fresh region.
    pub(super) fn leave_body_site(
        &self,
        key: SourceSpan,
        display: &str,
        start: &StoreMarks,
        syntax: u64,
    ) {
        let reads = self.site_reads.borrow_mut().take().unwrap_or_default();
        let (owners, spill) = if self.owner_ceiling.take().is_none() && self.owner_range_split.get()
        {
            (
                start.owner..start.ceiling.unwrap_or(start.owner),
                Some(start.fresh..self.next_owner.get()),
            )
        } else {
            (start.owner..self.next_owner.get(), None)
        };
        self.owner_range_split.set(false);
        self.next_owner.set(self.fresh_owner_cursor.get());
        self.body_records.borrow_mut().insert(
            key,
            BodyRecord {
                display: display.to_string(),
                start: *start,
                end: self.store_marks(),
                reads,
                owners,
                spill,
                syntax,
                dirty: false,
            },
        );
    }

    /// Whether the checker carries bodies at all this pass.
    pub(super) fn carries_bodies(&self) -> bool {
        !self.source_validation && self.template_catalog.borrow().body_fact_reuse()
    }

    fn carry_refusal(&self, key: &SourceSpan, syntax: u64) -> Result<(), CarryRefusal> {
        let previous = self.previous.as_ref().ok_or(CarryRefusal::NoPrevious)?;
        let record = previous.records.get(key).ok_or(CarryRefusal::NoRecord)?;
        if record.dirty {
            return Err(CarryRefusal::Dirty);
        }
        if previous.cross_round && record.syntax != syntax {
            return Err(CarryRefusal::SyntaxChanged);
        }
        let transfers = self.transfer_effects.borrow();
        let throughs = self.call_through_effects.borrow();
        let current = record
            .reads
            .iter()
            .all(|(callee, observed)| match observed {
                ObservedEffects::Transfers(effects) => transfers
                    .get(callee)
                    .map_or(effects.is_empty(), |now| now == effects),
                ObservedEffects::CallThroughs(effects) => throughs
                    .get(callee)
                    .map_or(effects.is_empty(), |now| now == effects),
            });
        if !current {
            timing::note("body_facts.stale_read", || {
                format!(
                    "{}: {}",
                    record.display,
                    stale_reads(&record.reads, &transfers, &throughs)
                )
            });
            return Err(CarryRefusal::StaleRead);
        }
        Ok(())
    }
}

/// A hash of a module-level `def` for a record: its syntax identities,
/// node kinds, the names it calls, its parameter arguments, and its
/// annotations, which is what an elaboration rewrites in a body it kept.
pub(super) fn def_syntax_hash(stmt: &Stmt) -> u64 {
    let mut hasher = SyntaxHasher::default();
    if let StmtKind::Def {
        name,
        type_params,
        params,
        ret,
        raises,
        raises_type,
        decorators,
        where_clauses,
        ..
    } = &stmt.kind
    {
        name.hash(&mut hasher.state);
        format!("{type_params:?}{params:?}{ret:?}{raises:?}{raises_type:?}{decorators:?}")
            .hash(&mut hasher.state);
        for clause in where_clauses {
            walk_expr(&mut hasher, clause);
        }
    }
    mojito_ast::visit::walk_stmt(&mut hasher, stmt);
    hasher.state.finish()
}

/// [`def_syntax_hash`] for a struct method.
pub(super) fn method_syntax_hash(m: &Method) -> u64 {
    let mut hasher = SyntaxHasher::default();
    m.name.hash(&mut hasher.state);
    format!(
        "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
        m.type_params,
        m.has_self,
        m.self_convention,
        m.self_origin,
        m.decorators,
        m.params,
        m.raises,
        m.raises_type,
        m.ret
    )
    .hash(&mut hasher.state);
    format!("{:?}", m.self_ty).hash(&mut hasher.state);
    for clause in &m.where_clauses {
        walk_expr(&mut hasher, clause);
    }
    walk_block(&mut hasher, &m.body);
    hasher.state.finish()
}

#[derive(Default)]
struct SyntaxHasher {
    state: std::hash::DefaultHasher,
}

impl Visitor for SyntaxHasher {
    fn visit_stmt(&mut self, statement: &Stmt) {
        statement.syntax_id.hash(&mut self.state);
        std::mem::discriminant(&statement.kind).hash(&mut self.state);
        match &statement.kind {
            StmtKind::VarDecl { name, .. }
            | StmtKind::RefDecl { name, .. }
            | StmtKind::Assign { name, .. } => name.hash(&mut self.state),
            StmtKind::Def {
                name,
                type_params,
                params,
                ret,
                ..
            } => {
                name.hash(&mut self.state);
                format!("{type_params:?}{params:?}{ret:?}").hash(&mut self.state);
            }
            _ => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        expr.syntax_id.hash(&mut self.state);
        std::mem::discriminant(&expr.kind).hash(&mut self.state);
        match &expr.kind {
            ExprKind::Identifier(name) => name.hash(&mut self.state),
            ExprKind::Call {
                name, param_args, ..
            } => {
                name.hash(&mut self.state);
                format!("{param_args:?}").hash(&mut self.state);
            }
            ExprKind::MethodCall { method, .. } => method.hash(&mut self.state),
            ExprKind::Member { field, .. } => field.hash(&mut self.state),
            ExprKind::Int(value) => format!("{value:?}").hash(&mut self.state),
            ExprKind::Float(value) => format!("{value:?}").hash(&mut self.state),
            ExprKind::Bool(value) => value.hash(&mut self.state),
            ExprKind::Str(value) => value.hash(&mut self.state),
            _ => {}
        }
    }

    fn visit_type(&mut self, ty: &Type) {
        format!("{ty:?}").hash(&mut self.state);
    }
}

/// The checked-handoff fields of a checker, moved out; the two the carry
/// sets itself are defaulted here and overwritten by the caller.
fn carried_result(checker: &Checker) -> DiscoveryResult {
    DiscoveryResult {
        statements: Vec::new(),
        explicit_destroy_types: HashMap::new(),
        overload_targets: checker.overload_targets.take(),
        contextual_bases: checker.contextual_bases.take(),
        generic_instantiations: checker.generic_instantiations.take(),
        method_instantiations: checker.method_instantiations.take(),
        struct_instantiations: checker.struct_instantiations.take(),
        hash_leaf_types: checker.hash_leaf_types.take(),
        call_transfers: checker.call_transfers.take(),
        implicit_conversions: checker.implicit_conversions.take(),
        implicit_conversion_types: checker.implicit_conversion_types.take(),
        conversion_source_borrows: checker.conversion_source_borrows.take(),
        conversion_raises: checker.implicit_conversion_raises.take(),
        checked_types: checker.declaration_types.take(),
        generic_parameters: checker.generic_parameters.take(),
        expression_types: checker.expression_types.take(),
        expression_bindings: checker.expression_bindings.take(),
        statement_bindings: checker.statement_bindings.take(),
        declaration_captures: checker.declaration_captures.take(),
        comprehension_bindings: checker.comprehension_bindings.take(),
        expression_place_types: checker.expression_place_types.take(),
        binding_types: checker.binding_types.take(),
        expression_effects: checker.expression_effects.take(),
        selected_calls: checker.selected_calls.take(),
        subscript_descriptors: checker.subscript_descriptors.take(),
        iteration_protocols: checker.iteration_protocols.take(),
        simd_constructions: checker.simd_constructions.take(),
        operation_adjustments: checker.operation_adjustments.take(),
        parameterized_method_calls: checker.parameterized_method_calls.take(),
        tuple_unpack_plans: checker.tuple_unpack_plans.take(),
        interior_references: checker.interior_references.take(),
        interior_invalidations: checker.interior_invalidations.take(),
        explicit_destroy_calls: checker.explicit_destroy_calls.take(),
        reference_value_uses: checker.reference_value_uses.take(),
        copy_place_value_uses: checker.copy_place_value_uses.take(),
        call_place_uses: checker.call_place_uses.take(),
        borrowed_read_call_places: checker.borrowed_read_call_places.take(),
        read_temporary_arguments: checker.read_temporary_arguments.take(),
        implicitly_copied_consuming_receivers: checker.implicitly_copied_consuming_receivers.take(),
        truthiness_conditions: checker.truthiness_conditions.take(),
        declaration_effects: checker.declaration_effects.take(),
    }
}

fn copy_map<K: Eq + Hash + Clone, V: Clone>(
    keys: &[K],
    from: &HashMap<K, V>,
    to: &mut FactMap<K, V>,
) {
    for key in keys {
        if let Some(value) = from.get(key) {
            to.insert(key.clone(), value.clone());
        }
    }
}

fn copy_set<K: Eq + Hash + Clone>(keys: &[K], from: &HashSet<K>, to: &mut FactSet<K>) {
    for key in keys {
        if from.contains(key) {
            to.insert(key.clone());
        }
    }
}

fn stale_reads(
    reads: &[(String, ObservedEffects)],
    transfers: &FactMap<String, Vec<TransferEffect>>,
    throughs: &FactMap<String, Vec<CallThroughEffect>>,
) -> String {
    reads
        .iter()
        .filter(|(callee, observed)| match observed {
            ObservedEffects::Transfers(effects) => !transfers
                .get(callee)
                .map_or(effects.is_empty(), |now| now == effects),
            ObservedEffects::CallThroughs(effects) => !throughs
                .get(callee)
                .map_or(effects.is_empty(), |now| now == effects),
        })
        .map(|(callee, _)| callee.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
