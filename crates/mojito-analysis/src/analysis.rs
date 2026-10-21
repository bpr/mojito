//! Stage 6: ownership and persistent-loan analysis over MIR.
//!
//! Mojo's move semantics: transferring a value with `^` (`y = x^`, `take(x^)`)
//! leaves the source **uninitialized**, so using it again is an error. This pass
//! walks each `MirFunction`'s structured regions, as upstream's
//! `CheckLifetimes` walks `hlcf`, tracking per variable whether it is
//! `Owned`, `Moved`, or — where control-flow paths disagree — `MaybeMoved`,
//! and per reference which interior-origin generations it carries; the loan
//! check reads the same regions. A use of a `Moved` variable is a
//! **use-after-move**; a use of a `MaybeMoved` one is a **conditional move**
//! (transferred on some paths but not others). Diagnostics carry the source
//! [`Span`](mojito_mir::mir::Span) of the offending use, recovered from the
//! MIR `SpanTable`.
//!
//! This is a distinct compiler stage after MIR lowering. The production
//! [`Compiler`](crate::compiler::Compiler) always runs it before drop elaboration
//! and VM execution. Backward liveness then builds on this move/init foundation
//! to insert ASAP drops in the same structured regions, before
//! `lower-control-flow` lowers them to blocks.

pub use canonicalize::{CANONICALIZE, canonicalize};
use copy_elision::elide_dying_copies;
pub use cse::{CSE, cse};
pub use dead_symbols::{
    ELIMINATE_DEAD_CONCRETE_SYMBOLS, ELIMINATE_DEAD_SYMBOLS, eliminate_dead_concrete_symbols,
    eliminate_dead_symbols,
};
pub use destructor_calls::name_destructors;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use drops::*;
pub use duplicate_functions::{ELIMINATE_DUPLICATE_FUNCTIONS, eliminate_duplicate_functions};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use field_drops::*;
pub use inline_parametric::{INLINE_PARAMETRIC, inline_parametric, is_inlined_before_elaboration};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use interior::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use loans::*;
pub use lower_lit::{LOWER_LIT, lower_lit};
pub use mem2reg::{LOWER_ARG_CONVENTIONS, MEM_2_REG, lower_arg_conventions, mem_2_reg};
use mojito_common::error::OwnershipError;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    CleanupDrop, MirBlock, MirCaptureMode, MirDeclarations, MirFunction, MirFunctionDeclaration,
    MirInstr, MirInteriorOrigin, MirIntrinsicSubscript, MirPlace, MirProgram, MirSubscriptArg,
    MirTerm, Proj, Reg, binder_scope, cleanup_drops, lower_copies_and_moves,
    materialized_parameter_slots, terminator_targets,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use moves::*;
pub use opt::{OptError, opt};
pub use post_elaboration::{
    ELABORATED_SYMBOL_PASSES, POST_ELABORATION_PIPELINE, eliminate_elaborated_symbols,
    post_elaborate_program,
};
pub use pre_elaboration::{
    BodyPass, CallGraphPass, Callees, ELABORATE_DROPS, LOWER_CONTROL_FLOW, PIPELINE, Pass,
    ProgramPass, finish_package_body, finish_package_body_without_pipeline, pass_named, pass_names,
    pre_elaborate_program, run_pipeline,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use region_walk::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use register_loans::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use scan::*;
pub use sccp::{SCCP, sccp};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use store_drops::*;
pub use unused_params::{REMOVE_UNUSED_PARAMS, remove_unused_params};

pub fn check_ownership_checked(
    program: &mojito_checked::checked::CheckedProgram,
) -> Result<(), OwnershipError> {
    check_ownership_program(&mojito_mir::mir::lower_checked_program_structured(program))
}

/// Run the ownership analysis over a lowered program.
///
/// This is the standalone-MIR core the pipeline composes with `mir::verify`
/// so production MIR is fully verified before execution; it runs on `hlcf`
/// regions as upstream's `CheckLifetimes` does, before `LowerControlFlow`.
///
/// Per function, the lifetime scan (moves and interior origins in one
/// walk) and the loan check read the structured body, which stays
/// structured for drop elaboration. A body already lowered (MIR text) is
/// scanned as blocks.
///
/// An imported package's bodies were analyzed when it was built; a call
/// into one reads its reference-parameter mask from the package's function
/// table.
pub fn check_ownership_program(prog: &MirProgram) -> Result<(), OwnershipError> {
    let MirProgram {
        functions,
        declarations,
        ..
    } = prog;
    let masks: Vec<(String, Vec<bool>)> = functions
        .iter()
        .map(|(name, function)| (name.clone(), function.ref_params.clone()))
        .collect();
    let callees = CalleeRefParams {
        ref_params: masks
            .iter()
            .map(|(name, mask)| (name.as_str(), mask.as_slice()))
            .collect(),
        param_writes: declarations
            .functions
            .iter()
            .map(|declaration| {
                (
                    declaration.lowered_name.as_str(),
                    declaration.param_writes.as_slice(),
                )
            })
            .collect(),
        structs: declarations
            .structs
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect(),
        imported: declarations.imported.as_ref(),
    };
    timing::count("functions", functions.len() as u64);
    for (_name, f) in functions {
        {
            let _lifetimes = timing::span("lifetimes");
            analyze_lifetimes(f)?;
        }
        {
            let _loans = timing::span("loans");
            analyze_loans(f, &callees)?;
        }
    }
    Ok(())
}

/// The program-wide callee contracts the loan analysis classifies retained
/// call places against.
///
/// A retained place is an exclusive write only where the callee's declaration
/// says the parameter writes through it (`param_writes`: `mut`, or `ref` under
/// a statically mutable origin); a place lent to any other slot — a read
/// convention, or an immutable, parametric, or bare `ref` (a borrowing-view
/// constructor, `Named`) — is a shared read. A callee without a declaration
/// falls back to its function's `ref_params` mask (receiver at slot zero), and
/// an unknown callee stays exclusive. `structs` resolves a single-`__init__`
/// construction, which calls the bare struct name, to its `<name>.__init__`
/// declaration. `ref_params`, `param_writes`, and `structs` cover the
/// program's own functions and declarations; a name they miss is looked up
/// in the package it imports.
#[derive(Default)]
pub struct CalleeRefParams<'a> {
    pub ref_params: HashMap<&'a str, &'a [bool]>,
    pub param_writes: HashMap<&'a str, &'a [bool]>,
    pub structs: HashSet<&'a str>,
    pub imported: Option<&'a mojito_mir::mir::link::ImportedDeclarations>,
}

impl CalleeRefParams<'_> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The access a retained place at positional argument `argument` of
    /// `callee` performs; `receiver` says the callee is a method whose
    /// function-level mask carries `self` at slot zero.
    fn retained_access(&self, callee: &str, argument: usize, receiver: bool) -> LoanAccess {
        let init_name;
        let imported = self.imported;
        let declared = if self.structs.contains(callee)
            || imported.is_some_and(|imported| imported.structure(callee).is_some())
        {
            init_name = format!("{callee}.__init__");
            init_name.as_str()
        } else {
            callee
        };
        let mask = self.param_writes.get(declared).copied().or_else(|| {
            imported?
                .function(declared)
                .map(|declaration| declaration.param_writes.as_slice())
        });
        if let Some(mask) = mask {
            return match mask.get(argument) {
                Some(false) => LoanAccess::Read,
                _ => LoanAccess::Write,
            };
        }
        let slot = if receiver { argument + 1 } else { argument };
        let reference = self.ref_params.get(callee).map_or_else(
            || imported?.package().ref_param(callee, slot),
            |mask| mask.get(slot).copied(),
        );
        match reference {
            Some(false) => LoanAccess::Read,
            _ => LoanAccess::Write,
        }
    }
}

// --- Liveness + ASAP drop elaboration ---------------------------------------

/// Elaborate ASAP destruction across a whole program: before each store that
/// overwrites an initialized droppable field, splice a `DropPlace`; after each
/// variable's last use, splice a `DropVar`.
///
/// Each body stays structured, as upstream's `DestructorInsertion` leaves
/// `hlcf`: the seams lower it (`lower-control-flow`) after `lower-lit`.
///
/// Applied by the VM before execution so a struct's `__deinit__` fires at the
/// value's last use (not at scope end), and a replaced field's at the store.
pub fn elaborate_drops_program(prog: MirProgram) -> MirProgram {
    let functions = prog
        .functions
        .into_iter()
        .map(|(name, f)| {
            // Module-scope (`__toplevel__`) variables live until program end, so
            // they are not ASAP-dropped — that also keeps their final values
            // intact for the CLI/`bindings()` global dump (a `DropVar` would
            // clear the slot).
            let elaborated = if name == "__toplevel__" {
                let mut f = f;
                elaborate_store_drops(&mut f, &name, &prog.declarations);
                name_destructors(&mut f, &prog.declarations);
                lower_copies_and_moves(&name, &mut f, &|name| prog.declarations.function(name));
                f
            } else {
                elaborate_drops_function(&name, f, &prog.declarations)
            };
            (name, elaborated)
        })
        .collect();
    MirProgram {
        functions,
        declarations: prog.declarations,
        invariant_errors: prog.invariant_errors,
    }
}

/// Erase the lifetime-only markers from every function of `program`,
/// `__toplevel__` included, once drop elaboration has read them: [`lower_lit`]
/// over each body.
pub fn lower_lit_program(program: &mut MirProgram) {
    for (name, function) in &mut program.functions {
        lower_lit(name, function, &program.declarations);
    }
}

/// [`elaborate_drops_program`] for the one function `name` of a program
/// whose declarations are `declarations`: an imported package's body is
/// drop-elaborated as it is materialized.
pub fn elaborate_drops_function(
    name: &str,
    mut function: MirFunction,
    declarations: &MirDeclarations,
) -> MirFunction {
    elaborate_store_drops(&mut function, name, declarations);
    let scope = binder_scope(name, |name| declarations.function(name));
    let slots = materialized_parameter_slots(&function, &scope);
    let mut elaborated = elaborate_drops(function, &slots, declarations);
    elide_dying_copies(&mut elaborated, declarations);
    refine_deinit_fields(&mut elaborated, declarations);
    name_destructors(&mut elaborated, declarations);
    mojito_mir::mir::name_constructors(&mut elaborated, declarations);
    lower_copies_and_moves(name, &mut elaborated, &|name| declarations.function(name));
    elaborated
}

mod canonicalize;
mod copy_elision;
mod cse;
mod dead_symbols;
mod destructor_calls;
mod drops;
mod duplicate_functions;
mod field_drops;
mod fold;
mod inline_parametric;
mod interior;
mod loans;
mod lower_lit;
mod mem2reg;
mod moves;
mod opt;
mod post_elaboration;
mod pre_elaboration;
mod region_walk;
mod register_loans;
mod scan;
mod sccp;
mod store_drops;
mod unused_params;

#[cfg(test)]
mod constant_tuple_place_tests {
    use super::*;
    use mojito_mir::mir::SpanTable;

    fn place(projection: Proj) -> MirPlace {
        let mut place = MirPlace::root(0, None);
        place.proj.push(projection);
        place
    }

    #[test]
    fn constant_tuple_indices_are_disjoint_but_dynamic_indices_overlap_them() {
        let first = place(Proj::ConstIndex(0));
        let second = place(Proj::ConstIndex(1));
        let dynamic = place(Proj::Index(Reg(0)));
        assert!(!mir_places_overlap(&first, &second));
        assert!(mir_places_overlap(&first, &dynamic));
        assert!(mir_places_overlap(&dynamic, &second));
    }

    #[test]
    fn ownership_tracks_constant_tuple_elements_independently() {
        let mut state = Node::owned();
        state.do_move(&[Key::ConstIndex(0)]);
        assert_eq!(state.read(&[Key::ConstIndex(0)]).0, Own::Moved);
        assert_eq!(state.read(&[Key::ConstIndex(1)]).0, Own::Owned);
        assert_eq!(state.read(&[Key::Index]).0, Own::Moved);
    }

    #[test]
    fn self_is_never_droppable_even_when_its_stable_slot_is_not_leading() {
        let function = MirFunction {
            blocks: Vec::new(),
            n_regs: 0,
            n_vars: 3,
            var_names: vec!["argument".into(), "self".into(), "local".into()],
            n_params: 1,
            param_types: Vec::new(),
            owned_params: vec![false],
            deinit_params: vec![false],
            ref_params: vec![false],
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: None,
            raises: false,
            error_ty: None,
            spans: SpanTable::default(),
            reg_types: HashMap::new(),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            callee_removed_args: Vec::new(),
            removed_args: Vec::new(),
            lowered_calls: Vec::new(),
        };

        assert!(!is_droppable_root(&function, 1));
        assert!(is_droppable_root(&function, 2));
    }

    #[test]
    fn consuming_receivers_are_droppable_roots_of_the_callee() {
        let mut function = MirFunction {
            blocks: Vec::new(),
            n_regs: 0,
            n_vars: 3,
            var_names: vec!["self".into(), "other".into(), "local".into()],
            n_params: 2,
            param_types: Vec::new(),
            owned_params: vec![false, false],
            deinit_params: vec![true, false],
            ref_params: vec![false, false],
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: None,
            raises: false,
            error_ty: None,
            spans: SpanTable::default(),
            reg_types: HashMap::new(),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            callee_removed_args: Vec::new(),
            removed_args: Vec::new(),
            lowered_calls: Vec::new(),
        };
        assert!(is_droppable_root(&function, 0));
        assert!(!is_droppable_root(&function, 1));

        function.deinit_params = vec![false, false];
        assert!(!is_droppable_root(&function, 0));
        function.owned_params = vec![true, false];
        assert!(is_droppable_root(&function, 0));
    }
}

#[cfg(test)]
mod interior_origin_tests {
    use super::*;
    use mojito_common::token::SourceSpan;
    use mojito_mir::mir::{LoopLabel, MirLoan, SpanTable, UseMode};
    use mojito_types::origin::OriginSeg;
    use mojito_types::types::Ty;
    use std::collections::HashMap;

    fn origin(path: Vec<OriginSeg>) -> MirInteriorOrigin {
        MirInteriorOrigin { root: 0, path }
    }

    fn element_origin() -> MirInteriorOrigin {
        origin(vec![OriginSeg::Interior("element".into())])
    }

    fn loan_with(mutable: bool, interior: Option<MirInteriorOrigin>) -> MirLoan {
        MirLoan {
            place: MirPlace::root(0, None),
            mutable,
            interior,
            shared: false,
            setup: false,
        }
    }

    fn establish(reference: VarId, marker: u32, interior: Option<MirInteriorOrigin>) -> MirInstr {
        establish_with(reference, marker, true, interior)
    }

    fn establish_with(
        reference: VarId,
        marker: u32,
        mutable: bool,
        interior: Option<MirInteriorOrigin>,
    ) -> MirInstr {
        MirInstr::EstablishLoans {
            reference,
            loans: vec![loan_with(mutable, interior)],
            marker: Reg(marker),
            dest_interior: None,
        }
    }

    fn invalidate(marker: u32) -> MirInstr {
        MirInstr::InvalidateInteriors {
            base: origin(Vec::new()),
            except: None,
            include_base_generation: false,
            marker: Reg(marker),
        }
    }

    fn use_reference(reference: VarId, dest: u32) -> MirInstr {
        MirInstr::UseVar {
            dest: Reg(dest),
            var: reference,
            mode: UseMode::BorrowShared,
            constructor: None,
        }
    }

    fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
        MirBlock { instrs, term }
    }

    fn region(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
        vec![block(instrs, term)]
    }

    fn yields(instrs: Vec<MirInstr>) -> Vec<MirBlock> {
        region(instrs, MirTerm::Yield(Vec::new()))
    }

    fn if_else(then: Vec<MirBlock>, orelse: Vec<MirBlock>) -> MirInstr {
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(99),
            then,
            elifs: Vec::new(),
            orelse,
        }
    }

    fn labelled(body: Vec<MirBlock>) -> MirInstr {
        MirInstr::Loop {
            label: Some(LoopLabel(0)),
            body,
        }
    }

    const BREAK: MirTerm = MirTerm::Break {
        label: Some(LoopLabel(0)),
    };
    const CONTINUE: MirTerm = MirTerm::Continue {
        label: Some(LoopLabel(0)),
    };

    fn function(blocks: Vec<MirBlock>, spans: &[(u32, (usize, usize))]) -> MirFunction {
        let mut span_table = SpanTable::default();
        for (reg, span) in spans {
            span_table.0.insert(
                *reg,
                (SourceSpan::new(Some("interior.mojo".into()), *span), None),
            );
        }
        MirFunction {
            blocks,
            n_regs: 128,
            n_vars: 3,
            var_names: vec!["values".into(), "first".into(), "second".into()],
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: Some(Ty::none()),
            raises: false,
            error_ty: None,
            spans: span_table,
            reg_types: HashMap::new(),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            callee_removed_args: Vec::new(),
            removed_args: Vec::new(),
            lowered_calls: Vec::new(),
        }
    }

    #[test]
    fn invalidation_reports_use_and_mutation_spans() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    invalidate(1),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        match analyze_lifetimes(&f) {
            Err(OwnershipError::InvalidatedInteriorReference {
                reference,
                origin,
                span,
                invalidated_at,
            }) => {
                assert_eq!(reference, "first");
                assert_eq!(origin, "values[\"element\"]");
                assert_eq!(span.span, (30, 35));
                assert_eq!(invalidated_at.span, (10, 20));
            }
            other => panic!("expected invalidated interior reference, got {other:?}"),
        }
    }

    #[test]
    fn invalidation_on_one_branch_rejects_use_after_join() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    if_else(yields(vec![invalidate(1)]), yields(Vec::new())),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_on_loop_backedge_rejects_maybe_stale_exit() {
        let exit = if_else(region(Vec::new(), BREAK), yields(Vec::new()));
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    labelled(region(vec![exit, invalidate(1)], CONTINUE)),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_before_a_break_from_a_try_reaches_the_loop_exit() {
        let body = MirInstr::Try {
            body: region(vec![invalidate(1)], BREAK),
            handler: None,
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        };
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    labelled(region(vec![body], CONTINUE)),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn use_after_the_root_moves_is_a_use_of_the_uninitialized_root() {
        let moved = MirInstr::UseVar {
            dest: Reg(1),
            var: 0,
            mode: UseMode::Move,
            constructor: None,
        };
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    moved,
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        match analyze_lifetimes(&f) {
            Err(OwnershipError::UseAfterMove { var, span }) => {
                assert_eq!(var, "values");
                assert_eq!(span.span, (30, 35));
            }
            other => panic!("expected a use of the uninitialized root, got {other:?}"),
        }
    }

    #[test]
    fn rebinding_installs_a_fresh_generation() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    invalidate(1),
                    establish(1, 2, Some(element_origin())),
                    use_reference(1, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_lifetimes(&f).is_ok());
    }

    #[test]
    fn refresh_on_invalidating_branch_preserves_path_correlation() {
        let refresh = vec![invalidate(1), establish(1, 2, Some(element_origin()))];
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    if_else(yields(refresh), yields(Vec::new())),
                    use_reference(1, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_lifetimes(&f).is_ok());
    }

    #[test]
    fn overlapping_interior_loans_are_not_exclusive() {
        // Two element generations over one list, even both mutable (nested
        // `for ref` loops), are generation-tracked rather than exclusive.
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    establish(2, 1, Some(element_origin())),
                    use_reference(1, 2),
                    use_reference(2, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_loans(&f, &CalleeRefParams::new()).is_ok());
    }

    #[test]
    fn mutable_interior_loan_conflicts_with_a_live_whole_place_loan() {
        // A `for ref` loop's mutable element loan established while a shared
        // whole-place view of the list lives, and the symmetric view taken
        // inside such a loop, both conflict.
        for (first, second) in [
            (
                establish_with(1, 0, false, None),
                establish_with(2, 1, true, Some(element_origin())),
            ),
            (
                establish_with(1, 0, true, Some(element_origin())),
                establish_with(2, 1, false, None),
            ),
        ] {
            let f = function(
                vec![block(
                    vec![first, second, use_reference(1, 2), use_reference(2, 3)],
                    MirTerm::Return(None),
                )],
                &[],
            );
            assert!(matches!(
                analyze_loans(&f, &CalleeRefParams::new()),
                Err(OwnershipError::LoanConflict { .. })
            ));
        }
    }

    #[test]
    fn shared_interior_loan_coexists_with_a_shared_whole_place_loan() {
        // A value loop beside a live view reads only.
        let f = function(
            vec![block(
                vec![
                    establish_with(1, 0, false, None),
                    establish_with(2, 1, false, Some(element_origin())),
                    use_reference(1, 2),
                    use_reference(2, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_loans(&f, &CalleeRefParams::new()).is_ok());
    }

    #[test]
    fn overlapping_ordinary_mutable_loans_still_conflict() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, None),
                    establish(2, 1, None),
                    use_reference(1, 2),
                    use_reference(2, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(matches!(
            analyze_loans(&f, &CalleeRefParams::new()),
            Err(OwnershipError::LoanConflict { .. })
        ));
    }

    #[test]
    fn a_write_in_an_if_arm_conflicts_with_a_reference_used_after_it() {
        let write = MirInstr::Store {
            place: MirPlace::root(0, None),
            src: Reg(4),
        };
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, None),
                    if_else(yields(vec![write]), yields(Vec::new())),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(matches!(
            analyze_loans(&f, &CalleeRefParams::new()),
            Err(OwnershipError::LoanConflict { .. })
        ));
    }

    #[test]
    fn a_write_inside_a_try_is_not_loan_checked() {
        let write = MirInstr::Store {
            place: MirPlace::root(0, None),
            src: Reg(4),
        };
        let guarded = MirInstr::Try {
            body: region(vec![write], MirTerm::TryYield),
            handler: None,
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        };
        let f = function(
            vec![block(
                vec![establish(1, 0, None), guarded, use_reference(1, 2)],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_loans(&f, &CalleeRefParams::new()).is_ok());
    }

    #[test]
    fn stale_uses_inside_try_handlers_are_checked() {
        let try_instr = MirInstr::Try {
            body: vec![block(
                vec![invalidate(1), MirInstr::Raise { src: Reg(2) }],
                MirTerm::TryYield,
            )],
            handler: Some((
                None,
                vec![block(vec![use_reference(1, 3)], MirTerm::TryYield)],
            )),
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        };
        let f = function(
            vec![block(
                vec![establish(1, 0, Some(element_origin())), try_instr],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (3, (30, 35))],
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_is_field_sensitive_and_targets_nested_interiors() {
        let left = origin(vec![
            OriginSeg::Field("left".into()),
            OriginSeg::Interior("element".into()),
        ]);
        let right = origin(vec![
            OriginSeg::Field("right".into()),
            OriginSeg::Interior("element".into()),
        ]);
        let left_base = origin(vec![OriginSeg::Field("left".into())]);
        assert!(interior_origin_invalidated_by(&left, &left_base, false));
        assert!(!interior_origin_invalidated_by(&right, &left_base, false));

        let nested = origin(vec![
            OriginSeg::Interior("element".into()),
            OriginSeg::Interior("element".into()),
        ]);
        assert!(!interior_origin_invalidated_by(
            &element_origin(),
            &element_origin(),
            false,
        ));
        assert!(interior_origin_invalidated_by(
            &element_origin(),
            &element_origin(),
            true,
        ));
        let projected = origin(vec![
            OriginSeg::Interior("element".into()),
            OriginSeg::Field("value".into()),
        ]);
        assert!(interior_origin_invalidated_by(
            &projected,
            &element_origin(),
            true,
        ));
        assert!(interior_origin_invalidated_by(
            &nested,
            &element_origin(),
            false,
        ));
        let sibling = origin(vec![OriginSeg::Interior("value".into())]);
        assert!(!interior_origin_invalidated_by(
            &sibling,
            &element_origin(),
            true,
        ));
    }
}

#[cfg(test)]
mod structured_move_tests {
    use super::*;
    use mojito_mir::mir::{LoopLabel, UseMode};
    use mojito_types::types::Ty;

    const X: VarId = 0;
    const S: VarId = 1;

    fn block(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
        vec![MirBlock { instrs, term }]
    }

    fn function(instrs: Vec<MirInstr>, term: MirTerm) -> MirFunction {
        MirFunction {
            blocks: block(instrs, term),
            n_regs: 64,
            n_vars: 2,
            var_names: vec!["x".into(), "s".into()],
            n_params: 1,
            ref_params: vec![false],
            ret_ty: Some(Ty::none()),
            ..MirFunction::default()
        }
    }

    fn use_var(var: VarId, dest: u32, mode: UseMode) -> MirInstr {
        MirInstr::UseVar {
            dest: Reg(dest),
            var,
            mode,
            constructor: None,
        }
    }

    fn take(var: VarId, dest: u32) -> MirInstr {
        use_var(var, dest, UseMode::Move)
    }

    fn read(var: VarId, dest: u32) -> MirInstr {
        use_var(var, dest, UseMode::BorrowShared)
    }

    fn refill(var: VarId, src: u32) -> MirInstr {
        MirInstr::DefVar {
            var,
            src: Reg(src),
            binding_ty: None,
            in_place: false,
        }
    }

    fn field_a() -> MirPlace {
        let mut place = MirPlace::root(X, None);
        place.proj.push(Proj::Field("a".into()));
        place
    }

    fn if_else(then: Vec<MirBlock>, orelse: Vec<MirBlock>) -> MirInstr {
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(60),
            then,
            elifs: Vec::new(),
            orelse,
        }
    }

    fn yields(instrs: Vec<MirInstr>) -> Vec<MirBlock> {
        block(instrs, MirTerm::Yield(Vec::new()))
    }

    fn labelled(body: Vec<MirBlock>) -> MirInstr {
        MirInstr::Loop {
            label: Some(LoopLabel(0)),
            body,
        }
    }

    fn try_part(body: Vec<MirBlock>, finalbody: Option<Vec<MirBlock>>) -> MirInstr {
        MirInstr::Try {
            body,
            handler: None,
            orelse: None,
            finalbody,
            cleanup: Vec::new(),
        }
    }

    const BREAK: MirTerm = MirTerm::Break {
        label: Some(LoopLabel(0)),
    };
    const CONTINUE: MirTerm = MirTerm::Continue {
        label: Some(LoopLabel(0)),
    };

    #[test]
    fn structured_if_joins_its_arms() {
        let f = function(
            vec![
                if_else(yields(vec![take(X, 1)]), yields(Vec::new())),
                read(X, 2),
            ],
            MirTerm::Return(None),
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::ConditionallyMoved { .. })
        ));
    }

    #[test]
    fn structured_loop_carries_its_continue_state_back() {
        let exit = if_else(yields(Vec::new()), block(Vec::new(), BREAK));
        let f = function(
            vec![labelled(block(
                vec![exit, read(X, 1), take(X, 2)],
                CONTINUE,
            ))],
            MirTerm::Return(None),
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::ConditionallyMoved { .. })
        ));
    }

    #[test]
    fn structured_break_from_a_try_reaches_its_loop() {
        let body = try_part(block(vec![take(X, 1)], BREAK), None);
        let f = function(
            vec![labelled(block(vec![body], CONTINUE)), read(X, 2)],
            MirTerm::Return(None),
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::UseAfterMove { .. })
        ));
    }

    #[test]
    fn structured_break_from_a_try_refilled_after_the_loop_is_accepted() {
        let moved = MirInstr::MovePlace {
            dest: Reg(1),
            place: field_a(),
        };
        let body = try_part(block(vec![moved], BREAK), None);
        let store = MirInstr::Store {
            place: field_a(),
            src: Reg(2),
        };
        let f = function(
            vec![labelled(block(vec![body], CONTINUE)), store, read(X, 3)],
            MirTerm::Return(None),
        );
        assert!(analyze_lifetimes(&f).is_ok());
    }

    #[test]
    fn structured_break_runs_the_finally_it_crosses() {
        let body = try_part(
            block(vec![take(X, 1)], BREAK),
            Some(block(vec![refill(X, 2)], MirTerm::TryYield)),
        );
        let f = function(
            vec![labelled(block(vec![body], CONTINUE)), read(X, 3)],
            MirTerm::Return(None),
        );
        assert!(analyze_lifetimes(&f).is_ok());
    }

    #[test]
    fn structured_match_next_carries_its_state_to_the_next_case() {
        let test = if_else(
            yields(Vec::new()),
            block(vec![take(X, 1)], MirTerm::MatchNext),
        );
        let f = function(
            vec![MirInstr::Match {
                cases: vec![
                    block(vec![test], MirTerm::MatchComplete),
                    block(vec![read(X, 2)], MirTerm::MatchComplete),
                ],
                orelse: yields(Vec::new()),
            }],
            MirTerm::Return(None),
        );
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::UseAfterMove { .. })
        ));
    }

    #[test]
    fn structured_unreachable_checks_no_exit() {
        let mut f = function(vec![take(S, 1)], MirTerm::Unreachable);
        f.ref_params = vec![false, true];
        f.n_params = 2;
        assert!(analyze_lifetimes(&f).is_ok());
        f.blocks[0].term = MirTerm::Return(None);
        assert!(matches!(
            analyze_lifetimes(&f),
            Err(OwnershipError::ReferenceParameterLeftUninitialized { .. })
        ));
    }
}
