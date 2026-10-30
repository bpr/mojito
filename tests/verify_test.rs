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
    Const, FuncRef, LoopLabel, MirBlock, MirCase, MirDeclarations, MirFunction, MirInstr,
    MirInteriorOrigin, MirIntrinsicSubscript, MirLoan, MirParamArg, MirPlace, MirProgram, MirTerm,
    Proj, Raises, Reg, SpanTable, TraitDeclaration, TraitRequirement, TryLabel, UseMode,
};
use mojito::param_expr::{MetaTy, ParamContext, ParamId, ParamRef};
use mojito::types::DependentType;
use mojito::types::TransferSet;
use mojito::types::{FuncTy, GenericFuncTy};
use mojito::{CtValue, ParamDecl, Ty, TyArg};
use std::collections::HashMap;

fn function(blocks: Vec<MirBlock>, n_regs: u32, reg_types: &[(u32, Ty)]) -> MirFunction {
    MirFunction {
        blocks,
        n_regs,
        n_vars: 2,
        var_names: vec!["x".to_string(), "$try_error0".to_string()],
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
        error_slot: None,
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

/// [`lower_source`] with the bundled standard library linked, for a program
/// whose operators or traits are library declarations (`Int`'s `+` is
/// `SIMD`'s dunder).
fn lower_linked_source(source: &str) -> MirProgram {
    let linked =
        mojito::link_source(source, std::path::Path::new("verify_test.mojo")).expect("link");
    let program = mojito::elaborate(linked).expect("elaborate");
    let checked = check_program(&program).expect("check");
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
        .filter(|(name, _)| name == "main")
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
                origin: mojito::PointerOrigin::Static,
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

    let mut missing = lower_linked_source(SOURCE);
    first_index_call(&mut missing).arguments.clear();
    expect_finding(&missing, "declares 1 fixed parameter");

    let mut out_of_range = lower_linked_source(SOURCE);
    first_index_call(&mut out_of_range).arguments[0].source =
        mojito::checked::CheckedCallArgumentSource::Positional(1);
    expect_finding(&out_of_range, "positional source 1 is out of range");
    expect_finding(&out_of_range, "positional source 0 is represented 0 times");

    let mut duplicate = lower_linked_source(SOURCE);
    let extra = first_index_call(&mut duplicate).arguments[0].clone();
    first_index_call(&mut duplicate).arguments.push(extra);
    expect_finding(&duplicate, "positional source 0 is represented 2 times");
    expect_finding(&duplicate, "has no matching variadic collector");

    let mut missing_place_requirement = lower_linked_source(SOURCE);
    first_index_call(&mut missing_place_requirement).arguments[0].requires_place = false;
    expect_finding(
        &missing_place_requirement,
        "place requirement does not match",
    );

    let mut wrong_type = lower_linked_source(SOURCE);
    first_index_call(&mut wrong_type).arguments[0].parameter_ty = Ty::string_literal();
    expect_finding(&wrong_type, "subscript source has type Int");
    expect_finding(
        &wrong_type,
        "does not match 'Box.__getitem__' declaration Int",
    );

    let mut wrong_result = lower_linked_source(SOURCE);
    first_index_call(&mut wrong_result).result_ty = Ty::string_literal();
    expect_finding(&wrong_result, "subscript result has type Int");
    expect_finding(
        &wrong_result,
        "selected result type StringLiteral does not match 'Box.__getitem__' declaration Int",
    );

    let mut missing_receiver_place_requirement = lower_linked_source(SOURCE);
    first_index_call(&mut missing_receiver_place_requirement).receiver_requires_place = false;
    expect_finding(
        &missing_receiver_place_requirement,
        "receiver place requirement does not match",
    );

    let mut wrong_receiver_convention = lower_linked_source(SOURCE);
    first_index_call(&mut wrong_receiver_convention).receiver_convention =
        Some(mojito::ast::ArgConvention::Mut);
    expect_finding(
        &wrong_receiver_convention,
        "subscript receiver convention Some(Mut)",
    );

    let mut wrong_argument_convention = lower_linked_source(SOURCE);
    first_index_call(&mut wrong_argument_convention).arguments[0].convention =
        Some(mojito::ast::ArgConvention::Imm);
    expect_finding(
        &wrong_argument_convention,
        "subscript parameter 0 convention Some(Imm)",
    );

    let mut receiverless_declaration = lower_linked_source(SOURCE);
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
    let source = "struct Columns:\n    var n: Int\n    def __init__(out self, start: NoneType, end: NoneType, stride: NoneType, __slice_literal__: NoneType = None):\n        self.n = 0\n\n@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) -> Int:\n        return self.value\n    def __getitem__(self, columns: Columns) -> Int:\n        return self.value\n    def __getitem__(self, row: Int, column: Int) -> Int:\n        return self.value\n\ndef main():\n    var access = Access(1)\n    print(access[0])\n    print(access[:])\n    print(access[0, 0])\n";
    let mut mir = lower_source(source);
    for instruction in mir
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
    {
        match instruction {
            MirInstr::Index { call, .. } | MirInstr::MultiIndex { call, .. } => *call = None,
            _ => {}
        }
    }
    expect_finding(&mir, "index operation must carry exactly one");
    expect_finding(&mir, "multi-index operation lacks a checked call contract");
}

#[test]
fn verifier_rejects_missing_conflicting_and_mistyped_intrinsic_subscripts() {
    // A lane read is `simd.extract`; a pointer subscript of the vector is
    // rejected.
    let lane_read_as = |intrinsic: MirIntrinsicSubscript| {
        let mut mir = lower_linked_source(
            "def main():\n    var lanes = SIMD[DType.int, 2](1, 2)\n    print(Int(mlir_value=__mlir_op.`pop.simd.extractelement`(lanes._mlir_value, Int(0).__mlir_index__())))\n",
        );
        let instruction = mir
            .functions
            .iter_mut()
            .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
            .find(|instruction| matches!(instruction, MirInstr::SimdExtract { .. }))
            .expect("SIMD lane read lowered");
        let MirInstr::SimdExtract { dest, value, index } = *instruction else {
            unreachable!();
        };
        *instruction = MirInstr::Index {
            dest,
            base: value,
            index,
            base_place: None,
            index_place: None,
            call: None,
            intrinsic: Some(intrinsic),
        };
        mir
    };
    expect_finding(
        &lane_read_as(MirIntrinsicSubscript::Pointer),
        "Pointer is incompatible with checked base type",
    );

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
fn verifier_types_simd_lane_instructions() {
    // `main` spells the op, so its operands are closed (`SIMD.select`'s own
    // body is generic over its lanes).
    let mut mir = lower_linked_source(
        "def main():\n    var v = SIMD[DType.int32, 4](1, 2, 3, 4)\n    var m = v.gt(2)\n    print(SIMD[DType.int32, 4](mlir_value=__mlir_op.`pop.simd.select`(m._mlir_value, v._mlir_value, v._mlir_value)))\n",
    );
    let select = mir
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find(|instruction| matches!(instruction, MirInstr::SimdSelect { .. }))
        .expect("SIMD select lowered");
    if let MirInstr::SimdSelect { mask, on_true, .. } = select {
        *mask = *on_true;
    }
    expect_finding(&mir, "simd.select mask has DType.int32 lanes, not bool");
}

#[test]
fn verifier_types_select() {
    let mut mir = lower_source(
        "def main():\n    var c = 3 > 2\n    print(__mlir_op.`pop.select`(c.__mlir_bool__(), Int(1), Int(2)))\n",
    );
    let select = mir
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find(|instruction| matches!(instruction, MirInstr::Select { .. }))
        .expect("select lowered");
    if let MirInstr::Select {
        condition, on_true, ..
    } = select
    {
        *condition = *on_true;
    }
    expect_finding(
        &mir,
        "select condition has type Int, not a one-lane bool vector",
    );
}

#[test]
fn verifier_checks_llvm_intrinsic_calls() {
    let lowered = || {
        lower_source(
            "def main():\n    var x = Int32(7)\n    print(__mlir_op.`pop.call_llvm_intrinsic`[intrin=\"llvm.ctpop\".value, _type=Int32, hasSideEffects=False.__mlir_i1__()](x))\n",
        )
    };
    let with_call = |edit: &dyn Fn(&mut MirInstr)| {
        let mut mir = lowered();
        let call = mir
            .functions
            .iter_mut()
            .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
            .find(|instruction| matches!(instruction, MirInstr::CallLlvmIntrinsic { .. }))
            .expect("LLVM intrinsic call lowered");
        edit(call);
        mir
    };
    let concrete_finding = |mir: &MirProgram, needle: &str| {
        let errors = mojito::mir::verify::verify_concrete(mir);
        assert!(
            errors.iter().any(|error| error.contains(needle)),
            "expected a finding containing '{needle}', got {errors:?}"
        );
    };
    assert!(
        !mojito::mir::verify::verify_concrete(&lowered())
            .iter()
            .any(|error| error.contains("intrinsic")),
        "a closed call in the table is concrete"
    );
    concrete_finding(
        &with_call(&|call| {
            if let MirInstr::CallLlvmIntrinsic { intrinsic, .. } = call {
                *intrinsic = Const::Value(CtValue::KgenString("llvm.sin".to_string()));
            }
        }),
        "which is no LLVM intrinsic Mojito computes",
    );
    concrete_finding(
        &with_call(&|call| {
            if let MirInstr::CallLlvmIntrinsic { spread, .. } = call {
                *spread = Some(0);
            }
        }),
        "keeps an LLVM intrinsic call's pack spread",
    );
    expect_finding(
        &with_call(&|call| {
            if let MirInstr::CallLlvmIntrinsic { spread, .. } = call {
                *spread = Some(0);
            }
        }),
        "llvm.intrinsic spreads the non-pack Int32",
    );
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
    let source = "struct Columns:\n    var n: Int\n    def __init__(out self, start: NoneType, end: NoneType, stride: NoneType, __slice_literal__: NoneType = None):\n        self.n = 0\n\n@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) raises -> Int:\n        raise Error(\"index\")\n    def __getitem__(self, columns: Columns) raises -> Int:\n        raise Error(\"slice\")\n    def __getitem__(self, row: Int, column: Int) raises -> Int:\n        raise Error(\"multi\")\n    def __setitem__(mut self, row: Int, column: Int, value: Int) raises:\n        raise Error(\"set\")\n\ndef main() raises:\n    var access = Access(0)\n    print(access[0])\n    print(access[:])\n    print(access[0, 0])\n    access[0, 0] = 1\n";
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
fn verifier_checks_constant_array_element_projections() {
    let array = |index: usize, element: Ty| {
        let mut place = MirPlace::root(
            0,
            Some(
                TyKind::PopArray {
                    size: mojito::types::ArraySize::Known(2),
                    element: Ty::int(),
                }
                .into(),
            ),
        );
        place.project(Proj::ConstIndex(index), element.clone());
        program(function(
            vec![block(
                vec![MirInstr::LoadPlace {
                    dest: Reg(0),
                    place,
                }],
                MirTerm::Return(None),
            )],
            1,
            &[(0, element)],
        ))
    };
    let errors = verify(&array(1, Ty::int()));
    assert!(
        !errors.iter().any(|error| error.contains("array element")),
        "{errors:?}"
    );
    expect_finding(&array(2, Ty::int()), "projects array element 2 out of 2");
    expect_finding(
        &array(0, Ty::bool()),
        "place array element 0 typed Bool, declared Int",
    );
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

    // An `Int` is a width-one `SIMD`, whose lane a subscript reads.
    let mut non_indexed = MirPlace::root(0, Some(Ty::bool()));
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
fn verifier_rejects_a_top_level_try_yield() {
    let f = function(
        vec![block(Vec::new(), MirTerm::TryYield(Vec::new()))],
        0,
        &[],
    );
    expect_finding(&program(f), "`try.yield` outside the region it ends");
}

#[test]
fn verifier_rejects_a_non_bool_if_condition() {
    let arm = || vec![block(Vec::new(), MirTerm::Yield(Vec::new()))];
    let f = function(
        vec![block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                },
                MirInstr::If {
                    results: Vec::new(),
                    cond: Reg(0),
                    then: arm(),
                    elifs: Vec::new(),
                    else_args: Vec::new(),
                    orelse: arm(),
                },
            ],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "`if` condition has type Int");
}

#[test]
fn verifier_rejects_a_body_of_two_blocks() {
    let f = function(
        vec![
            block(Vec::new(), MirTerm::Return(None)),
            block(Vec::new(), MirTerm::Return(None)),
        ],
        0,
        &[],
    );
    expect_finding(&program(f), "body holds 2 blocks");
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
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Str("boom".to_string()),
            }],
            MirTerm::ErrorReturn,
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

// Pins the abstract-dispatch verify witness via the raw seam (schema-freeze
// survivor); under the authoritative Compiler this shape is retained-template
// residue.
#[test]
fn verifier_rejects_missing_or_value_step_iterator_result_adapters() {
    let source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    let mut missing = lower_linked_source(source);
    let call = missing
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "first")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                result_adapter,
                ..
            } if method == "__next__" => Some(result_adapter),
            _ => None,
        })
        .expect("abstract iterator next");
    *call = None;
    expect_finding(
        &missing,
        "abstract value-returning __next__ call lacks its copy-reference adapter",
    );

    let mut concrete = lower_linked_source(source);
    let call = concrete
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "first")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.blocks))
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                resolved: Some(target),
                ..
            } if method == "__next__" => Some(target),
            _ => None,
        })
        .expect("abstract iterator next");
    *call = "IntRefIter.__len__".to_string();
    expect_finding(
        &concrete,
        "copy-reference result adapter is not attached to a value-yielding __next__ call",
    );
    expect_finding(
        &concrete,
        "method-call result ABI does not match 'IntRefIter.__len__'",
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
                    receiver_convention: None,
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: None,
                    refined_bound: None,
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
                    receiver_convention: None,
                    arg_places: vec![None],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: Some(0),
                    refined_bound: None,
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
        5,
        &[
            (0, Ty::int()),
            (1, Ty::bool()),
            (2, Ty::int()),
            (3, Ty::bool()),
            (4, Ty::int()),
        ],
    );
    verify(&program(f))
        .into_iter()
        .filter(|finding| finding.contains("that some path reaching it does not hold"))
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

/// A raise point: a raising call and its error region, whose `then`
/// region stores the call's error into [`try_region`]'s error slot, `$v1`,
/// and raises to it.
fn raising_call() -> Vec<MirInstr> {
    vec![
        MirInstr::Call {
            dest: Reg(2),
            func: FuncRef::named("boom"),
            raises: Some(Raises {
                ty: Ty::int(),
                flag: Reg(3),
                error: Reg(4),
            }),
            args: Vec::new(),
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            param_arg_regs: Vec::new(),
            capture_accesses: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        },
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(3),
            then: vec![block(
                vec![MirInstr::DefVar {
                    var: 1,
                    src: Reg(4),
                    binding_ty: Some(Ty::int()),
                    in_place: false,
                }],
                MirTerm::TryRaise {
                    label: TRY,
                    values: Vec::new(),
                },
            )],
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: vec![block(Vec::new(), MirTerm::Yield(Vec::new()))],
        },
    ]
}

/// The label of [`try_region`]'s `try`, which [`raising_call`] raises to.
const TRY: TryLabel = TryLabel(0);

/// A `try` labelled [`TRY`] around `body`, with `handler` as its handler.
const fn try_region(body: Vec<MirBlock>, handler: Option<Vec<MirBlock>>) -> MirInstr {
    MirInstr::Try {
        label: TRY,
        error: Some(1),
        results: Vec::new(),
        body,
        handler_args: Vec::new(),
        handler,
        else_args: Vec::new(),
        orelse: None,
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
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
    let consumed = [define_x(), vec![DROP_X, MirInstr::ConsumeVar { var: 0 }]].concat();
    let findings = slot_end_findings(vec![block(consumed, MirTerm::Return(None))]);
    assert!(
        findings[0].contains("var.consume of slot $v0"),
        "{findings:?}"
    );
    let moved = || vec![block(vec![move_x()], MirTerm::Yield(Vec::new()))];
    let both_arms = vec![block(
        [define_x(), vec![if_region(moved(), moved()), DROP_X]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(both_arms).len(), 1);
}

/// A slot one path leaves empty — ended in one arm, or stored in one arm
/// only after a `var.declare` — is not held on every path, as upstream's
/// `UninitializedValueScan` rejects a destroy of a value some path has not
/// initialized.
#[test]
fn verifier_rejects_a_drop_of_a_slot_some_path_does_not_hold() {
    let one_arm = vec![block(
        [
            define_x(),
            vec![
                if_region(
                    vec![block(vec![move_x()], MirTerm::Yield(Vec::new()))],
                    vec![block(Vec::new(), MirTerm::Yield(Vec::new()))],
                ),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(
        slot_end_findings(one_arm),
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
    let declared = vec![block(
        [
            define_x(),
            vec![
                MirInstr::DeclareVar { var: 0 },
                if_region(yields(store_x()), yields(Vec::new())),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(declared).len(), 1);
    let never_stored = vec![block(vec![DROP_X], MirTerm::Return(None))];
    assert_eq!(slot_end_findings(never_stored).len(), 1);
}

/// The findings of `blocks` about owned registers ended twice, in a
/// function whose parameter `x` and local `y` hold a struct `S` whose drop
/// runs code; `%r0` is `S` and `%r1` is `Bool`.
fn register_end_findings(blocks: Vec<MirBlock>) -> Vec<String> {
    register_end_findings_in(blocks, &[], MirDeclarations::default())
}

/// [`register_end_findings`] over `declarations`, the registers `more`
/// typed besides `%r0` and `%r1`.
fn register_end_findings_in(
    blocks: Vec<MirBlock>,
    more: &[(u32, Ty)],
    declarations: MirDeclarations,
) -> Vec<String> {
    owned_register_findings(blocks, more, declarations)
        .into_iter()
        .filter(|finding| finding.contains("ends owned register"))
        .collect()
}

/// The findings of `blocks` about owned registers read after their end,
/// in [`register_end_findings`]' function; `%r2` is `S` too.
fn register_read_findings(blocks: Vec<MirBlock>) -> Vec<String> {
    let s = Ty::new(TyKind::Struct("S".into(), Vec::new()));
    owned_register_findings(blocks, &[(2, s)], MirDeclarations::default())
        .into_iter()
        .filter(|finding| finding.contains("reads owned register"))
        .collect()
}

/// Every verifier finding of `blocks` in [`register_end_findings`]'
/// function.
fn owned_register_findings(
    blocks: Vec<MirBlock>,
    more: &[(u32, Ty)],
    declarations: MirDeclarations,
) -> Vec<String> {
    let s = Ty::new(TyKind::Struct("S".into(), Vec::new()));
    let mut types = vec![(0, s.clone()), (1, Ty::bool())];
    types.extend_from_slice(more);
    let n_regs = types.iter().map(|(reg, _)| reg + 1).max().unwrap_or(0);
    let mut f = function(blocks, n_regs, &types);
    f.n_vars = 2;
    f.var_names = vec!["x".to_string(), "y".to_string()];
    f.n_params = 1;
    f.param_types = vec![s.clone()];
    f.owned_params = vec![false];
    f.deinit_params = vec![false];
    f.ref_params = vec![false];
    f.var_tys = HashMap::from([(0, s.clone()), (1, s)]);
    f.ret_ty = Some(Ty::none());
    let mut program = program(f);
    program.declarations = declarations;
    verify(&program)
}

/// `%r0`, an owned copy of `x`, and `%r1`, a `Bool` condition.
fn copy_x_and_condition() -> Vec<MirInstr> {
    vec![
        MirInstr::UseVar {
            dest: Reg(0),
            var: 0,
            mode: UseMode::Copy,
            constructor: None,
        },
        MirInstr::Const {
            dest: Reg(1),
            k: Const::Bool(true),
        },
    ]
}

const STORE_R0_IN_Y: MirInstr = MirInstr::DefVar {
    var: 1,
    src: Reg(0),
    binding_ty: None,
    in_place: false,
};

/// An owned register ends once on every path, as upstream's
/// `CheckLifetimes` rejects a use of an owned SSA value after its
/// consumption: a register stored once, or once in each arm, ends once,
/// and one stored twice ends twice.
#[test]
fn verifier_rejects_an_owned_register_ended_twice() {
    let once = [copy_x_and_condition(), vec![STORE_R0_IN_Y]].concat();
    assert!(register_end_findings(vec![block(once, MirTerm::Return(None))]).is_empty());
    let each_arm = [
        copy_x_and_condition(),
        vec![if_region(
            yields(vec![STORE_R0_IN_Y]),
            yields(vec![STORE_R0_IN_Y]),
        )],
    ]
    .concat();
    assert!(register_end_findings(vec![block(each_arm, MirTerm::Return(None))]).is_empty());
    let twice = [copy_x_and_condition(), vec![STORE_R0_IN_Y, STORE_R0_IN_Y]].concat();
    assert_eq!(
        register_end_findings(vec![block(twice, MirTerm::Return(None))]),
        [
            "MIR function 'test' block 0: var.store ends owned register %r0, which some path reaching it has ended"
        ]
    );
    let after_one_arm = [
        copy_x_and_condition(),
        vec![
            if_region(yields(vec![STORE_R0_IN_Y]), yields(Vec::new())),
            MirInstr::Drop {
                reg: Reg(0),
                destructor: None,
            },
        ],
    ]
    .concat();
    assert_eq!(
        register_end_findings(vec![block(after_one_arm, MirTerm::Return(None))]),
        [
            "MIR function 'test' block 0: drop.reg ends owned register %r0, which some path reaching it has ended"
        ]
    );
}

/// An owned register stored into a slot is not read again, as upstream's
/// `CheckLifetimes` rejects a use of an owned SSA value after its
/// consumption: a read before the store, or of the slot after it, is
/// accepted, and a read of the register after it, on one path or both, is
/// rejected.
#[test]
fn verifier_rejects_a_read_of_an_ended_owned_register() {
    let copy_r0 = MirInstr::CopyValue {
        dest: Reg(2),
        value: Reg(0),
        constructor: None,
    };
    let load_y = MirInstr::LoadPlace {
        dest: Reg(2),
        place: MirPlace::root(1, Some(Ty::new(TyKind::Struct("S".into(), Vec::new())))),
    };
    let before = [copy_x_and_condition(), vec![copy_r0.clone(), STORE_R0_IN_Y]].concat();
    assert!(register_read_findings(vec![block(before, MirTerm::Return(None))]).is_empty());
    let slot = [copy_x_and_condition(), vec![STORE_R0_IN_Y, load_y]].concat();
    assert!(register_read_findings(vec![block(slot, MirTerm::Return(None))]).is_empty());
    let after = [copy_x_and_condition(), vec![STORE_R0_IN_Y, copy_r0.clone()]].concat();
    assert_eq!(
        register_read_findings(vec![block(after, MirTerm::Return(None))]),
        [
            "MIR function 'test' block 0: value.copy reads owned register %r0, which some path reaching it has ended"
        ]
    );
    let after_one_arm = [
        copy_x_and_condition(),
        vec![
            if_region(yields(vec![STORE_R0_IN_Y]), yields(Vec::new())),
            copy_r0,
        ],
    ]
    .concat();
    assert_eq!(
        register_read_findings(vec![block(after_one_arm, MirTerm::Return(None))]),
        [
            "MIR function 'test' block 0: value.copy reads owned register %r0, which some path reaching it has ended"
        ]
    );
}

const DROP_R0: MirInstr = MirInstr::Drop {
    reg: Reg(0),
    destructor: None,
};

/// An owned argument a call through a trait bound passes to a `var`
/// parameter of the requirement ends there, as upstream's call reads the
/// requirement's convention, so a later `drop.reg` of it ends it twice.
#[test]
fn verifier_rejects_a_drop_of_an_argument_a_bound_call_took() {
    let dispatch = "__trait_dispatch.absorb";
    let declarations = MirDeclarations {
        traits: vec![TraitDeclaration {
            name: "Absorber".into(),
            refines: Vec::new(),
            requirements: vec![TraitRequirement {
                name: "absorb".into(),
                arity: 1,
                dispatch: dispatch.into(),
                param_names: vec!["t".into()],
                param_types: vec![Ty::new(TyKind::Struct("S".into(), Vec::new()))],
                param_conventions: vec![Some(mojito::ast::ArgConvention::Var)],
                variadic_index: None,
                required: vec![true],
                positional_only: None,
                keyword_only: None,
                variadic: None,
                kw_variadic: None,
                variadic_convention: None,
            }],
        }],
        ..MirDeclarations::default()
    };
    let bound = Ty::new(TyKind::Param {
        binder: ParamRef {
            id: ParamId::new("test", 0),
            name: "T".into(),
        },
        bounds: vec!["Absorber".into()],
        callable_bound: None,
    });
    let absorb = MirInstr::MethodCall {
        dest: Reg(3),
        recv: Reg(2),
        method: "absorb".into(),
        resolved: Some(dispatch.into()),
        raises: None,
        reference_result: None,
        result_adapter: None,
        args: vec![Reg(0)],
        kwargs: Vec::new(),
        recv_place: None,
        recv_writes: false,
        receiver_convention: None,
        arg_places: vec![None],
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
        instantiated_args: Vec::new(),
        spread: None,
        refined_bound: None,
    };
    let instrs = [copy_x_and_condition(), vec![absorb, DROP_R0]].concat();
    assert_eq!(
        register_end_findings_in(
            vec![block(instrs, MirTerm::Return(None))],
            &[(2, bound), (3, Ty::none())],
            declarations,
        ),
        [
            "MIR function 'test' block 0: drop.reg ends owned register %r0, which some path reaching it has ended"
        ]
    );
}

/// A pack over a shallow read is borrowed, upstream's non-owned
/// `VariadicPack` of references: it takes no element, so its owned element
/// is still the caller's to destroy, where an owned pack takes it.
#[test]
fn verifier_accepts_a_drop_of_an_element_a_borrowed_pack_holds() {
    let s = Ty::new(TyKind::Struct("S".into(), Vec::new()));
    let load_y = MirInstr::LoadPlace {
        dest: Reg(2),
        place: MirPlace::root(1, Some(s.clone())),
    };
    let tuple = |elems| MirInstr::MakeTuple {
        dest: Reg(3),
        elems,
        element_types: None,
    };
    let borrowed = [
        copy_x_and_condition(),
        vec![load_y, tuple(vec![Reg(2), Reg(0)]), DROP_R0],
    ]
    .concat();
    let pair = Ty::new(TyKind::Tuple(vec![s.clone(), s.clone()]));
    assert!(
        register_end_findings_in(
            vec![block(borrowed, MirTerm::Return(None))],
            &[(2, s.clone()), (3, pair)],
            MirDeclarations::default(),
        )
        .is_empty()
    );
    let owned = [copy_x_and_condition(), vec![tuple(vec![Reg(0)]), DROP_R0]].concat();
    assert_eq!(
        register_end_findings_in(
            vec![block(owned, MirTerm::Return(None))],
            &[(3, Ty::new(TyKind::Tuple(vec![s])))],
            MirDeclarations::default(),
        ),
        [
            "MIR function 'test' block 0: drop.reg ends owned register %r0, which some path reaching it has ended"
        ]
    );
}

/// `x` stored from r0.
fn store_x() -> Vec<MirInstr> {
    vec![MirInstr::DefVar {
        var: 0,
        src: Reg(0),
        binding_ty: Some(Ty::int()),
        in_place: false,
    }]
}

/// A slot re-stored after a move, stored in both arms after a
/// `var.declare`, or still held where a region raises is held on every
/// path.
#[test]
fn verifier_accepts_a_drop_of_a_slot_every_path_holds() {
    let both_stored = vec![block(
        [
            define_x(),
            vec![
                MirInstr::DeclareVar { var: 0 },
                if_region(yields(store_x()), yields(store_x())),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(both_stored), Vec::<String>::new());
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
    // The body moves `x` after its last raise, so its handler still owns it.
    let body = vec![block(
        [raising_call(), vec![move_x()]].concat(),
        MirTerm::TryYield(Vec::new()),
    )];
    let handler = vec![block(vec![DROP_X], MirTerm::TryYield(Vec::new()))];
    let raising = vec![block(
        [define_x(), vec![try_region(body, Some(handler))]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(raising), Vec::<String>::new());
}

/// The part-end findings `verify` reports for a function over slot 0
/// (`x`) and the registers of [`slot_end_findings`].
fn part_end_findings(blocks: Vec<MirBlock>) -> Vec<String> {
    let f = function(
        blocks,
        5,
        &[
            (0, Ty::int()),
            (1, Ty::bool()),
            (2, Ty::int()),
            (3, Ty::bool()),
            (4, Ty::int()),
        ],
    );
    verify(&program(f))
        .into_iter()
        .filter(|finding| finding.contains("some path reaching it has ended"))
        .collect()
}

/// The place `x.<field>`.
fn x_field(field: &str) -> MirPlace {
    let mut place = MirPlace::root(0, Some(Ty::int()));
    place.project(Proj::Field(field.to_string()), Ty::int());
    place
}

/// `x.<field>` moved into r2.
fn move_x_field(field: &str) -> MirInstr {
    MirInstr::MovePlace {
        dest: Reg(2),
        place: x_field(field),
    }
}

/// A destroy of a value a part of which some path has ended — a whole
/// `drop.var`, or a `drop.place` of that part — destroys the part twice,
/// as upstream's `ValueSet` rejects a destroy of a value with an
/// uninitialized field.
#[test]
fn verifier_rejects_a_destroy_after_a_part_end() {
    let whole = [define_x(), vec![move_x_field("a"), DROP_X]].concat();
    assert_eq!(
        part_end_findings(vec![block(whole, MirTerm::Return(None))]),
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') whose part `a` some path reaching it has ended"
        ]
    );
    let one_arm = vec![block(
        [
            define_x(),
            vec![
                if_region(yields(vec![move_x_field("a")]), yields(Vec::new())),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(part_end_findings(one_arm).len(), 1);
    let part = [
        define_x(),
        vec![
            move_x_field("a"),
            MirInstr::DropPlace {
                place: x_field("a"),
                destructor: None,
            },
        ],
    ]
    .concat();
    let findings = part_end_findings(vec![block(part, MirTerm::Return(None))]);
    assert!(
        findings[0].contains("drop.place of slot $v0"),
        "{findings:?}"
    );
}

/// A part stored again before the whole drop, a `var.consume` (which
/// destroys nothing) after a part's end, and a drop of a part beside an
/// ended one find every part they destroy held.
#[test]
fn verifier_accepts_a_destroy_of_every_part_held() {
    let refilled = [
        define_x(),
        vec![
            move_x_field("a"),
            MirInstr::Store {
                place: x_field("a"),
                src: Reg(0),
            },
            DROP_X,
        ],
    ]
    .concat();
    assert_eq!(
        part_end_findings(vec![block(refilled, MirTerm::Return(None))]),
        Vec::<String>::new()
    );
    let consumed = [
        define_x(),
        vec![
            move_x_field("a"),
            MirInstr::DropPlace {
                place: x_field("b"),
                destructor: None,
            },
            MirInstr::ConsumeVar { var: 0 },
        ],
    ]
    .concat();
    assert_eq!(
        part_end_findings(vec![block(consumed, MirTerm::Return(None))]),
        Vec::<String>::new()
    );
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

/// A slot some path through a region op ends is not held after it: an
/// `if` whose arms both move it, or one arm, a loop whose every `break`
/// follows a move, one leaving through a `try`, a loop whose second
/// iteration drops what its first dropped, and a `match` whose completing
/// case and else region both move it. A `try` in an arm whose handler
/// drops a slot its body ended before its raise point is the same finding.
#[test]
fn verifier_rejects_a_drop_of_a_slot_a_region_op_ended() {
    let both_arms = if_region(yields(vec![move_x()]), yields(vec![move_x()]));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![both_arms])),
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
    let looped = loop_region(breaks(vec![move_x()]));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![looped])).len(),
        1
    );
    let crossing = loop_region(region(
        vec![try_region(breaks(vec![move_x()]), None)],
        MirTerm::Unreachable,
    ));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![crossing])).len(),
        1
    );
    let one_arm = if_region(yields(vec![move_x()]), yields(Vec::new()));
    assert_eq!(
        slot_end_findings(defined_then_dropped(vec![one_arm])).len(),
        1
    );
    let leave = if_region(yields(Vec::new()), breaks(Vec::new()));
    let redropped = loop_region(region(
        vec![leave, DROP_X],
        MirTerm::Continue {
            label: LOOP,
            values: Vec::new(),
        },
    ));
    let redropped = vec![block(
        [define_x(), vec![redropped]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(redropped).len(), 1);
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
    let handled = try_region(
        region(
            [vec![DROP_X], raising_call()].concat(),
            MirTerm::TryYield(Vec::new()),
        ),
        Some(region(vec![DROP_X], MirTerm::TryYield(Vec::new()))),
    );
    let in_arm = if_region(yields(vec![handled]), yields(Vec::new()));
    let findings = slot_end_findings(vec![block(
        [define_x(), vec![in_arm]].concat(),
        MirTerm::Return(None),
    )]);
    assert_eq!(
        findings,
        [
            "MIR function 'test' block 0: drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
}

/// A slot every path through a region op holds is held after it: a loop
/// whose body stores it before dropping it each iteration, and a `match`
/// whose `match.next` path leaves it for the else region.
#[test]
fn verifier_accepts_a_drop_of_a_slot_a_region_op_holds() {
    let leave = if_region(yields(Vec::new()), breaks(Vec::new()));
    let looped = loop_region(region(
        [store_x(), vec![leave, DROP_X]].concat(),
        MirTerm::Continue {
            label: LOOP,
            values: Vec::new(),
        },
    ));
    let refilled = vec![block(
        [define_x(), vec![looped]].concat(),
        MirTerm::Return(None),
    )];
    assert_eq!(slot_end_findings(refilled), Vec::<String>::new());
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
