//! Phase — standalone typed-MIR verification tests.
//!
//! Positive per-fixture coverage lives in `tests/corpus_test.rs` (`verify::*`),
//! which lowers every executable fixture and requires complete register/place
//! typing with no invariant errors, before and after drop elaboration.
//! Negative coverage here (hand-built malformed MIR) pins each verifier check
//! class.

use mojito::mir::verify::verify;
use mojito::types::TyKind;
use mojito::{check_program, parse};
use std::path::Path;

// --- Negative coverage: hand-built malformed MIR per verifier check class ---

use mojito::mir::{
    CleanupDrop, Const, FuncRef, LoopLabel, MirBlock, MirCase, MirDeclarations, MirFunction,
    MirInstr, MirInteriorOrigin, MirIntrinsicSubscript, MirLoan, MirParamArg, MirPlace, MirProgram,
    MirTerm, Proj, Reg, SpanTable, UseMode,
};
use mojito::param_expr::{MetaTy, ParamContext, ParamId};
use mojito::types::DependentType;
use mojito::types::TransferSet;
use mojito::types::{FuncTy, GenericFuncTy};
use mojito::{CtValue, ParamDecl, Ty, TyArg};
use std::collections::HashMap;

fn function(blocks: Vec<MirBlock>, n_regs: u32, reg_types: &[(u32, Ty)]) -> MirFunction {
    MirFunction {
        blocks,
        n_regs,
        n_vars: 1,
        var_names: vec!["x".to_string()],
        n_params: 0,
        param_types: Vec::new(),
        owned_params: Vec::new(),
        deinit_params: Vec::new(),
        ref_params: Vec::new(),
        returns_reference: false,
        var_tys: HashMap::new(),
        ret_ty: Some(Ty::int()),
        raises: false,
        error_ty: None,
        spans: SpanTable::default(),
        reg_types: reg_types.iter().cloned().collect(),
        removed_params: Vec::new(),
        callee_removed_params: Vec::new(),
        callee_removed_args: Vec::new(),
        removed_args: Vec::new(),
        lowered_calls: Vec::new(),
    }
}

fn program(f: MirFunction) -> MirProgram {
    MirProgram {
        functions: vec![("test".to_string(), f)],
        declarations: MirDeclarations::default(),
        invariant_errors: Vec::new(),
    }
}

const fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
    MirBlock { instrs, term }
}

fn expect_finding(prog: &MirProgram, needle: &str) {
    let errors = verify(prog);
    assert!(
        errors.iter().any(|error| error.contains(needle)),
        "expected a finding containing '{needle}', got {errors:?}"
    );
}

fn lower_source(source: &str) -> MirProgram {
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "valid source produced invalid MIR: {:?}",
        mir.invariant_errors
    );
    mir
}

fn reference_ty(referent: Ty, mutability: mojito::Mutability) -> Ty {
    TyKind::Ref(mojito::RefTy {
        referent,
        origin: mojito::Origin::Untracked {
            mutable: !matches!(mutability, mojito::Mutability::Immutable),
        },
        mutability,
    })
    .into()
}

fn tracked_pointer_ty(element: Ty, mutable: bool) -> Ty {
    TyKind::Pointer {
        element,
        origin: mojito::PointerOrigin::Place {
            place: mojito::OriginPlace {
                root: mojito::OwnerId(0),
                path: Vec::new(),
            },
            mutable,
        },
    }
    .into()
}

/// Every instruction of `blocks` that holds no region, at any depth, in
/// order.
fn leaf_instructions_mut(blocks: &mut [MirBlock]) -> Vec<&mut MirInstr> {
    let mut leaves = Vec::new();
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        if mojito::mir::instruction_regions(instruction).is_empty() {
            leaves.push(instruction);
        } else {
            for region in mojito::mir::instruction_regions_mut(instruction) {
                leaves.extend(leaf_instructions_mut(region));
            }
        }
    }
    leaves
}

fn first_index_call(program: &mut MirProgram) -> &mut mojito::mir::MirSubscriptCall {
    program
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::Index {
                call: Some(call), ..
            } => Some(call),
            _ => None,
        })
        .expect("lowered Index call")
}

fn projected_reference_loan_place(program: &mut MirProgram) -> &mut MirPlace {
    program
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::EstablishLoans { loans, .. } => loans.iter_mut().find_map(|loan| {
                (loan.place.through.is_some()
                    && loan.place.proj.iter().any(
                        |projection| matches!(projection, Proj::Field(field) if field == "value"),
                    ))
                .then_some(&mut loan.place)
            }),
            _ => None,
        })
        .expect("projected reference loan place")
}

#[test]
fn consuming_nominal_element_read_has_complete_dispatch_metadata() {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(
            include_str!("../conformance/fixtures/owned_nominal_element_copy.mojo"),
            Path::new("owned_nominal_element_copy.mojo"),
        )
        .expect("compile owned nominal element copy");
    let mir = compiled.mir().clone();
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert_eq!(verify(&mir), Vec::<String>::new());
}

#[test]
fn verifier_rejects_corrupt_projected_reference_handle_metadata() {
    let source = "@fieldwise_init\nstruct Item:\n    var value: Int\n\n@fieldwise_init\nstruct Shelf:\n    var item: Item\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.item)] Item:\n        return self.item\n\ndef main():\n    var shelf = Shelf(Item(1))\n    ref field = shelf[0].value\n    print(field)\n";

    let mut invalid_through = lower_source(source);
    let invalid_slot = invalid_through
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1
        .n_vars;
    projected_reference_loan_place(&mut invalid_through).through =
        Some(u32::try_from(invalid_slot).expect("slot count fits MIR slot id"));
    expect_finding(&invalid_through, "invalid through-reference slot");

    let mut non_reference_through = lower_source(source);
    let non_reference_slot = non_reference_through
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1
        .var_tys
        .iter()
        .find_map(|(slot, ty)| (!matches!(ty.kind(), TyKind::Ref(_))).then_some(*slot))
        .expect("ordinary local slot");
    projected_reference_loan_place(&mut non_reference_through).through = Some(non_reference_slot);
    expect_finding(
        &non_reference_through,
        "is not a checked reference-capability binding",
    );

    let mut wrong_root_type = lower_source(source);
    projected_reference_loan_place(&mut wrong_root_type).root_ty = Some(Ty::int());
    expect_finding(&wrong_root_type, "place root slot");

    let mut wrong_field_type = lower_source(source);
    let place = projected_reference_loan_place(&mut wrong_field_type);
    let field = place
        .proj
        .iter()
        .position(|projection| matches!(projection, Proj::Field(name) if name == "value"))
        .expect("value projection");
    place.projection_tys[field] = Ty::string_literal();
    expect_finding(&wrong_field_type, "field 'value'");
    expect_finding(&wrong_field_type, "typed StringLiteral, declared Int");
}

#[test]
fn verifier_checks_make_ref_capability_types() {
    let place = MirPlace::root(0, Some(Ty::int()));

    let non_capability = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::int())],
    );
    expect_finding(
        &program(non_capability),
        "MakeRef destination has non-reference-capability type Int",
    );

    let wrong_referent = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(
            0,
            reference_ty(Ty::string_literal(), mojito::Mutability::Mutable),
        )],
    );
    expect_finding(
        &program(wrong_referent),
        "MakeRef destination targets StringLiteral, incompatible with place storage Int",
    );

    let raw_pointer = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(
            0,
            TyKind::Pointer {
                element: Ty::int(),
                origin: mojito::PointerOrigin::Untracked { mutable: true },
            }
            .into(),
        )],
    );
    expect_finding(
        &program(raw_pointer),
        "MakeRef destination has non-reference-capability type Pointer",
    );

    let tracked_pointer = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, tracked_pointer_ty(Ty::int(), true))],
    );
    assert_eq!(verify(&program(tracked_pointer)), Vec::<String>::new());
}

#[test]
fn verifier_accepts_forwarding_make_ref_through_stored_reference() {
    // A projected place ENDING at a stored reference admits the forwarding
    // interpretation: the destination is typed as the stored handle itself
    // (`ref s = self.src` reborrows), not as a borrow of the field slot.
    let stored_ref_place = |mutability: mojito::Mutability| {
        let mut place = MirPlace::root(
            0,
            Some(TyKind::Struct("Holder".to_string(), Vec::new()).into()),
        );
        place.project(
            Proj::Field("src".to_string()),
            reference_ty(Ty::int(), mutability),
        );
        place
    };

    let forwarded = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: stored_ref_place(mojito::Mutability::Mutable),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, reference_ty(Ty::int(), mojito::Mutability::Immutable))],
    );
    assert_eq!(verify(&program(forwarded)), Vec::<String>::new());

    let unrelated_referent = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: stored_ref_place(mojito::Mutability::Mutable),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(
            0,
            reference_ty(Ty::string_literal(), mojito::Mutability::Immutable),
        )],
    );
    expect_finding(
        &program(unrelated_referent),
        "MakeRef destination targets StringLiteral",
    );

    let stolen_mutability = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: stored_ref_place(mojito::Mutability::Immutable),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, reference_ty(Ty::int(), mojito::Mutability::Mutable))],
    );
    expect_finding(
        &program(stolen_mutability),
        "MakeRef destination recovers permission unavailable through its source capability",
    );
}

#[test]
fn verifier_checks_read_and_write_reference_capabilities() {
    let read_non_capability = function(
        vec![block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, Ty::int())],
    );
    expect_finding(
        &program(read_non_capability),
        "ReadRef source has non-reference-capability type Int",
    );

    let read_wrong_referent = function(
        vec![block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[
            (0, Ty::string_literal()),
            (1, reference_ty(Ty::int(), mojito::Mutability::Immutable)),
        ],
    );
    expect_finding(
        &program(read_wrong_referent),
        "ReadRef result type StringLiteral is incompatible with referent Int",
    );

    let write_immutable_reference = function(
        vec![block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[
            (0, reference_ty(Ty::int(), mojito::Mutability::Immutable)),
            (1, Ty::int()),
        ],
    );
    expect_finding(
        &program(write_immutable_reference),
        "WriteRef source capability of type ref Int is immutable",
    );

    let write_immutable_pointer = function(
        vec![block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, tracked_pointer_ty(Ty::int(), false)), (1, Ty::int())],
    );
    expect_finding(&program(write_immutable_pointer), "is immutable");

    let write_wrong_referent = function(
        vec![block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[
            (0, tracked_pointer_ty(Ty::int(), true)),
            (1, Ty::string_literal()),
        ],
    );
    expect_finding(
        &program(write_wrong_referent),
        "WriteRef value type StringLiteral is incompatible with referent Int",
    );

    let read_pointer = function(
        vec![block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, tracked_pointer_ty(Ty::int(), false))],
    );
    assert_eq!(verify(&program(read_pointer)), Vec::<String>::new());
}

#[test]
fn verifier_checks_store_ref_source_type_and_permission() {
    let mutable_int = reference_ty(Ty::int(), mojito::Mutability::Mutable);
    let immutable_int = reference_ty(Ty::int(), mojito::Mutability::Immutable);

    let store = |source: Ty| {
        function(
            vec![block(
                vec![MirInstr::StoreRef {
                    place: MirPlace::root(0, Some(mutable_int.clone())),
                    reference: Reg(0),
                }],
                MirTerm::Return(None),
            )],
            1,
            &[(0, source)],
        )
    };

    expect_finding(
        &program(store(Ty::int())),
        "StoreRef source has non-reference type Int",
    );
    expect_finding(
        &program(store(reference_ty(
            Ty::string_literal(),
            mojito::Mutability::Mutable,
        ))),
        "StoreRef source referent StringLiteral is incompatible with storage referent Int",
    );
    expect_finding(
        &program(store(immutable_int)),
        "StoreRef source permission cannot initialize storage",
    );
    assert_eq!(
        verify(&program(store(mutable_int.clone()))),
        Vec::<String>::new()
    );
}

fn value_parameter(name: &str, ty: Ty) -> ParamDecl {
    ParamDecl::Value {
        id: mojito::param_expr::ParamId::new(&format!("$test:{name}"), 0),
        name: name.to_string(),
        ty,
        default: None,
        callable_default: None,
        infer_only: false,
        variadic: false,
        constraints: Vec::new(),
    }
}

fn generic_callable(decls: Vec<ParamDecl>) -> Ty {
    TyKind::GenericFunc(Box::new(GenericFuncTy {
        decls,
        func: FuncTy {
            environment: mojito::CallableEnvironment::Thin,
            params: Vec::new(),
            names: Vec::new(),
            ret: Ty::int(),
            required: Vec::new(),
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: Vec::new(),
            ref_params: Vec::new(),
            ref_return: None,
            transfers: TransferSet::default(),
        },
    }))
    .into()
}

fn dependent_generic_callable(index: &str) -> Ty {
    TyKind::GenericFunc(Box::new(GenericFuncTy {
        decls: vec![value_parameter("index", Ty::int())],
        func: FuncTy {
            environment: mojito::CallableEnvironment::Thin,
            params: vec![DependentType::resolve(
                ParamContext::detached()
                    .select(
                        vec![Ty::int(), Ty::string_literal()],
                        &ParamContext::detached().decl_ref(
                            ParamId::new(&format!("$test:{index}"), 0),
                            index,
                            MetaTy::int(),
                        ),
                    )
                    .expect("an Int index selects"),
            )],
            names: vec!["element".to_string()],
            ret: Ty::none(),
            required: vec![true],
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: vec![None],
            ref_params: vec![None],
            ref_return: None,
            transfers: TransferSet::default(),
        },
    }))
    .into()
}

fn concrete_callable(parameter: Ty) -> Ty {
    TyKind::Func(Box::new(FuncTy {
        environment: mojito::CallableEnvironment::Thin,
        params: vec![parameter],
        names: vec!["element".to_string()],
        ret: Ty::none(),
        required: vec![true],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: vec![None],
        ref_params: vec![None],
        ref_return: None,
        transfers: TransferSet::default(),
    }))
    .into()
}

#[test]
fn verifier_rejects_untyped_registers() {
    let f = function(
        vec![block(
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Int(1),
            }],
            MirTerm::Return(Some(Reg(0))),
        )],
        1,
        &[],
    );
    expect_finding(&program(f), "untyped register r0");
}

#[test]
fn verifier_rejects_a_subscript_receiver_place_with_the_wrong_type() {
    let f = function(
        vec![block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                },
                MirInstr::Const {
                    dest: Reg(1),
                    k: Const::Int(0),
                },
                MirInstr::Index {
                    dest: Reg(2),
                    base: Reg(0),
                    index: Reg(1),
                    base_place: Some(Box::new(MirPlace::root(0, Some(Ty::string_literal())))),
                    index_place: None,
                    call: None,
                    intrinsic: Some(MirIntrinsicSubscript::Pointer),
                },
            ],
            MirTerm::Return(Some(Reg(2))),
        )],
        3,
        &[(0, Ty::int()), (1, Ty::int()), (2, Ty::int())],
    );
    expect_finding(&program(f), "subscript receiver place type StringLiteral");
}

#[test]
fn verifier_rejects_corrupt_subscript_argument_contracts() {
    const SOURCE: &str = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(ref self, mut index: Int) -> Int:\n        index += 1\n        return self.value\n\ndef main():\n    var box = Box(40)\n    var index = 0\n    print(box[index])\n";

    let mut missing = lower_source(SOURCE);
    first_index_call(&mut missing).arguments.clear();
    expect_finding(&missing, "declares 1 fixed parameter");

    let mut out_of_range = lower_source(SOURCE);
    first_index_call(&mut out_of_range).arguments[0].source =
        mojito::checked::CheckedCallArgumentSource::Positional(1);
    expect_finding(&out_of_range, "positional source 1 is out of range");
    expect_finding(&out_of_range, "positional source 0 is represented 0 times");

    let mut duplicate = lower_source(SOURCE);
    let extra = first_index_call(&mut duplicate).arguments[0].clone();
    first_index_call(&mut duplicate).arguments.push(extra);
    expect_finding(&duplicate, "positional source 0 is represented 2 times");
    expect_finding(&duplicate, "has no matching variadic collector");

    let mut missing_place_requirement = lower_source(SOURCE);
    first_index_call(&mut missing_place_requirement).arguments[0].requires_place = false;
    expect_finding(
        &missing_place_requirement,
        "place requirement does not match",
    );

    let mut wrong_type = lower_source(SOURCE);
    first_index_call(&mut wrong_type).arguments[0].parameter_ty = Ty::string_literal();
    expect_finding(&wrong_type, "subscript source has type Int");
    expect_finding(
        &wrong_type,
        "does not match 'Box.__getitem__' declaration Int",
    );

    let mut wrong_result = lower_source(SOURCE);
    first_index_call(&mut wrong_result).result_ty = Ty::string_literal();
    expect_finding(&wrong_result, "subscript result has type Int");
    expect_finding(
        &wrong_result,
        "selected result type StringLiteral does not match 'Box.__getitem__' declaration Int",
    );

    let mut missing_receiver_place_requirement = lower_source(SOURCE);
    first_index_call(&mut missing_receiver_place_requirement).receiver_requires_place = false;
    expect_finding(
        &missing_receiver_place_requirement,
        "receiver place requirement does not match",
    );

    let mut wrong_receiver_convention = lower_source(SOURCE);
    first_index_call(&mut wrong_receiver_convention).receiver_convention =
        Some(mojito::ast::ArgConvention::Mut);
    expect_finding(
        &wrong_receiver_convention,
        "subscript receiver convention Some(Mut)",
    );

    let mut wrong_argument_convention = lower_source(SOURCE);
    first_index_call(&mut wrong_argument_convention).arguments[0].convention =
        Some(mojito::ast::ArgConvention::Imm);
    expect_finding(
        &wrong_argument_convention,
        "subscript parameter 0 convention Some(Imm)",
    );

    let mut receiverless_declaration = lower_source(SOURCE);
    let target = first_index_call(&mut receiverless_declaration)
        .target
        .clone();
    receiverless_declaration
        .declarations
        .functions
        .iter_mut()
        .find(|declaration| declaration.lowered_name == target)
        .expect("selected declaration")
        .has_receiver = false;
    expect_finding(&receiverless_declaration, "has no declared receiver");
}

#[test]
fn verifier_rejects_missing_nominal_subscript_contracts() {
    let source = "@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) -> Int:\n        return self.value\n    def __getitem__(self, columns: Slice) -> Int:\n        return self.value\n    def __getitem__(self, row: Int, column: Int) -> Int:\n        return self.value\n\ndef main():\n    var access = Access(1)\n    print(access[0])\n    print(access[:])\n    print(access[0, 0])\n";
    let mut mir = lower_source(source);
    for instruction in mir
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
    {
        match instruction {
            MirInstr::Index { call, .. }
            | MirInstr::Slice { call, .. }
            | MirInstr::MultiIndex { call, .. } => *call = None,
            _ => {}
        }
    }
    expect_finding(&mir, "index operation must carry exactly one");
    expect_finding(&mir, "slice operation must carry exactly one");
    expect_finding(&mir, "multi-index operation lacks a checked call contract");
}

#[test]
fn verifier_rejects_missing_conflicting_and_mistyped_intrinsic_subscripts() {
    // Slice dispatch shapes are hand-built: source-level slicing is nominal
    // only (the StringLiteral slice intrinsic was retired with the audited
    // head), so a missing or spurious dispatch never comes from lowering.
    let slice = |call, intrinsic| {
        program(function(
            vec![block(
                vec![
                    MirInstr::Const {
                        dest: Reg(0),
                        k: Const::Str("abc".to_string()),
                    },
                    MirInstr::Slice {
                        dest: Reg(1),
                        object: Reg(0),
                        kind: mojito::types::SliceKind::ContiguousSlice,
                        lower: None,
                        upper: None,
                        step: None,
                        object_place: None,
                        arg_places: Vec::new(),
                        call,
                        intrinsic,
                    },
                ],
                MirTerm::Return(Some(Reg(1))),
            )],
            2,
            &[(0, Ty::string_literal()), (1, Ty::string_literal())],
        ))
    };
    let missing = slice(None, None);
    expect_finding(&missing, "slice operation must carry exactly one");

    // No intrinsic slice dispatch remains: any recorded kind is a finding.
    let spurious = slice(None, Some(MirIntrinsicSubscript::Pointer));
    expect_finding(&spurious, "is not a slice dispatch");

    let mut mistyped = lower_source(
        "def main():\n    var lanes = SIMD[DType.int, 2](1, 2)\n    print(lanes[0])\n",
    );
    let intrinsic = mistyped
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::Index { intrinsic, .. } => Some(intrinsic),
            _ => None,
        })
        .expect("SIMD Index lowered");
    assert_eq!(*intrinsic, Some(MirIntrinsicSubscript::Simd));
    *intrinsic = Some(MirIntrinsicSubscript::Pointer);
    expect_finding(&mistyped, "Pointer is incompatible with checked base type");

    let source = "@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) -> Int:\n        return self.value\n\ndef main():\n    print(Access(1)[0])\n";
    let mut conflicting = lower_source(source);
    conflicting
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::Index { intrinsic, .. } => Some(intrinsic),
            _ => None,
        })
        .expect("nominal Index lowered")
        .replace(MirIntrinsicSubscript::TupleStorage);
    expect_finding(&conflicting, "index operation must carry exactly one");
}

#[test]
fn verifier_rejects_cross_receiver_and_fabricated_reference_subscript_contracts() {
    let source = "@fieldwise_init\nstruct Item:\n    var value: Int\n\n@fieldwise_init\nstruct Box:\n    var item: Item\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.item)] Item:\n        return self.item\n\n@fieldwise_init\nstruct Other:\n    var item: Item\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.item)] Item:\n        return self.item\n\ndef main():\n    var box = Box(Item(1))\n    print(box[0].value)\n";

    let mut wrong_owner = lower_source(source);
    first_index_call(&mut wrong_owner).target = "Other.__getitem__".to_string();
    expect_finding(&wrong_owner, "does not belong to receiver type Box");

    let mut wrong_family = lower_source(source);
    first_index_call(&mut wrong_family).target = "Box.__setitem__".to_string();
    expect_finding(&wrong_family, "is not in the __getitem__ method family");

    let mut wrong_referent = lower_source(source);
    first_index_call(&mut wrong_referent)
        .reference_result
        .as_mut()
        .expect("reference-returning contract")
        .referent = Ty::string_literal();
    expect_finding(
        &wrong_referent,
        "reference-result referent StringLiteral does not match 'Box.__getitem__' declaration Item",
    );

    let mut missing_reference_abi = lower_source(source);
    first_index_call(&mut missing_reference_abi).reference_result = None;
    expect_finding(
        &missing_reference_abi,
        "reference-result ABI does not match 'Box.__getitem__' declaration",
    );

    let mut wrong_reference_permission = lower_source(source);
    let reference = first_index_call(&mut wrong_reference_permission)
        .reference_result
        .as_mut()
        .expect("reference-returning contract");
    reference.mutability = match reference.mutability {
        mojito::Mutability::Immutable | mojito::Mutability::Param(_) => mojito::Mutability::Mutable,
        mojito::Mutability::Mutable => mojito::Mutability::Immutable,
    };
    expect_finding(
        &wrong_reference_permission,
        "reference-result metadata ref Item does not match selected result type ref Item",
    );
}

#[test]
fn verifier_classifies_every_raising_subscript_instruction() {
    let source = "@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) raises -> Int:\n        raise Error(\"index\")\n    def __getitem__(self, columns: Slice) raises -> Int:\n        raise Error(\"slice\")\n    def __getitem__(self, row: Int, column: Int) raises -> Int:\n        raise Error(\"multi\")\n    def __setitem__(mut self, row: Int, column: Int, value: Int) raises:\n        raise Error(\"set\")\n\ndef main() raises:\n    var access = Access(0)\n    print(access[0])\n    print(access[:])\n    print(access[0, 0])\n    access[0, 0] = 1\n";
    let mut mir = lower_source(source);
    let (_, main) = mir
        .functions
        .iter_mut()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    main.raises = false;
    let errors = verify(&mir);
    let unprotected = errors
        .iter()
        .filter(|error| error.contains("unprotected raising call"))
        .count();
    assert_eq!(unprotected, 4, "{errors:?}");
}

#[test]
fn verifier_rejects_an_invalid_constant_tuple_projection() {
    let mut place = MirPlace::root(
        0,
        Some(TyKind::Tuple(vec![Ty::int(), Ty::string_literal()]).into()),
    );
    place.project(Proj::ConstIndex(2), Ty::int());
    let f = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(0),
                place,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "projects Tuple element 2 out of 2");
}

#[test]
fn verifier_rejects_malformed_dynamic_place_projections() {
    let pointer = Ty::new(TyKind::Pointer {
        element: Ty::int(),
        origin: mojito::PointerOrigin::Untracked { mutable: true },
    });

    let mut wrong_element = MirPlace::root(0, Some(pointer.clone()));
    wrong_element.project(Proj::Index(Reg(0)), Ty::string_literal());
    let wrong_element = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_element,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, Ty::string_literal())],
    );
    expect_finding(
        &program(wrong_element),
        "place dynamic element typed StringLiteral",
    );

    let mut wrong_index = MirPlace::root(0, Some(pointer));
    wrong_index.project(Proj::Index(Reg(0)), Ty::int());
    let wrong_index = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_index,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::string_literal()), (1, Ty::int())],
    );
    expect_finding(&program(wrong_index), "has non-Indexer type StringLiteral");

    let mut non_indexed = MirPlace::root(0, Some(Ty::int()));
    non_indexed.project(Proj::Index(Reg(0)), Ty::int());
    let non_indexed = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: non_indexed,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, Ty::int())],
    );
    expect_finding(
        &program(non_indexed),
        "dynamic element projection requires checked indexed storage",
    );
}

#[test]
fn verifier_rejects_malformed_interior_loan_metadata() {
    let empty = function(
        vec![block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: Vec::new(),
                marker: Reg(0),
                dest_interior: None,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::none())],
    );
    expect_finding(&program(empty), "loan generation has no owner loans");

    let malformed = function(
        vec![block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: vec![MirLoan {
                    place: MirPlace::root(0, Some(Ty::int())),
                    mutable: true,
                    interior: Some(MirInteriorOrigin {
                        root: 0,
                        path: vec![mojito::OriginSeg::Field("value".to_string())],
                    }),
                    shared: false,
                    setup: false,
                }],
                marker: Reg(0),
                dest_interior: None,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::none())],
    );
    expect_finding(&program(malformed), "has no interior segment");

    let immutable = reference_ty(Ty::int(), mojito::Mutability::Immutable);
    let mut escalated = function(
        vec![block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: vec![MirLoan {
                    place: MirPlace::root(0, Some(immutable.clone())),
                    mutable: true,
                    interior: None,
                    shared: false,
                    setup: false,
                }],
                marker: Reg(0),
                dest_interior: None,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::none())],
    );
    escalated.var_tys.insert(0, immutable);
    expect_finding(
        &program(escalated),
        "mutable loan recovers permission unavailable through its source capability",
    );

    let mismatched_root = function(
        vec![block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: vec![MirLoan {
                    place: MirPlace::root(0, Some(Ty::int())),
                    mutable: false,
                    interior: Some(MirInteriorOrigin {
                        root: 1,
                        path: vec![mojito::OriginSeg::Interior("element".to_string())],
                    }),
                    shared: false,
                    setup: false,
                }],
                marker: Reg(0),
                dest_interior: None,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::none())],
    );
    expect_finding(
        &program(mismatched_root),
        "executable place roots at slot 0",
    );

    let unnamed_refresh = function(
        vec![block(
            vec![MirInstr::InvalidateInteriors {
                base: MirInteriorOrigin {
                    root: 0,
                    path: Vec::new(),
                },
                except: None,
                include_base_generation: true,
                marker: Reg(0),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::none())],
    );
    expect_finding(
        &program(unnamed_refresh),
        "inclusive interior invalidation has no named interior generation",
    );
}

#[test]
fn verifier_rejects_an_undeclared_lifted_closure_target() {
    let f = function(
        vec![block(
            vec![MirInstr::MakeClosure {
                dest: Reg(0),
                function: "missing$nested".to_string(),
                captures: Vec::new(),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "undeclared lifted function 'missing$nested'");
}

#[test]
fn verifier_rejects_invalid_jump_targets() {
    let f = function(vec![block(Vec::new(), MirTerm::Jump(7))], 0, &[]);
    expect_finding(&program(f), "jump to invalid block 7");
}

#[test]
fn verifier_rejects_top_level_falloff_and_escape() {
    let f = function(vec![block(Vec::new(), MirTerm::TryYield)], 0, &[]);
    expect_finding(&program(f), "TryYield terminator outside a try region");
    let f = function(
        vec![block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 0,
                cleanup: Vec::new(),
            },
        )],
        0,
        &[],
    );
    expect_finding(&program(f), "EscapeJump terminator outside a try region");
}

#[test]
fn verifier_rejects_a_branch_to_a_try_region_entry() {
    let body = vec![
        block(Vec::new(), MirTerm::Jump(1)),
        block(Vec::new(), MirTerm::Jump(0)),
    ];
    let f = function(
        vec![block(
            vec![MirInstr::Try {
                body,
                handler: None,
                orelse: None,
                finalbody: None,
                cleanup: Vec::new(),
            }],
            MirTerm::Return(None),
        )],
        0,
        &[],
    );
    expect_finding(&program(f), "branch to its region's entry block");
}

#[test]
fn verifier_rejects_non_bool_branch_conditions() {
    let f = function(
        vec![
            block(
                vec![MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                }],
                MirTerm::Branch {
                    cond: Reg(0),
                    then_b: 1,
                    else_b: 1,
                },
            ),
            block(Vec::new(), MirTerm::Return(None)),
        ],
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "branch condition has type Int");
}

#[test]
fn verifier_rejects_return_type_mismatches() {
    let f = function(
        vec![block(
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Str("no".to_string()),
            }],
            MirTerm::Return(Some(Reg(0))),
        )],
        1,
        &[(0, Ty::string_literal())],
    );
    expect_finding(
        &program(f),
        "return of StringLiteral from a function returning Int",
    );
}

#[test]
fn verifier_rejects_runtime_pack_types_in_function_body_slots() {
    let mut f = function(vec![block(Vec::new(), MirTerm::Return(None))], 0, &[]);
    f.n_params = 1;
    f.param_types = vec![TyKind::RuntimePack(vec![Ty::int(), Ty::string_literal()]).into()];
    f.var_tys.insert(
        0,
        TyKind::RuntimePack(vec![Ty::int(), Ty::string_literal()]).into(),
    );
    expect_finding(&program(f), "retains ABI-only RuntimePack type");
}

#[test]
fn verifier_rejects_unprotected_raises_in_nonraising_functions() {
    let f = function(
        vec![block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Str("boom".to_string()),
                },
                MirInstr::Raise { src: Reg(0) },
            ],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::string_literal())],
    );
    expect_finding(&program(f), "unprotected raise in nonraising function");
}

#[test]
fn verifier_rejects_mismatched_declared_call_arguments() {
    use mojito::mir::MirFunctionDeclaration;
    let f = function(
        vec![block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Str("no".to_string()),
                },
                MirInstr::Call {
                    dest: Reg(1),
                    func: FuncRef::named("callee"),
                    raises: None,
                    args: vec![Reg(0)],
                    kwargs: Vec::new(),
                    arg_places: vec![None],
                    kwarg_places: Vec::new(),
                    param_arg_regs: Vec::new(),
                    capture_accesses: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::string_literal()), (1, Ty::int())],
    );
    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "callee".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![Ty::int()],
        defaults: vec![None],
        required: vec![true],
        variadic: None,
        variadic_convention: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_convention: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: false,
        receiver_convention: None,
        param_conventions: vec![None],
        ret_ty: Ty::int(),
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: vec![false],
        param_writes: vec![false],
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
        inline: mojito_checked::declaration::InlineLevel::default(),
        provenance: mojito_checked::declaration::FunctionProvenance::default(),
    });
    expect_finding(
        &prog,
        "argument 0 of 'callee' has type StringLiteral, declared Int",
    );
    prog.declarations.functions[0].param_conventions.clear();
    expect_finding(&prog, "has 0 parameter conventions, expected 1");
}

#[test]
fn verifier_rejects_malformed_direct_compile_time_arguments() {
    use mojito::mir::MirFunctionDeclaration;
    let declaration = value_parameter("n", Ty::int());
    let f = function(
        vec![block(
            vec![MirInstr::Call {
                dest: Reg(1),
                func: FuncRef::named("callee"),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: Some("n".to_string()),
                    value: Some(Reg(0)),
                    expr: None,
                }],
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::string_literal()), (1, Ty::int())],
    );
    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "callee".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_convention: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_convention: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: vec![declaration],
        has_receiver: false,
        receiver_convention: None,
        param_conventions: Vec::new(),
        ret_ty: Ty::int(),
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: Vec::new(),
        param_writes: Vec::new(),
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
        inline: mojito_checked::declaration::InlineLevel::default(),
        provenance: mojito_checked::declaration::FunctionProvenance::default(),
    });
    expect_finding(&prog, "register type StringLiteral, declared Int");

    let MirInstr::Call { param_arg_regs, .. } = &mut prog.functions[0].1.blocks[0].instrs[0] else {
        unreachable!()
    };
    param_arg_regs.clear();
    expect_finding(
        &prog,
        "required compile-time value parameter 'n' is missing",
    );
}

#[test]
fn verifier_rejects_malformed_indirect_compile_time_arguments() {
    let declarations = vec![value_parameter("n", Ty::int())];
    let callable = generic_callable(declarations.clone());
    let f = function(
        vec![block(
            vec![MirInstr::CallIndirect {
                dest: Reg(2),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                callee_place: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: Some("n".to_string()),
                    value: Some(Reg(1)),
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: None,
                instantiated_args: Vec::new(),
                spread: None,
            }],
            MirTerm::Return(None),
        )],
        3,
        &[(0, callable), (1, Ty::string_literal()), (2, Ty::int())],
    );
    let mut prog = program(f);
    expect_finding(&prog, "register type StringLiteral, declared Int");

    let MirInstr::CallIndirect {
        param_arg_regs,
        param_decls,
        ..
    } = &mut prog.functions[0].1.blocks[0].instrs[0]
    else {
        unreachable!()
    };
    param_arg_regs.clear();
    param_decls.clear();
    expect_finding(
        &prog,
        "compile-time parameter metadata does not match its callable contract",
    );
}

#[test]
fn verifier_substitutes_retained_dependent_callable_arguments() {
    let declarations = vec![value_parameter("index", Ty::int())];
    let f = function(
        vec![block(
            vec![MirInstr::CallIndirect {
                dest: Reg(3),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: vec![Reg(2)],
                kwargs: Vec::new(),
                callee_place: None,
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: None,
                    value: Some(Reg(1)),
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::int())),
                instantiated_args: vec![TyArg::Val(CtValue::Int(0))],
                spread: None,
            }],
            MirTerm::Return(None),
        )],
        4,
        &[
            (0, dependent_generic_callable("index")),
            (1, Ty::int()),
            (2, Ty::int()),
            (3, Ty::none()),
        ],
    );
    let prog = program(f);
    assert_eq!(verify(&prog), Vec::<String>::new());
}

#[test]
fn verifier_rejects_corrupt_or_unbound_dependent_callable_indices() {
    let declarations = vec![value_parameter("index", Ty::int())];
    let f = function(
        vec![block(
            vec![MirInstr::CallIndirect {
                dest: Reg(3),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: vec![Reg(2)],
                kwargs: Vec::new(),
                callee_place: None,
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: None,
                    value: Some(Reg(1)),
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::int())),
                instantiated_args: vec![TyArg::Val(CtValue::Int(1))],
                spread: None,
            }],
            MirTerm::Return(None),
        )],
        4,
        &[
            (0, dependent_generic_callable("index")),
            (1, Ty::int()),
            (2, Ty::int()),
            (3, Ty::none()),
        ],
    );
    let mut prog = program(f);
    expect_finding(
        &prog,
        "checker-instantiated callable contract does not match its retained generic arguments",
    );

    let MirInstr::CallIndirect {
        instantiated_contract,
        instantiated_args,
        ..
    } = &mut prog.functions[0].1.blocks[0].instrs[0]
    else {
        unreachable!()
    };
    *instantiated_contract = None;
    instantiated_args.clear();
    prog.functions[0]
        .1
        .reg_types
        .insert(0, dependent_generic_callable("escaped"));
    expect_finding(
        &prog,
        "dependent index references unbound parameter(s): escaped",
    );
}

#[test]
fn verifier_rejects_corrupt_indirect_effect_and_reference_result_metadata() {
    let contract = TyKind::Func(Box::new(FuncTy {
        environment: mojito::CallableEnvironment::Thin,
        params: Vec::new(),
        names: Vec::new(),
        ret: Ty::int(),
        required: Vec::new(),
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: true,
        error: Some(Ty::string_literal()),
        conventions: Vec::new(),
        ref_params: Vec::new(),
        ref_return: Some(Box::new(mojito::origin::RefSig {
            origin: mojito::origin::SigOrigin::Static,
            mutability: mojito::origin::SigMutability::Mutable,
        })),
        transfers: TransferSet::default(),
    }));
    let result = TyKind::Ref(mojito::RefTy {
        referent: Ty::int(),
        origin: mojito::Origin::Untracked { mutable: false },
        mutability: mojito::Mutability::Immutable,
    });
    let f = function(
        vec![block(
            vec![MirInstr::CallIndirect {
                dest: Reg(1),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                callee_place: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_contract: None,
                instantiated_args: Vec::new(),
                spread: None,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, contract.into()), (1, result.into())],
    );
    let prog = program(f);
    expect_finding(&prog, "raising metadata does not match");
    expect_finding(&prog, "reference result mutability does not match");
    expect_finding(&prog, "reference result origin does not match");
}

#[test]
fn verifier_rejects_misaligned_call_place_metadata() {
    let f = function(
        vec![block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                },
                MirInstr::Call {
                    dest: Reg(1),
                    func: FuncRef::named("callee"),
                    raises: None,
                    args: vec![Reg(0)],
                    kwargs: Vec::new(),
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    param_arg_regs: Vec::new(),
                    capture_accesses: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, Ty::none())],
    );
    expect_finding(&program(f), "call place metadata is not aligned");
}

#[test]
fn verifier_rejects_a_mismatched_iterator_exhaustion_contract() {
    use mojito::mir::MirFunctionDeclaration;

    let other_error = Ty::new(TyKind::Struct("OtherError".to_string(), Vec::new()));
    let stop_iteration = TyKind::Struct("StopIteration".to_string(), Vec::new());
    let mut f = function(
        vec![block(
            vec![MirInstr::TryNext {
                dest: Reg(0),
                yielded: Reg(1),
                iter: 0,
                call: Box::new(mojito::checked::CheckedIteratorCall {
                    target: "Iterator.__next__".to_string(),
                    result_ty: Ty::int(),
                    reference_result: None,
                    raises: Some(other_error.clone()),
                    result_adapter: None,
                }),
                exhaustion: other_error,
                adapter_constructor: None,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::int()), (1, Ty::bool())],
    );
    f.var_names[0] = "iterator".to_string();
    f.var_tys
        .insert(0, TyKind::Struct("Iterator".to_string(), Vec::new()).into());

    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "Iterator.__next__".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_convention: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_convention: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: true,
        receiver_convention: Some(mojito::ast::ArgConvention::Mut),
        param_conventions: Vec::new(),
        ret_ty: Ty::int(),
        returns_reference: false,
        raises: true,
        error_ty: Some(stop_iteration.into()),
        ref_params: Vec::new(),
        param_writes: Vec::new(),
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
        inline: mojito_checked::declaration::InlineLevel::default(),
        provenance: mojito_checked::declaration::FunctionProvenance::default(),
    });

    expect_finding(&prog, "TryNext catches non-StopIteration type OtherError");
    expect_finding(&prog, "does not match 'Iterator.__next__' raising contract");
}

#[test]
fn verifier_rejects_an_iterator_reference_result_abi_mismatch() {
    use mojito::mir::MirFunctionDeclaration;

    let stop_iteration = Ty::new(TyKind::Struct("StopIteration".to_string(), Vec::new()));
    let result_ty = reference_ty(Ty::int(), mojito::Mutability::Immutable);
    let TyKind::Ref(reference_result) = result_ty.kind() else {
        unreachable!("reference_ty constructs TyKind::Ref")
    };
    let mut f = function(
        vec![block(
            vec![MirInstr::TryNext {
                dest: Reg(0),
                yielded: Reg(1),
                iter: 0,
                call: Box::new(mojito::checked::CheckedIteratorCall {
                    target: "Iterator.__next__".to_string(),
                    result_ty: result_ty.clone(),
                    reference_result: Some(reference_result.clone()),
                    raises: Some(stop_iteration.clone()),
                    result_adapter: None,
                }),
                exhaustion: stop_iteration.clone(),
                adapter_constructor: None,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, result_ty), (1, Ty::bool())],
    );
    f.var_names[0] = "iterator".to_string();
    f.var_tys
        .insert(0, TyKind::Struct("Iterator".to_string(), Vec::new()).into());

    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "Iterator.__next__".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_convention: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_convention: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: true,
        receiver_convention: Some(mojito::ast::ArgConvention::Mut),
        param_conventions: Vec::new(),
        ret_ty: Ty::int(),
        // Deliberately corrupt: the checked operation returns a reference, but
        // the retained declaration claims an owned-value ABI.
        returns_reference: false,
        raises: true,
        error_ty: Some(stop_iteration),
        ref_params: Vec::new(),
        param_writes: Vec::new(),
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
        inline: mojito_checked::declaration::InlineLevel::default(),
        provenance: mojito_checked::declaration::FunctionProvenance::default(),
    });

    expect_finding(&prog, "TryNext result contract does not match");
}

// Pins the abstract-dispatch verify witness via the raw seam (schema-freeze
// survivor); under the authoritative Compiler this shape is retained-template
// residue.
#[test]
fn verifier_rejects_missing_or_value_step_iterator_result_adapters() {
    let source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    let mut missing = lower_source(source);
    let call = missing
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "first")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::TryNext { call, .. } => Some(call),
            _ => None,
        })
        .expect("abstract iterator next");
    call.result_adapter = None;
    expect_finding(
        &missing,
        "abstract value-returning iterator dispatch lacks its copy-reference adapter",
    );

    let mut concrete = lower_source(source);
    let call = concrete
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "first")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::TryNext { call, .. } => Some(call),
            _ => None,
        })
        .expect("abstract iterator next");
    call.target = "IntRefIter.__len__".to_string();
    expect_finding(
        &concrete,
        "TryNext result contract does not match 'IntRefIter.__len__'",
    );
}

// Pins the abstract-dispatch verify witness via the raw seam (schema-freeze
// survivor); under the authoritative Compiler this shape is retained-template
// residue.
#[test]
fn verifier_rejects_a_missing_abstract_next_method_adapter() {
    let source = include_str!("../conformance/fixtures/copyable_iterator_refinement.mojo");
    let mut prog = lower_source(source);
    let adapter = prog
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "take")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                result_adapter,
                ..
            } if method == "__next__" => Some(result_adapter),
            _ => None,
        })
        .expect("abstract __next__ method call");
    *adapter = None;
    expect_finding(
        &prog,
        "abstract value-returning __next__ call lacks its copy-reference adapter",
    );
}

// Pins the abstract-dispatch verify witness via the raw seam (schema-freeze
// survivor); under the authoritative Compiler this shape is retained-template
// residue.
#[test]
fn verifier_distinguishes_method_result_abi_from_a_reference_shaped_value() {
    let result_ty = reference_ty(Ty::int(), mojito::Mutability::Immutable);
    let TyKind::Ref(reference) = result_ty.kind() else {
        unreachable!("reference_ty constructs TyKind::Ref")
    };
    let abstract_next = |reference_result, result_adapter| {
        function(
            vec![block(
                vec![MirInstr::MethodCall {
                    dest: Reg(1),
                    recv: Reg(0),
                    method: "__next__".to_string(),
                    resolved: Some("__trait_dispatch.__next__".to_string()),
                    raises: None,
                    reference_result,
                    result_adapter,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    recv_place: None,
                    recv_writes: true,
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: None,
                }],
                MirTerm::Return(None),
            )],
            2,
            &[
                (
                    0,
                    TyKind::Struct("OpaqueIterator".to_string(), Vec::new()).into(),
                ),
                (1, result_ty.clone()),
            ],
        )
    };

    // A value-returning contract can have a reference-shaped Element. Its ABI
    // still needs the adapter because ABI is not inferred from the value type.
    let value_abi = program(abstract_next(
        None,
        Some(mojito::mir::MirResultAdapter::new(
            mojito::checked::CheckedResultAdapter::CopyIteratorReference,
        )),
    ));
    assert_eq!(verify(&value_abi), Vec::<String>::new());

    // A genuine abstract reference ABI carries its reference metadata and does
    // not use the value-result adapter.
    let reference_abi = program(abstract_next(Some(Box::new(reference.clone())), None));
    assert_eq!(verify(&reference_abi), Vec::<String>::new());
}

#[test]
fn verifier_rejects_corrupt_concrete_method_result_abis() {
    let reference_source = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def get(ref self) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef main():\n    var box = Box(1)\n    ref value = box.get()\n    print(value)\n";
    let mut erased = lower_source(reference_source);
    let reference_result = erased
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                reference_result,
                ..
            } if method == "get" => Some(reference_result),
            _ => None,
        })
        .expect("reference-returning method call");
    assert!(reference_result.take().is_some());
    expect_finding(&erased, "method-call result ABI does not match 'Box.get'");

    let value_source = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def get(self) -> Int:\n        return self.value\n\ndef main():\n    var box = Box(1)\n    print(box.get())\n";
    let mut fabricated = lower_source(value_source);
    let reference_result = fabricated
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                reference_result,
                ..
            } if method == "get" => Some(reference_result),
            _ => None,
        })
        .expect("value-returning method call");
    *reference_result = match (reference_ty(Ty::int(), mojito::Mutability::Immutable)).kind() {
        TyKind::Ref(reference) => Some(Box::new(reference.clone())),
        _ => unreachable!("reference_ty constructs TyKind::Ref"),
    };
    expect_finding(
        &fabricated,
        "method-call result ABI does not match 'Box.get'",
    );
}

#[test]
fn verifier_rejects_a_loan_destination_domain_with_a_foreign_root() {
    // The destination domain of a transferred-loan generation must root at
    // the generation's own reference slot.
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Carrier[origin: Origin[mut=True]]:\n    var slot: RefBox[Self.origin]\n\n@fieldwise_init\nstruct Two[origin: Origin[mut=True]]:\n    var a: Carrier[Self.origin]\n    var b: List[Int]\n\ndef stash_into_a(mut t: Two, var box: RefBox):\n    t.a.slot = box^\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var t = Two(Carrier(RefBox(whole)), List[Int](1))\n    var local: List[Int] = [9]\n    ref alias = local\n    stash_into_a(t, RefBox(alias))\n    print(1)\n";
    let mut prog = lower_source(source);
    let domain = prog
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::EstablishLoans {
                dest_interior: Some(domain),
                ..
            } => Some(domain),
            _ => None,
        })
        .expect("interior-destination generation");
    domain.root += 1;
    expect_finding(&prog, "loan destination domain roots at slot");
}

#[test]
fn verifier_checks_stores_into_erased_instance_storage() {
    let with_environment = |environment| {
        let mut ty = concrete_callable(Ty::int());
        ty.update(|kind| {
            if let Some(FuncTy {
                environment: slot, ..
            }) = kind.as_func_mut()
            {
                *slot = environment;
            }
        });
        ty
    };
    let store = |storage: Ty, source: Ty| {
        program(function(
            vec![block(
                vec![MirInstr::Store {
                    place: MirPlace::root(0, Some(storage)),
                    src: Reg(0),
                }],
                MirTerm::Return(None),
            )],
            1,
            &[(0, source)],
        ))
    };
    let clean = |prog: &MirProgram| assert_eq!(verify(prog), Vec::<String>::new());

    // A `ref` element slot takes a reference of any origin, or writes
    // through to its referent.
    let slot = reference_ty(Ty::int(), mojito::Mutability::Mutable);
    let mut elsewhere = slot.clone();
    elsewhere.update(|kind| {
        if let TyKind::Ref(reference) = kind {
            reference.origin = mojito::Origin::Unbound;
        }
    });
    clean(&store(slot.clone(), elsewhere));
    clean(&store(slot.clone(), Ty::int()));
    expect_finding(
        &store(
            slot,
            reference_ty(Ty::string_literal(), mojito::Mutability::Mutable),
        ),
        "store of ref StringLiteral into storage of type Int",
    );

    // A capturing closure fits unqualified callable storage; an unqualified
    // callable does not fit capturing storage.
    let capturing = with_environment(mojito::CallableEnvironment::Capturing(
        mojito::CaptureOriginSet::Concrete(Vec::new()),
    ));
    let unqualified = with_environment(mojito::CallableEnvironment::Default);
    clean(&store(unqualified.clone(), capturing.clone()));
    expect_finding(&store(capturing, unqualified), "store of def(");
}

#[test]
fn verifier_rejects_a_method_call_spread_of_a_non_pack_register() {
    let spread_call = |resolved: Option<String>| {
        program(function(
            vec![block(
                vec![MirInstr::MethodCall {
                    dest: Reg(2),
                    recv: Reg(0),
                    method: "take".to_string(),
                    resolved,
                    raises: None,
                    reference_result: None,
                    result_adapter: None,
                    args: vec![Reg(1)],
                    kwargs: Vec::new(),
                    recv_place: None,
                    recv_writes: false,
                    arg_places: vec![None],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: Some(0),
                }],
                MirTerm::Return(None),
            )],
            3,
            &[
                (0, TyKind::Struct("Sink".to_string(), Vec::new()).into()),
                (1, Ty::int()),
                (2, Ty::none()),
            ],
        ))
    };
    expect_finding(
        &spread_call(Some("Sink.take".to_string())),
        "spread argument 0 has type Int, not a pack collector",
    );
    expect_finding(
        &spread_call(None),
        "unresolved method call spreads argument 0",
    );
}

#[test]
fn verifier_rejects_in_place_binding_without_its_producer() {
    let in_place = |producer: MirInstr| {
        program(function(
            vec![block(
                vec![
                    producer,
                    MirInstr::DefVar {
                        var: 0,
                        src: Reg(0),
                        binding_ty: Some(Ty::int()),
                        in_place: true,
                    },
                ],
                MirTerm::Return(None),
            )],
            1,
            &[(0, Ty::int())],
        ))
    };
    expect_finding(
        &in_place(MirInstr::Const {
            dest: Reg(0),
            k: Const::Int(1),
        }),
        "in-place binding of r0 does not follow the call producing it",
    );
}

/// The slot-end findings `verify` reports for a function over slot 0 (`x`),
/// registers r0 (Int), r1 (Bool), and r2 (Int).
fn slot_end_findings(blocks: Vec<MirBlock>) -> Vec<String> {
    let f = function(
        blocks,
        3,
        &[(0, Ty::int()), (1, Ty::bool()), (2, Ty::int())],
    );
    verify(&program(f))
        .into_iter()
        .filter(|finding| finding.contains("every path has already ended"))
        .collect()
}

fn define_x() -> Vec<MirInstr> {
    vec![
        MirInstr::Const {
            dest: Reg(0),
            k: Const::Int(1),
        },
        MirInstr::Const {
            dest: Reg(1),
            k: Const::Bool(true),
        },
        MirInstr::DefVar {
            var: 0,
            src: Reg(0),
            binding_ty: Some(Ty::int()),
            in_place: false,
        },
    ]
}

const fn move_x() -> MirInstr {
    MirInstr::UseVar {
        dest: Reg(2),
        var: 0,
        mode: UseMode::Move,
        constructor: None,
    }
}

const DROP_X: MirInstr = MirInstr::DropVar {
    var: 0,
    destructor: None,
};

fn raising_call() -> MirInstr {
    MirInstr::Call {
        dest: Reg(2),
        func: FuncRef::named("boom"),
        raises: Some(Ty::int()),
        args: Vec::new(),
        kwargs: Vec::new(),
        arg_places: Vec::new(),
        kwarg_places: Vec::new(),
        param_arg_regs: Vec::new(),
        capture_accesses: Vec::new(),
        receiver: None,
        instantiated_args: Vec::new(),
        spread: None,
    }
}

/// A `try` around `body`, with an empty handler when `handler` holds.
fn try_region(body: Vec<MirBlock>, handler: Option<Vec<MirBlock>>) -> MirInstr {
    MirInstr::Try {
        body,
        handler: handler.map(|blocks| (None, blocks)),
        orelse: None,
        finalbody: None,
        cleanup: Vec::new(),
    }
}

/// A drop or consume of a slot every path already dropped, consumed, or
/// moved destroys its value twice.
#[test]
fn verifier_rejects_a_drop_of_an_ended_slot() {
    let doubled = [define_x(), vec![DROP_X, DROP_X]].concat();
    let findings = slot_end_findings(vec![block(doubled, MirTerm::Return(None))]);
    assert_eq!(
        findings,
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that every path has already ended"
        ]
    );
    let consumed = [define_x(), vec![DROP_X, MirInstr::ConsumeVar { var: 0 }]].concat();
    let findings = slot_end_findings(vec![block(consumed, MirTerm::Return(None))]);
    assert!(
        findings[0].contains("var.consume of slot $v0"),
        "{findings:?}"
    );
    let branch = MirTerm::Branch {
        cond: Reg(1),
        then_b: 1,
        else_b: 2,
    };
    let both_arms = vec![
        block(define_x(), branch.clone()),
        block(vec![move_x()], MirTerm::Jump(3)),
        block(vec![move_x()], MirTerm::Jump(3)),
        block(vec![DROP_X], MirTerm::Return(None)),
    ];
    assert_eq!(slot_end_findings(both_arms).len(), 1);
    // An escape and the `try`'s fall-off both reach block 1 with `x` moved.
    let body = vec![
        block(vec![move_x()], branch),
        block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 1,
                cleanup: Vec::new(),
            },
        ),
        block(Vec::new(), MirTerm::TryYield),
    ];
    let escaping = vec![
        block(
            [define_x(), vec![try_region(body, None)]].concat(),
            MirTerm::Jump(1),
        ),
        block(vec![DROP_X], MirTerm::Return(None)),
    ];
    assert_eq!(slot_end_findings(escaping).len(), 1);
}

/// A `try` whose raise edge, an escape, or a return runs a cleanup list
/// over `x` once, after `x` ended or before something ends it again.
fn listed_try(
    body: Vec<MirBlock>,
    handler: Option<Vec<MirBlock>>,
    finalbody: Option<Vec<MirBlock>>,
) -> MirInstr {
    MirInstr::Try {
        body,
        handler: handler.map(|blocks| (None, blocks)),
        orelse: None,
        finalbody,
        cleanup: vec![CleanupDrop::unnamed(0)],
    }
}

/// A cleanup list naming a slot every path reaching it already ended
/// destroys its value twice, as a second `drop.var` does: a `try`'s
/// raise-edge list, a return's, and an escape's, and a drop after any of
/// them.
#[test]
fn verifier_rejects_a_cleanup_list_naming_an_ended_slot() {
    let top = |region: MirInstr| {
        vec![block(
            [define_x(), vec![region]].concat(),
            MirTerm::Return(None),
        )]
    };
    let body = vec![block(vec![DROP_X, raising_call()], MirTerm::TryYield)];
    let raised = top(listed_try(body, Some(Vec::new()), None));
    assert_eq!(
        slot_end_findings(raised),
        [
            "MIR function 'test' block 0: try.cleanup of slot $v0 ('x') that every path has already ended"
        ]
    );
    let body = vec![block(vec![raising_call()], MirTerm::TryYield)];
    let handler = vec![block(vec![DROP_X], MirTerm::TryYield)];
    let findings = slot_end_findings(top(listed_try(body, Some(handler), None)));
    assert!(findings[0].contains("drop.var of slot $v0"), "{findings:?}");
    let returned = vec![block(
        [define_x(), vec![DROP_X]].concat(),
        MirTerm::ReturnWithCleanup {
            value: None,
            cleanup: vec![CleanupDrop::unnamed(0)],
        },
    )];
    let findings = slot_end_findings(returned);
    assert!(
        findings[0].contains("return.cleanup of slot $v0"),
        "{findings:?}"
    );
    let body = vec![block(
        Vec::new(),
        MirTerm::ReturnWithCleanup {
            value: None,
            cleanup: vec![CleanupDrop::unnamed(0)],
        },
    )];
    let finalbody = vec![block(vec![DROP_X], MirTerm::TryYield)];
    let findings = slot_end_findings(top(try_region_finally(body, finalbody)));
    assert!(
        findings[0].contains("return.cleanup of slot $v0"),
        "{findings:?}"
    );
    let body = vec![block(
        vec![DROP_X],
        MirTerm::EscapeJump {
            target: 1,
            cleanup: vec![CleanupDrop::unnamed(0)],
        },
    )];
    let escaping = vec![
        block(
            [define_x(), vec![try_region(body, None)]].concat(),
            MirTerm::Jump(1),
        ),
        block(Vec::new(), MirTerm::Return(None)),
    ];
    let findings = slot_end_findings(escaping);
    assert!(
        findings[0].contains("escape cleanup of slot $v0"),
        "{findings:?}"
    );
}

/// A cleanup list naming a slot some path still holds where it runs is
/// no finding: a raise-edge list over a slot one raise point holds, and a
/// return's over a slot the `finally` it crosses reads but leaves held.
#[test]
fn verifier_accepts_a_cleanup_list_naming_a_held_slot() {
    let top = |region: MirInstr| {
        vec![block(
            [define_x(), vec![region]].concat(),
            MirTerm::Return(None),
        )]
    };
    let body = vec![block(
        vec![raising_call(), DROP_X, raising_call()],
        MirTerm::TryYield,
    )];
    let raised = top(listed_try(body, Some(Vec::new()), None));
    assert_eq!(slot_end_findings(raised), Vec::<String>::new());
    let body = vec![block(
        Vec::new(),
        MirTerm::ReturnWithCleanup {
            value: None,
            cleanup: vec![CleanupDrop::unnamed(0)],
        },
    )];
    let finalbody = vec![block(
        vec![MirInstr::KeepAlive { var: 0 }],
        MirTerm::TryYield,
    )];
    let returned = top(try_region_finally(body, finalbody));
    assert_eq!(slot_end_findings(returned), Vec::<String>::new());
}

/// A `try` around `body` with no handler and `finalbody` as its `finally`.
const fn try_region_finally(body: Vec<MirBlock>, finalbody: Vec<MirBlock>) -> MirInstr {
    MirInstr::Try {
        body,
        handler: None,
        orelse: None,
        finalbody: Some(finalbody),
        cleanup: Vec::new(),
    }
}

/// A slot ended on some paths only, re-stored, or still live where a
/// region escapes or raises is not ended on every path.
#[test]
fn verifier_accepts_a_drop_of_a_slot_some_path_left_live() {
    let branch = MirTerm::Branch {
        cond: Reg(1),
        then_b: 1,
        else_b: 2,
    };
    let one_arm = vec![
        block(define_x(), branch.clone()),
        block(vec![move_x()], MirTerm::Jump(3)),
        block(Vec::new(), MirTerm::Jump(3)),
        block(vec![DROP_X], MirTerm::Return(None)),
    ];
    assert_eq!(slot_end_findings(one_arm), Vec::<String>::new());
    let restored = [
        define_x(),
        vec![
            move_x(),
            MirInstr::DefVar {
                var: 0,
                src: Reg(0),
                binding_ty: Some(Ty::int()),
                in_place: false,
            },
            DROP_X,
        ],
    ]
    .concat();
    let restored = vec![block(restored, MirTerm::Return(None))];
    assert_eq!(slot_end_findings(restored), Vec::<String>::new());
    // The body escapes to block 1 before moving `x`, and falls off after.
    let body = vec![
        block(Vec::new(), branch),
        block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 1,
                cleanup: Vec::new(),
            },
        ),
        block(vec![move_x()], MirTerm::TryYield),
    ];
    let escaping = vec![
        block(
            [define_x(), vec![try_region(body, None)]].concat(),
            MirTerm::Jump(1),
        ),
        block(vec![DROP_X], MirTerm::Return(None)),
    ];
    assert_eq!(slot_end_findings(escaping), Vec::<String>::new());
    // The body moves `x` after its last raise, so its handler still owns it.
    let body = vec![block(vec![raising_call(), move_x()], MirTerm::TryYield)];
    let handler = vec![block(vec![DROP_X], MirTerm::TryYield)];
    let raising = vec![block(
        [define_x(), vec![try_region(body, Some(handler))]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(raising), Vec::<String>::new());
}

const LOOP: Option<LoopLabel> = Some(LoopLabel(0));

/// An `if` on r1 with `then` and `orelse` as its arms.
const fn if_region(then: Vec<MirBlock>, orelse: Vec<MirBlock>) -> MirInstr {
    MirInstr::If {
        results: Vec::new(),
        cond: Reg(1),
        then,
        elifs: Vec::new(),
        else_args: Vec::new(),
        orelse,
    }
}

/// A loop labelled [`LOOP`] around `body`.
const fn loop_region(body: Vec<MirBlock>) -> MirInstr {
    MirInstr::Loop {
        label: LOOP,
        operands: Vec::new(),
        args: Vec::new(),
        results: Vec::new(),
        body,
    }
}

/// One block running `instrs` and ending in `term`, as a region.
fn region(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
    vec![block(instrs, term)]
}

fn yields(instrs: Vec<MirInstr>) -> Vec<MirBlock> {
    region(instrs, MirTerm::Yield(Vec::new()))
}

fn breaks(instrs: Vec<MirInstr>) -> Vec<MirBlock> {
    region(
        instrs,
        MirTerm::Break {
            label: LOOP,
            values: Vec::new(),
        },
    )
}

/// `main`'s body: define `x`, run `ops`, then drop `x`.
fn defined_then_dropped(ops: Vec<MirInstr>) -> Vec<MirBlock> {
    vec![block(
        [define_x(), ops, vec![DROP_X]].concat(),
        MirTerm::Return(None),
    )]
}

/// A slot every path through a region op ends is ended after it: an `if`
/// whose arms both move it, a loop whose every `break` follows a move, one
/// leaving through a `try`'s `finally`, and a `match` whose completing case
/// and else region both move it. A `try` in an arm whose list names a slot
/// ended before its raise point is the same finding.
#[test]
fn verifier_rejects_a_drop_of_a_slot_a_region_op_ended() {
    let both_arms = if_region(yields(vec![move_x()]), yields(vec![move_x()]));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![both_arms])),
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that every path has already ended"
        ]
    );
    let looped = loop_region(breaks(vec![move_x()]));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![looped])).len(),
        1
    );
    let finally = region(Vec::new(), MirTerm::TryYield);
    let crossing = loop_region(region(
        vec![try_region_finally(breaks(vec![move_x()]), finally)],
        MirTerm::Unreachable,
    ));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![crossing])).len(),
        1
    );
    let matched = MirInstr::Match {
        results: Vec::new(),
        cases: vec![MirCase {
            args: Vec::new(),
            body: region(vec![move_x()], MirTerm::MatchComplete(Vec::new())),
        }],
        else_args: Vec::new(),
        orelse: yields(vec![move_x()]),
    };
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![matched])).len(),
        1
    );
    let listed = listed_try(
        region(vec![DROP_X, raising_call()], MirTerm::TryYield),
        Some(Vec::new()),
        None,
    );
    let in_arm = if_region(yields(vec![listed]), yields(Vec::new()));
    let findings = slot_end_findings(vec![block(
        [define_x(), vec![in_arm]].concat(),
        MirTerm::Return(None),
    )]);
    assert_eq!(
        findings,
        [
            "MIR function 'test' block 0: try.cleanup of slot $v0 ('x') that every path has already ended"
        ]
    );
}

/// A slot some path through a region op leaves live is not ended after it:
/// an `if` with one arm moving it, a loop whose body drops it and continues
/// once a store before the loop filled it, and a `match` whose `match.next`
/// path leaves it for the else region.
#[test]
fn verifier_accepts_a_drop_of_a_slot_a_region_op_left_live() {
    let one_arm = if_region(yields(vec![move_x()]), yields(Vec::new()));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![one_arm])),
        Vec::<String>::new()
    );
    let leave = if_region(yields(Vec::new()), breaks(Vec::new()));
    let looped = loop_region(region(
        vec![leave, DROP_X],
        MirTerm::Continue {
            label: LOOP,
            values: Vec::new(),
        },
    ));
    let filled = vec![block(
        [define_x(), vec![looped]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(filled), Vec::<String>::new());
    let test = if_region(
        region(vec![move_x()], MirTerm::MatchComplete(Vec::new())),
        region(Vec::new(), MirTerm::MatchNext(Vec::new())),
    );
    let matched = MirInstr::Match {
        results: Vec::new(),
        cases: vec![MirCase {
            args: Vec::new(),
            body: region(vec![test], MirTerm::Unreachable),
        }],
        else_args: Vec::new(),
        orelse: yields(vec![DROP_X]),
    };
    let next = vec![block(
        [define_x(), vec![matched]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(next), Vec::<String>::new());
}
