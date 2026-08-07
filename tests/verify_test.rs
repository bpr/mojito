//! Phase — standalone typed-MIR verification tests.
//!
//! Positive per-fixture coverage lives in `tests/corpus_test.rs` (`verify::*`),
//! which lowers every executable fixture and requires complete register/place
//! typing with no invariant errors, before and after drop elaboration.
//! Negative coverage here (hand-built malformed MIR) pins each verifier check
//! class.

use mojito::mir::verify::verify;
use mojito::{check_program, parse};
use std::path::Path;

// --- Negative coverage: hand-built malformed MIR per verifier check class ---

use mojito::ct::CtExpr;
use mojito::mir::{
    Const, FuncRef, MirBlock, MirDeclarations, MirFunction, MirInstr, MirInteriorOrigin,
    MirIntrinsicSubscript, MirLoan, MirParamArg, MirPlace, MirProgram, MirTerm, Proj, Reg,
    SpanTable,
};
use mojito::types::DependentType;
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
        ret_ty: Some(Ty::Int),
        raises: false,
        error_ty: None,
        spans: SpanTable::default(),
        reg_types: reg_types.iter().cloned().collect(),
    }
}

fn program(f: MirFunction) -> MirProgram {
    MirProgram {
        functions: vec![("test".to_string(), f)],
        declarations: MirDeclarations::default(),
        invariant_errors: Vec::new(),
    }
}

fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
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
    Ty::Ref(mojito::RefTy {
        referent: Box::new(referent),
        origin: mojito::Origin::Untracked {
            mutable: !matches!(mutability, mojito::Mutability::Immutable),
        },
        mutability,
    })
}

fn tracked_pointer_ty(element: Ty, mutable: bool) -> Ty {
    Ty::Pointer {
        element: Box::new(element),
        origin: mojito::PointerOrigin::Place {
            place: mojito::OriginPlace {
                root: mojito::OwnerId(0),
                path: Vec::new(),
            },
            mutable,
        },
    }
}

fn first_index_call(program: &mut MirProgram) -> &mut mojito::mir::MirSubscriptCall {
    program
        .functions
        .iter_mut()
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
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
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1
        .blocks
        .iter_mut()
        .flat_map(|block| block.instrs.iter_mut())
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
    let mir = mojito::mir::lower_checked_program(compiled.checked());
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
        .find_map(|(slot, ty)| (!matches!(ty, Ty::Ref(_))).then_some(*slot))
        .expect("ordinary local slot");
    projected_reference_loan_place(&mut non_reference_through).through = Some(non_reference_slot);
    expect_finding(
        &non_reference_through,
        "is not a checked reference-capability binding",
    );

    let mut wrong_root_type = lower_source(source);
    projected_reference_loan_place(&mut wrong_root_type).root_ty = Some(Ty::Int);
    expect_finding(&wrong_root_type, "place root slot");

    let mut wrong_field_type = lower_source(source);
    let place = projected_reference_loan_place(&mut wrong_field_type);
    let field = place
        .proj
        .iter()
        .position(|projection| matches!(projection, Proj::Field(name) if name == "value"))
        .expect("value projection");
    place.projection_tys[field] = Ty::String;
    expect_finding(&wrong_field_type, "field 'value'");
    expect_finding(&wrong_field_type, "typed String, declared Int");
}

#[test]
fn verifier_checks_make_ref_capability_types() {
    let place = MirPlace::root(0, Some(Ty::Int));

    let non_capability = function(
        vec![block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::Int)],
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
        &[(0, reference_ty(Ty::String, mojito::Mutability::Mutable))],
    );
    expect_finding(
        &program(wrong_referent),
        "MakeRef destination targets String, incompatible with place storage Int",
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
            Ty::Pointer {
                element: Box::new(Ty::Int),
                origin: mojito::PointerOrigin::Legacy,
            },
        )],
    );
    expect_finding(
        &program(raw_pointer),
        "MakeRef destination has non-reference-capability type UnsafePointer",
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
        &[(0, tracked_pointer_ty(Ty::Int, true))],
    );
    assert_eq!(verify(&program(tracked_pointer)), Vec::<String>::new());
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
        &[(0, Ty::Int), (1, Ty::Int)],
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
            (0, Ty::String),
            (1, reference_ty(Ty::Int, mojito::Mutability::Immutable)),
        ],
    );
    expect_finding(
        &program(read_wrong_referent),
        "ReadRef result type String is incompatible with referent Int",
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
            (0, reference_ty(Ty::Int, mojito::Mutability::Immutable)),
            (1, Ty::Int),
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
        &[(0, tracked_pointer_ty(Ty::Int, false)), (1, Ty::Int)],
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
        &[(0, tracked_pointer_ty(Ty::Int, true)), (1, Ty::String)],
    );
    expect_finding(
        &program(write_wrong_referent),
        "WriteRef value type String is incompatible with referent Int",
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
        &[(0, Ty::Int), (1, tracked_pointer_ty(Ty::Int, false))],
    );
    assert_eq!(verify(&program(read_pointer)), Vec::<String>::new());
}

#[test]
fn verifier_checks_store_ref_source_type_and_permission() {
    let mutable_int = reference_ty(Ty::Int, mojito::Mutability::Mutable);
    let immutable_int = reference_ty(Ty::Int, mojito::Mutability::Immutable);

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
        &program(store(Ty::Int)),
        "StoreRef source has non-reference type Int",
    );
    expect_finding(
        &program(store(reference_ty(Ty::String, mojito::Mutability::Mutable))),
        "StoreRef source referent String is incompatible with storage referent Int",
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
        name: name.to_string(),
        ty: Box::new(ty),
        default: None,
        callable_default: None,
        infer_only: false,
        variadic: false,
        constraints: Vec::new(),
    }
}

fn generic_callable(decls: Vec<ParamDecl>) -> Ty {
    Ty::GenericFunc {
        environment: mojito::CallableEnvironment::Thin,
        decls,
        params: Vec::new(),
        names: Vec::new(),
        ret: Box::new(Ty::Int),
        required: Vec::new(),
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: Vec::new(),
        ref_params: Box::new(Vec::new()),
        ref_return: None,
    }
}

fn dependent_generic_callable(index: &str) -> Ty {
    Ty::GenericFunc {
        environment: mojito::CallableEnvironment::Thin,
        decls: vec![value_parameter("index", Ty::Int)],
        params: vec![Ty::Dependent(DependentType::Indexed {
            elements: vec![Ty::Int, Ty::String],
            index: CtExpr::Param(index.to_string()),
        })],
        names: vec!["element".to_string()],
        ret: Box::new(Ty::None),
        required: vec![true],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: vec![None],
        ref_params: Box::new(vec![None]),
        ref_return: None,
    }
}

fn concrete_callable(parameter: Ty) -> Ty {
    Ty::Func {
        environment: mojito::CallableEnvironment::Thin,
        params: vec![parameter],
        names: vec!["element".to_string()],
        ret: Box::new(Ty::None),
        required: vec![true],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: vec![None],
        ref_params: Box::new(vec![None]),
        ref_return: None,
    }
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
                    base_place: Some(MirPlace::root(0, Some(Ty::String))),
                    index_place: None,
                    call: None,
                    intrinsic: Some(MirIntrinsicSubscript::Pointer),
                },
            ],
            MirTerm::Return(Some(Reg(2))),
        )],
        3,
        &[(0, Ty::Int), (1, Ty::Int), (2, Ty::Int)],
    );
    expect_finding(&program(f), "subscript receiver place type String");
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
    first_index_call(&mut wrong_type).arguments[0].parameter_ty = Ty::String;
    expect_finding(&wrong_type, "subscript source has type Int");
    expect_finding(
        &wrong_type,
        "does not match 'Box.__getitem__' declaration Int",
    );

    let mut wrong_result = lower_source(SOURCE);
    first_index_call(&mut wrong_result).result_ty = Ty::String;
    expect_finding(&wrong_result, "subscript result has type Int");
    expect_finding(
        &wrong_result,
        "selected result type String does not match 'Box.__getitem__' declaration Int",
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
        Some(mojito::ast::ArgConvention::Read);
    expect_finding(
        &wrong_argument_convention,
        "subscript parameter 0 convention Some(Read)",
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
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
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
    let mut missing = lower_source("def main():\n    print(\"abc\"[1:])\n");
    missing
        .functions
        .iter_mut()
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
        .find_map(|instruction| match instruction {
            MirInstr::Slice { intrinsic, .. } => Some(intrinsic),
            _ => None,
        })
        .expect("String Slice lowered")
        .take();
    expect_finding(&missing, "slice operation must carry exactly one");

    let mut mistyped = lower_source(
        "def main():\n    var lanes = SIMD[DType.int, 2](1, 2)\n    print(lanes[0])\n",
    );
    let intrinsic = mistyped
        .functions
        .iter_mut()
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
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
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
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
    *first_index_call(&mut wrong_referent)
        .reference_result
        .as_mut()
        .expect("reference-returning contract")
        .referent = Ty::String;
    expect_finding(
        &wrong_referent,
        "reference-result referent String does not match 'Box.__getitem__' declaration Item",
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
        mojito::Mutability::Immutable => mojito::Mutability::Mutable,
        mojito::Mutability::Mutable => mojito::Mutability::Immutable,
        mojito::Mutability::Param(_) => mojito::Mutability::Mutable,
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
    let mut place = MirPlace::root(0, Some(Ty::Tuple(vec![Ty::Int, Ty::String])));
    place.project(Proj::ConstIndex(2), Ty::Int);
    let f = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(0),
                place,
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::Int)],
    );
    expect_finding(&program(f), "projects Tuple element 2 out of 2");
}

#[test]
fn verifier_rejects_malformed_dynamic_place_projections() {
    let pointer = Ty::Pointer {
        element: Box::new(Ty::Int),
        origin: mojito::PointerOrigin::Legacy,
    };

    let mut wrong_element = MirPlace::root(0, Some(pointer.clone()));
    wrong_element.project(Proj::Index(Reg(0)), Ty::String);
    let wrong_element = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_element,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::Int), (1, Ty::String)],
    );
    expect_finding(
        &program(wrong_element),
        "place dynamic element typed String",
    );

    let mut wrong_index = MirPlace::root(0, Some(pointer));
    wrong_index.project(Proj::Index(Reg(0)), Ty::Int);
    let wrong_index = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_index,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::String), (1, Ty::Int)],
    );
    expect_finding(&program(wrong_index), "has non-Indexer type String");

    let mut non_indexed = MirPlace::root(0, Some(Ty::Int));
    non_indexed.project(Proj::Index(Reg(0)), Ty::Int);
    let non_indexed = function(
        vec![block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: non_indexed,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::Int), (1, Ty::Int)],
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
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::None)],
    );
    expect_finding(&program(empty), "loan generation has no owner loans");

    let malformed = function(
        vec![block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: vec![MirLoan {
                    place: MirPlace::root(0, Some(Ty::Int)),
                    mutable: true,
                    interior: Some(MirInteriorOrigin {
                        root: 0,
                        path: vec![mojito::OriginSeg::Field("value".to_string())],
                    }),
                }],
                marker: Reg(0),
            }],
            MirTerm::Return(None),
        )],
        1,
        &[(0, Ty::None)],
    );
    expect_finding(&program(malformed), "has no interior segment");

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
        &[(0, Ty::None)],
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
        &[(0, Ty::Int)],
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
    let f = function(vec![block(Vec::new(), MirTerm::FallOff)], 0, &[]);
    expect_finding(&program(f), "FallOff terminator outside a try region");
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
        &[(0, Ty::Int)],
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
        &[(0, Ty::String)],
    );
    expect_finding(
        &program(f),
        "return of String from a function returning Int",
    );
}

#[test]
fn verifier_rejects_runtime_pack_types_in_function_body_slots() {
    let mut f = function(vec![block(Vec::new(), MirTerm::Return(None))], 0, &[]);
    f.n_params = 1;
    f.param_types = vec![Ty::RuntimePack(vec![Ty::Int, Ty::String])];
    f.var_tys
        .insert(0, Ty::RuntimePack(vec![Ty::Int, Ty::String]));
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
        &[(0, Ty::String)],
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
                },
            ],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::String), (1, Ty::Int)],
    );
    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "callee".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![Ty::Int],
        defaults: vec![None],
        required: vec![true],
        variadic: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: false,
        receiver_convention: None,
        param_conventions: vec![None],
        ret_ty: Ty::Int,
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: vec![false],
    });
    expect_finding(
        &prog,
        "argument 0 of 'callee' has type String, declared Int",
    );
    prog.declarations.functions[0].param_conventions.clear();
    expect_finding(&prog, "has 0 parameter conventions, expected 1");
}

#[test]
fn verifier_rejects_malformed_direct_compile_time_arguments() {
    use mojito::mir::MirFunctionDeclaration;
    let declaration = value_parameter("n", Ty::Int);
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
                }],
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::String), (1, Ty::Int)],
    );
    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "callee".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: vec![declaration],
        has_receiver: false,
        receiver_convention: None,
        param_conventions: Vec::new(),
        ret_ty: Ty::Int,
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: Vec::new(),
    });
    expect_finding(&prog, "register type String, declared Int");

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
    let declarations = vec![value_parameter("n", Ty::Int)];
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
                }],
                param_decls: declarations,
                instantiated_contract: None,
                instantiated_args: Vec::new(),
            }],
            MirTerm::Return(None),
        )],
        3,
        &[(0, callable), (1, Ty::String), (2, Ty::Int)],
    );
    let mut prog = program(f);
    expect_finding(&prog, "register type String, declared Int");

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
    let declarations = vec![value_parameter("index", Ty::Int)];
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
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::Int)),
                instantiated_args: vec![TyArg::Val(CtValue::Int(0))],
            }],
            MirTerm::Return(None),
        )],
        4,
        &[
            (0, dependent_generic_callable("index")),
            (1, Ty::Int),
            (2, Ty::Int),
            (3, Ty::None),
        ],
    );
    let prog = program(f);
    assert_eq!(verify(&prog), Vec::<String>::new());
}

#[test]
fn verifier_rejects_corrupt_or_unbound_dependent_callable_indices() {
    let declarations = vec![value_parameter("index", Ty::Int)];
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
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::Int)),
                instantiated_args: vec![TyArg::Val(CtValue::Int(1))],
            }],
            MirTerm::Return(None),
        )],
        4,
        &[
            (0, dependent_generic_callable("index")),
            (1, Ty::Int),
            (2, Ty::Int),
            (3, Ty::None),
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
    let contract = Ty::Func {
        environment: mojito::CallableEnvironment::Thin,
        params: Vec::new(),
        names: Vec::new(),
        ret: Box::new(Ty::Int),
        required: Vec::new(),
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: true,
        error: Some(Box::new(Ty::String)),
        conventions: Vec::new(),
        ref_params: Box::new(Vec::new()),
        ref_return: Some(Box::new(mojito::origin::RefSig {
            origin: mojito::origin::SigOrigin::Static,
            mutability: mojito::origin::SigMutability::Mutable,
        })),
    };
    let result = Ty::Ref(mojito::RefTy {
        referent: Box::new(Ty::Int),
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
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, contract), (1, result)],
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
                },
            ],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::Int), (1, Ty::None)],
    );
    expect_finding(&program(f), "call place metadata is not aligned");
}

#[test]
fn verifier_rejects_a_mismatched_iterator_exhaustion_contract() {
    use mojito::mir::MirFunctionDeclaration;

    let other_error = Ty::Struct("OtherError".to_string(), Vec::new());
    let stop_iteration = Ty::Struct("StopIteration".to_string(), Vec::new());
    let mut f = function(
        vec![block(
            vec![MirInstr::TryNext {
                dest: Reg(0),
                yielded: Reg(1),
                iter: 0,
                call: mojito::checked::CheckedIteratorCall {
                    target: "Iterator.__next__".to_string(),
                    result_ty: Ty::Int,
                    reference_result: None,
                    raises: Some(other_error.clone()),
                    result_adapter: None,
                },
                exhaustion: other_error,
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, Ty::Int), (1, Ty::Bool)],
    );
    f.var_names[0] = "iterator".to_string();
    f.var_tys
        .insert(0, Ty::Struct("Iterator".to_string(), Vec::new()));

    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "Iterator.__next__".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: true,
        receiver_convention: Some(mojito::ast::ArgConvention::Mut),
        param_conventions: Vec::new(),
        ret_ty: Ty::Int,
        returns_reference: false,
        raises: true,
        error_ty: Some(stop_iteration),
        ref_params: Vec::new(),
    });

    expect_finding(&prog, "TryNext catches non-StopIteration type OtherError");
    expect_finding(&prog, "does not match 'Iterator.__next__' raising contract");
}

#[test]
fn verifier_rejects_an_iterator_reference_result_abi_mismatch() {
    use mojito::mir::MirFunctionDeclaration;

    let stop_iteration = Ty::Struct("StopIteration".to_string(), Vec::new());
    let result_ty = reference_ty(Ty::Int, mojito::Mutability::Immutable);
    let Ty::Ref(reference_result) = &result_ty else {
        unreachable!("reference_ty constructs Ty::Ref")
    };
    let mut f = function(
        vec![block(
            vec![MirInstr::TryNext {
                dest: Reg(0),
                yielded: Reg(1),
                iter: 0,
                call: mojito::checked::CheckedIteratorCall {
                    target: "Iterator.__next__".to_string(),
                    result_ty: result_ty.clone(),
                    reference_result: Some(reference_result.clone()),
                    raises: Some(stop_iteration.clone()),
                    result_adapter: None,
                },
                exhaustion: stop_iteration.clone(),
            }],
            MirTerm::Return(None),
        )],
        2,
        &[(0, result_ty), (1, Ty::Bool)],
    );
    f.var_names[0] = "iterator".to_string();
    f.var_tys
        .insert(0, Ty::Struct("Iterator".to_string(), Vec::new()));

    let mut prog = program(f);
    prog.declarations.functions.push(MirFunctionDeclaration {
        lowered_name: "Iterator.__next__".to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: Vec::new(),
        has_receiver: true,
        receiver_convention: Some(mojito::ast::ArgConvention::Mut),
        param_conventions: Vec::new(),
        ret_ty: Ty::Int,
        // Deliberately corrupt: the checked operation returns a reference, but
        // the retained declaration claims an owned-value ABI.
        returns_reference: false,
        raises: true,
        error_ty: Some(stop_iteration),
        ref_params: Vec::new(),
    });

    expect_finding(&prog, "TryNext result contract does not match");
}

// Pins the abstract-dispatch verify witness via the raw seam (schema-freeze
// survivor); under the authoritative Compiler this shape is retained-template
// residue.
#[test]
fn verifier_rejects_missing_or_concrete_iterator_result_adapters() {
    let source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    let mut missing = lower_source(source);
    let call = missing
        .functions
        .iter_mut()
        .find(|(name, _)| name == "first")
        .expect("generic first MIR")
        .1
        .blocks
        .iter_mut()
        .flat_map(|block| block.instrs.iter_mut())
        .find_map(|instruction| match instruction {
            MirInstr::Next {
                call: Some(call), ..
            }
            | MirInstr::TryNext { call, .. } => Some(call),
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
        .find(|(name, _)| name == "first")
        .expect("generic first MIR")
        .1
        .blocks
        .iter_mut()
        .flat_map(|block| block.instrs.iter_mut())
        .find_map(|instruction| match instruction {
            MirInstr::Next {
                call: Some(call), ..
            }
            | MirInstr::TryNext { call, .. } => Some(call),
            _ => None,
        })
        .expect("abstract iterator next");
    call.target = "IntRefIter.__next__".to_string();
    expect_finding(
        &concrete,
        "iterator copy-reference adapter is attached to concrete target",
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
        .find(|(name, _)| name == "take")
        .expect("generic take MIR")
        .1
        .blocks
        .iter_mut()
        .flat_map(|block| block.instrs.iter_mut())
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
    let result_ty = reference_ty(Ty::Int, mojito::Mutability::Immutable);
    let Ty::Ref(reference) = &result_ty else {
        unreachable!("reference_ty constructs Ty::Ref")
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
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                }],
                MirTerm::Return(None),
            )],
            2,
            &[
                (0, Ty::Struct("OpaqueIterator".to_string(), Vec::new())),
                (1, result_ty.clone()),
            ],
        )
    };

    // A value-returning contract can have a reference-shaped Element. Its ABI
    // still needs the adapter because ABI is not inferred from the value type.
    let value_abi = program(abstract_next(
        None,
        Some(mojito::checked::CheckedResultAdapter::CopyIteratorReference),
    ));
    assert_eq!(verify(&value_abi), Vec::<String>::new());

    // A genuine abstract reference ABI carries its reference metadata and does
    // not use the value-result adapter.
    let reference_abi = program(abstract_next(Some(reference.clone()), None));
    assert_eq!(verify(&reference_abi), Vec::<String>::new());
}

#[test]
fn verifier_rejects_corrupt_concrete_method_result_abis() {
    let reference_source = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def get(ref self) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef main():\n    var box = Box(1)\n    ref value = box.get()\n    print(value)\n";
    let mut erased = lower_source(reference_source);
    let reference_result = erased
        .functions
        .iter_mut()
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
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
        .flat_map(|(_, function)| function.blocks.iter_mut())
        .flat_map(|block| block.instrs.iter_mut())
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                reference_result,
                ..
            } if method == "get" => Some(reference_result),
            _ => None,
        })
        .expect("value-returning method call");
    *reference_result = match reference_ty(Ty::Int, mojito::Mutability::Immutable) {
        Ty::Ref(reference) => Some(reference),
        _ => unreachable!("reference_ty constructs Ty::Ref"),
    };
    expect_finding(
        &fabricated,
        "method-call result ABI does not match 'Box.get'",
    );
}
