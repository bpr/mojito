//! Stage 6: ownership and persistent-loan analysis over MIR.
//!
//! Mojo's move semantics: transferring a value with `^` (`y = x^`, `take(x^)`)
//! leaves the source **uninitialized**, so using it again is an error. This pass
//! is a forward dataflow over each `MirFunction`'s basic blocks that tracks, per
//! variable, whether it is `Owned`, `Moved`, or — where control-flow paths
//! disagree — `MaybeMoved`. A use of a `Moved` variable is a **use-after-move**; a
//! use of a `MaybeMoved` one is a **conditional move** (transferred on some paths
//! but not others). Diagnostics carry the source [`Span`](mojito_mir::mir::Span) of the
//! offending use, recovered from the MIR `SpanTable`.
//!
//! This is a distinct compiler stage after MIR lowering. The production
//! [`Compiler`](crate::compiler::Compiler) always runs it before drop elaboration
//! and VM execution. Backward liveness then builds on this move/init foundation
//! to insert ASAP drops and control-flow-edge cleanup.

use mojito_common::error::OwnershipError;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MirBlock, MirCaptureMode, MirFunction, MirInstr, MirInteriorOrigin, MirPlace, MirProgram,
    MirTerm, Proj, Reg, SpanTable, UseMode,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub fn check_ownership_checked(
    program: &mojito_checked::checked::CheckedProgram,
) -> Result<(), OwnershipError> {
    let prog = mojito_mir::mir::lower_checked_program(program);
    check_ownership_program(&prog)
}

/// Run the ownership analysis over an already-lowered program — the
/// standalone-MIR core the pipeline composes with `mir::verify` so production
/// MIR is fully verified before execution.
pub fn check_ownership_program(prog: &MirProgram) -> Result<(), OwnershipError> {
    let callees = CalleeRefParams {
        ref_params: prog
            .functions
            .iter()
            .map(|(name, function)| (name.as_str(), function.ref_params.as_slice()))
            .collect(),
        param_writes: prog
            .declarations
            .functions
            .iter()
            .map(|declaration| {
                (
                    declaration.lowered_name.as_str(),
                    declaration.param_writes.as_slice(),
                )
            })
            .collect(),
        structs: prog
            .declarations
            .structs
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect(),
    };
    timing::count("functions", prog.functions.len() as u64);
    for (_name, f) in &prog.functions {
        {
            let _moves = timing::span("moves");
            analyze_moves(f)?;
        }
        {
            let _origins = timing::span("interior_origins");
            analyze_interior_origins(f)?;
        }
        let _loans = timing::span("loans");
        analyze_loans(f, &callees)?;
    }
    Ok(())
}

/// The program-wide callee contracts the loan analysis classifies retained
/// call places against. A retained place is an exclusive write only where
/// the callee's declaration says the parameter writes through it
/// (`param_writes`: `mut`, or `ref` under a statically mutable origin); a
/// place lent to any other slot — a read convention, or an immutable,
/// parametric, or bare `ref` (a borrowing-view constructor, `Named`) — is a
/// shared read. A callee without a declaration falls back to its function's
/// `ref_params` mask (receiver at slot zero), and an unknown callee stays
/// exclusive. `structs` resolves a single-`__init__` construction, which
/// calls the bare struct name, to its `<name>.__init__` declaration.
#[derive(Default)]
pub struct CalleeRefParams<'a> {
    pub ref_params: HashMap<&'a str, &'a [bool]>,
    pub param_writes: HashMap<&'a str, &'a [bool]>,
    pub structs: HashSet<&'a str>,
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
        let declared = if self.structs.contains(callee) {
            init_name = format!("{callee}.__init__");
            init_name.as_str()
        } else {
            callee
        };
        if let Some(mask) = self.param_writes.get(declared) {
            return match mask.get(argument) {
                Some(false) => LoanAccess::Read,
                _ => LoanAccess::Write,
            };
        }
        let slot = if receiver { argument + 1 } else { argument };
        match self.ref_params.get(callee).and_then(|mask| mask.get(slot)) {
            Some(false) => LoanAccess::Read,
            _ => LoanAccess::Write,
        }
    }
}

// --- Liveness + ASAP drop elaboration ---------------------------------------

/// Elaborate ASAP destruction across a whole program: after each variable's last
/// use, splice a `DropVar`. Applied by the VM before execution so a struct's
/// `__deinit__` fires at the value's last use (not at scope end).
pub fn elaborate_drops_program(prog: MirProgram) -> MirProgram {
    MirProgram {
        functions: prog
            .functions
            .into_iter()
            .map(|(name, f)| {
                // Module-scope (`__toplevel__`) variables live until program end, so
                // they are not ASAP-dropped — that also keeps their final values
                // intact for the CLI/`bindings()` global dump (a `DropVar` would
                // clear the slot).
                let elaborated = if name == "__toplevel__" {
                    f
                } else {
                    let mut elaborated = elaborate_drops(&f);
                    refine_deinit_fields(&mut elaborated, &prog.declarations.structs);
                    elaborated
                };
                (name, elaborated)
            })
            .collect(),
        declarations: prog.declarations,
        invariant_errors: prog.invariant_errors,
    }
}

mod drops;
mod field_drops;
mod interior;
mod loans;
mod moves;
mod register_loans;
mod scan;

use drops::*;
use field_drops::*;
use interior::*;
use loans::*;
use moves::*;
use register_loans::*;
use scan::*;

#[cfg(test)]
mod constant_tuple_place_tests {
    use super::*;

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
    use mojito_mir::mir::MirLoan;
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
        }
    }

    fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
        MirBlock { instrs, term }
    }

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
            ret_ty: Some(Ty::None),
            raises: false,
            error_ty: None,
            spans: span_table,
            reg_types: HashMap::new(),
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
        match analyze_interior_origins(&f) {
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
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 1,
                        else_b: 2,
                    },
                ),
                block(vec![invalidate(1)], MirTerm::Jump(3)),
                block(Vec::new(), MirTerm::Jump(3)),
                block(vec![use_reference(1, 2)], MirTerm::Return(None)),
            ],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_interior_origins(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_on_loop_backedge_rejects_maybe_stale_exit() {
        let f = function(
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Jump(1),
                ),
                block(
                    Vec::new(),
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 2,
                        else_b: 3,
                    },
                ),
                block(vec![invalidate(1)], MirTerm::Jump(1)),
                block(vec![use_reference(1, 2)], MirTerm::Return(None)),
            ],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_interior_origins(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
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
        assert!(analyze_interior_origins(&f).is_ok());
    }

    #[test]
    fn refresh_on_invalidating_branch_preserves_path_correlation() {
        let f = function(
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 1,
                        else_b: 2,
                    },
                ),
                block(
                    vec![invalidate(1), establish(1, 2, Some(element_origin()))],
                    MirTerm::Jump(3),
                ),
                block(Vec::new(), MirTerm::Jump(3)),
                block(vec![use_reference(1, 3)], MirTerm::Return(None)),
            ],
            &[],
        );
        assert!(analyze_interior_origins(&f).is_ok());
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
    fn stale_uses_inside_try_handlers_are_checked() {
        let try_instr = MirInstr::Try {
            body: vec![block(
                vec![invalidate(1), MirInstr::Raise { src: Reg(2) }],
                MirTerm::FallOff,
            )],
            handler: Some((
                None,
                vec![block(vec![use_reference(1, 3)], MirTerm::FallOff)],
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
            analyze_interior_origins(&f),
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
