//! Phase — standalone typed-MIR verification tests.
//!
//! Positive per-fixture coverage lives in `tests/corpus_test.rs` (`verify::*`),
//! which lowers every executable fixture and requires complete register/place
//! typing with no invariant errors, before and after drop elaboration.
//! Negative coverage here (hand-built malformed MIR) pins each verifier check
//! class.

use mojito::check_program;
use mojito::mir::verify::{verify, verify_function_in};
use mojito::types::TyKind;
use std::path::Path;

/// Parse `source` with the bundled library linked and elaborated, since
/// `print`, `Bool`, and `StringLiteral` are its declarations, as upstream's
/// prelude's are.
fn parse(source: &str) -> Result<Vec<mojito::ast::Stmt>, String> {
    let linked = mojito::link_source(source, std::path::Path::new("snippet.mojo"))
        .map_err(|error| format!("{error:?}"))?;
    mojito::elaborate(linked).map_err(|error| format!("{error:?}"))
}

// --- Negative coverage: hand-built malformed MIR per verifier check class ---

use mojito::mir::{
    Const, FuncRef, LoopLabel, MirBlock, MirCase, MirDeclarations, MirFunction, MirInstr,
    MirInteriorOrigin, MirIntrinsicSubscript, MirLoan, MirParamArg, MirPlace, MirProgram,
    MirSubscriptArg, MirTerm, Proj, Raises, Reg, SpanTable, TraitDeclaration, TraitRequirement,
    TryLabel, UseMode,
};
use mojito::param_expr::{MetaTy, ParamContext, ParamId, ParamRef};
use mojito::types::DependentType;
use mojito::types::TransferSet;
use mojito::types::{FuncTy, GenericFuncTy};
use mojito::{CtValue, ParamDecl, Ty, TyArg};
use std::collections::HashMap;

fn function(blocks: MirBlock, n_regs: u32, reg_types: &[(u32, Ty)]) -> MirFunction {
    MirFunction {
        body: blocks,
        n_regs,
        n_vars: 2,
        var_names: vec!["x".to_string(), "$try_error0".to_string()],
        n_params: 0,
        param_types: Vec::new(),
        owned_params: Vec::new(),
        deinit_params: Vec::new(),
        ref_params: Vec::new(),
        returns_reference: false,
        ref_bindings: std::collections::BTreeSet::new(),
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
        fresh_splices: None,
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
        address_space: mojito::types::AddressSpaceSlot::GENERIC,
    })
    .into()
}

fn tracked_pointer_ty(element: Ty, mutable: bool) -> Ty {
    Ty::pointer(
        element,
        mojito::Origin::Place(mojito::OriginPlace {
            root: mojito::OwnerId(0),
            path: Vec::new(),
        }),
        mojito::Mutability::of(mutable),
        mojito::types::AddressSpaceSlot::GENERIC,
    )
}

/// Every instruction of `block` that holds no region, at any depth, in
/// order.
fn leaf_instructions_mut(block: &mut MirBlock) -> Vec<&mut MirInstr> {
    let mut leaves = Vec::new();
    for instruction in &mut block.instrs {
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
    place.projection_tys[field] = Ty::string_literal_of("s");
    expect_finding(&wrong_field_type, "field 'value'");
    expect_finding(
        &wrong_field_type,
        "typed StringLiteral[\"s\"], declared Int",
    );
}

#[test]
fn verifier_checks_make_ref_capability_types() {
    let place = MirPlace::root(0, Some(Ty::int()));

    let non_capability = function(
        block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(0, Ty::int())],
    );
    expect_finding(
        &program(non_capability),
        "MakeRef destination has non-reference-capability type Int",
    );

    let wrong_referent = function(
        block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(
            0,
            reference_ty(Ty::string_literal_of("s"), mojito::Mutability::Mutable),
        )],
    );
    expect_finding(
        &program(wrong_referent),
        "MakeRef destination targets StringLiteral[\"s\"], incompatible with place storage Int",
    );

    let raw_pointer = function(
        block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: place.clone(),
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(
            0,
            Ty::pointer(
                Ty::int(),
                mojito::Origin::Static,
                mojito::Mutability::Immutable,
                mojito::types::AddressSpaceSlot::GENERIC,
            ),
        )],
    );
    expect_finding(
        &program(raw_pointer),
        "MakeRef destination has non-reference-capability type Pointer",
    );

    let tracked_pointer = function(
        block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place,
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(0, tracked_pointer_ty(Ty::int(), true))],
    );
    assert_eq!(verify(&program(tracked_pointer)), Vec::<String>::new());
}

#[test]
fn verifier_reads_a_whole_pointer_slot_as_the_pointer() {
    // The whole of a pointer local is the pointer, as upstream's `!lit.ref`
    // of a pointer local addresses the local; it designates the pointee only
    // read through the slot (an inlined handle parameter), and a `mut`
    // pointer parameter's handle designates the caller's pointer.
    let pointer = tracked_pointer_ty(Ty::int(), false);
    let make_ref = |place: MirPlace, target: Ty, reference_parameter: bool| {
        let mut made = function(
            block(
                vec![MirInstr::MakeRef {
                    dest: Reg(0),
                    place,
                }],
                MirTerm::Return(None),
            ),
            1,
            &[(0, reference_ty(target, mojito::Mutability::Immutable))],
        );
        made.var_tys.insert(0, pointer.clone());
        if reference_parameter {
            made.n_params = 1;
            made.param_types = vec![pointer.clone()];
            made.owned_params = vec![false];
            made.deinit_params = vec![false];
            made.ref_params = vec![true];
        }
        program(made)
    };
    let whole = MirPlace::root(0, Some(pointer.clone()));
    let mut through = MirPlace::root(0, Some(pointer.clone()));
    through.ty = Some(Ty::int());
    through.through = Some(0);

    assert_eq!(
        verify(&make_ref(whole.clone(), pointer.clone(), false)),
        Vec::<String>::new()
    );
    expect_finding(
        &make_ref(whole, Ty::int(), false),
        "MakeRef destination targets Int, incompatible with place storage Pointer",
    );
    assert_eq!(
        verify(&make_ref(through.clone(), Ty::int(), false)),
        Vec::<String>::new()
    );
    expect_finding(
        &make_ref(through, pointer.clone(), false),
        "incompatible with place storage Int",
    );
    let mut parameter = MirPlace::root(0, Some(pointer.clone()));
    parameter.through = Some(0);
    assert_eq!(
        verify(&make_ref(parameter, pointer.clone(), true)),
        Vec::<String>::new()
    );
}

/// `holder.src`, a field storing a reference of `mutability` to an `Int`.
fn stored_ref_place(mutability: mojito::Mutability) -> MirPlace {
    let mut place = MirPlace::root(
        0,
        Some(TyKind::Struct("Holder".to_string(), Vec::new()).into()),
    );
    place.project(
        Proj::Field("src".to_string()),
        reference_ty(Ty::int(), mutability),
    );
    place
}

#[test]
fn verifier_rejects_make_ref_forwarding_a_stored_reference() {
    // A `ref.make` of a place ending at a stored reference borrows the
    // field's slot; one typed as the stored handle itself forwards nothing.
    let forwarded = function(
        block(
            vec![MirInstr::MakeRef {
                dest: Reg(0),
                place: stored_ref_place(mojito::Mutability::Mutable),
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(0, reference_ty(Ty::int(), mojito::Mutability::Immutable))],
    );
    expect_finding(
        &program(forwarded),
        "MakeRef destination targets Int, incompatible with place storage",
    );
}

#[test]
fn verifier_accepts_a_stored_reference_read_out_of_its_slot() {
    // `ref.make` borrows the field's slot, `ref (ref mut Int)`, and a
    // `ref.read` of it loads the stored handle at the type it is stored.
    let stored = reference_ty(Ty::int(), mojito::Mutability::Mutable);
    let read_out = |read: Ty| {
        function(
            block(
                vec![
                    MirInstr::MakeRef {
                        dest: Reg(1),
                        place: stored_ref_place(mojito::Mutability::Mutable),
                    },
                    MirInstr::ReadRef {
                        dest: Reg(0),
                        reference: Reg(1),
                    },
                ],
                MirTerm::Return(None),
            ),
            2,
            &[
                (0, read),
                (
                    1,
                    reference_ty(stored.clone(), mojito::Mutability::Immutable),
                ),
            ],
        )
    };
    assert_eq!(
        verify(&program(read_out(stored.clone()))),
        Vec::<String>::new()
    );
    expect_finding(
        &program(read_out(reference_ty(
            Ty::string_literal_of("s"),
            mojito::Mutability::Mutable,
        ))),
        "ReadRef result type",
    );
}

#[test]
fn verifier_requires_a_ref_field_load_to_load_its_stored_handle() {
    // A value read of a `ref` field is the field's `place.load`, typed as
    // the handle it stores, and a `ref.read` of that handle; a load typed
    // as the referent is rejected.
    let source = "@fieldwise_init\nstruct H[o: MutOrigin]:\n    var r: ref[Self.o] Int\n\ndef main():\n    var n = 4\n    var h = H(n)\n    print(h.r + 1)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(errors.is_empty(), "the lowered read verifies: {errors:?}");
    let main = function_named(&mut mir, "main");
    let instructions = leaf_instructions_mut(&mut main.body);
    let (load, read) = instructions
        .windows(2)
        .find_map(|pair| match (&*pair[0], &*pair[1]) {
            (
                MirInstr::LoadPlace { dest, place },
                MirInstr::ReadRef {
                    dest: read,
                    reference,
                },
            ) if reference == dest
                && matches!(place.proj.last(), Some(Proj::Field(field)) if field == "r") =>
            {
                Some((*dest, *read))
            }
            _ => None,
        })
        .expect("the field read is a handle load and a ref.read");
    assert!(matches!(
        main.reg_types.get(&load.0).map(Ty::kind),
        Some(TyKind::Ref(_))
    ));
    assert_eq!(main.reg_types.get(&read.0), Some(&Ty::int()));
    main.reg_types.insert(load.0, Ty::int());
    expect_finding(
        &mir,
        "LoadPlace of `ref` field place loads its stored handle",
    );
}

#[test]
fn verifier_checks_read_and_write_reference_capabilities() {
    let read_non_capability = function(
        block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        ),
        2,
        &[(0, Ty::int()), (1, Ty::int())],
    );
    expect_finding(
        &program(read_non_capability),
        "ReadRef source has non-reference-capability type Int",
    );

    let read_wrong_referent = function(
        block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        ),
        2,
        &[
            (0, Ty::string_literal_of("s")),
            (1, reference_ty(Ty::int(), mojito::Mutability::Immutable)),
        ],
    );
    expect_finding(
        &program(read_wrong_referent),
        "ReadRef result type StringLiteral[\"s\"] is incompatible with referent Int",
    );

    let write_immutable_reference = function(
        block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        ),
        2,
        &[
            (0, reference_ty(Ty::int(), mojito::Mutability::Immutable)),
            (1, Ty::int()),
        ],
    );
    expect_finding(
        &program(write_immutable_reference),
        "WriteRef source capability of type ref[ImmUntrackedOrigin] Int is immutable",
    );

    let write_immutable_pointer = function(
        block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        ),
        2,
        &[(0, tracked_pointer_ty(Ty::int(), false)), (1, Ty::int())],
    );
    expect_finding(&program(write_immutable_pointer), "is immutable");

    let write_wrong_referent = function(
        block(
            vec![MirInstr::WriteRef {
                reference: Reg(0),
                value: Reg(1),
            }],
            MirTerm::Return(None),
        ),
        2,
        &[
            (0, tracked_pointer_ty(Ty::int(), true)),
            (1, Ty::string_literal_of("s")),
        ],
    );
    expect_finding(
        &program(write_wrong_referent),
        "WriteRef value type StringLiteral[\"s\"] is incompatible with referent Int",
    );

    let read_pointer = function(
        block(
            vec![MirInstr::ReadRef {
                dest: Reg(0),
                reference: Reg(1),
            }],
            MirTerm::Return(None),
        ),
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
            block(
                vec![MirInstr::StoreRef {
                    place: MirPlace::root(0, Some(mutable_int.clone())),
                    reference: Reg(0),
                }],
                MirTerm::Return(None),
            ),
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
            Ty::string_literal_of("s"),
            mojito::Mutability::Mutable,
        ))),
        "StoreRef source referent StringLiteral[\"s\"] is incompatible with storage referent Int",
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
                        vec![Ty::int(), Ty::string_literal_of("s")],
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
        block(
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Int(1),
            }],
            MirTerm::Return(Some(Reg(0))),
        ),
        1,
        &[],
    );
    expect_finding(&program(f), "untyped register r0");
}

/// The literal-materialization findings of `literal`, typed `found`,
/// converted at the `!kgen.simd` storage of `target`.
fn storage_materialization_findings(literal: Const, found: Ty, target: &Ty) -> Vec<String> {
    let storage = mojito::types::storage_ty(target);
    let f = function(
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: literal,
                },
                MirInstr::MaterializeLiteral {
                    dest: Reg(1),
                    value: Reg(0),
                    target: storage.clone(),
                },
            ],
            MirTerm::Return(None),
        ),
        2,
        &[(0, found), (1, storage)],
    );
    verify(&program(f))
        .into_iter()
        .filter(|error| error.contains("materializ"))
        .collect()
}

#[test]
fn verifier_checks_a_literal_materialized_at_its_storage() {
    // `#pop<float_literal_convert>` is typed at the whole `!kgen.simd`
    // storage, a vector's as a scalar's.
    let float =
        || Const::FloatLiteral(mojito::FloatLiteral::parse_decimal("0.5").expect("decimal"));
    let float_ty =
        || Ty::float_literal_of(&mojito_common::literal::FloatLiteral::from_int(&0.into()));
    let lanes = |width| mojito::types::canonical_simd_ty(mojito::ast::Dtype::Float32, width);
    for width in [1, 4] {
        assert_eq!(
            storage_materialization_findings(float(), float_ty(), &lanes(width)),
            Vec::<String>::new()
        );
    }
    let int_lanes = mojito::types::canonical_simd_ty(mojito::ast::Dtype::Int32, 4);
    let truncating = storage_materialization_findings(float(), float_ty(), &int_lanes);
    assert!(
        truncating
            .iter()
            .any(|error| error.contains("cannot materialize")),
        "{truncating:?}"
    );
    // An integer literal converts into a `bool` vector, each lane whether
    // it is nonzero, as `#pop.int_literal_convert` does.
    let mask = mojito::types::canonical_simd_ty(mojito::ast::Dtype::Bool, 4);
    assert_eq!(
        storage_materialization_findings(
            Const::IntLiteral(mojito::IntLiteral::from(1)),
            Ty::int_literal_of(&0.into()),
            &mask,
        ),
        Vec::<String>::new()
    );
    let int = storage_materialization_findings(float(), float_ty(), &Ty::int());
    assert!(
        int.iter().any(|error| error.contains("cannot materialize")),
        "{int:?}"
    );
}

#[test]
fn verifier_rejects_a_literal_materialized_at_a_struct() {
    // Upstream's literal conversion is typed at the `!pop.simd` storage;
    // the `Int` is a `struct.create` over it.
    let f = function(
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::IntLiteral(mojito::IntLiteral::from(1)),
                },
                MirInstr::MaterializeLiteral {
                    dest: Reg(1),
                    value: Reg(0),
                    target: Ty::int(),
                },
            ],
            MirTerm::Return(None),
        ),
        2,
        &[(0, Ty::int_literal_of(&0.into())), (1, Ty::int())],
    );
    expect_finding(
        &program(f),
        "literal materialization targets Int, not a `!kgen.simd` storage",
    );
}

/// A `mut` argument of a direct call is its place's address, as
/// upstream's `lit.call` takes a `!lit.ref` of the caller's storage; the
/// verifier rejects the value read from the place in its stead.
#[test]
fn verifier_requires_a_reference_at_a_mut_argument() {
    let source = "def inc(mut x: Int):\n    x += 1\n\ndef main():\n    var i = 3\n    inc(i)\n    print(i)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(errors.is_empty(), "the lowered call verifies: {errors:?}");
    let (_, main) = mir
        .functions
        .iter_mut()
        .find(|(name, _)| name == "main")
        .expect("main is lowered");
    let argument = leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == "inc" => Some(args[0]),
            _ => None,
        })
        .expect("main calls inc");
    let address = leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find(|instruction| {
            matches!(instruction, MirInstr::MakeRef { dest, .. } if *dest == argument)
        })
        .expect("the argument is a ref.make of the place");
    let load = match &*address {
        MirInstr::MakeRef { dest, place } => MirInstr::LoadPlace {
            dest: *dest,
            place: place.clone(),
        },
        other => other.clone(),
    };
    *address = load;
    main.reg_types.insert(argument.0, Ty::int());
    expect_finding(
        &mir,
        "argument 0 of 'inc' is received by address but passes a Int value",
    );
}

/// A memory-only `read` argument of a direct call is its place's address,
/// as upstream's `lit.call` takes a `!lit.ref` wherever the callee's
/// signature does; the verifier rejects the value read from the place.
#[test]
fn verifier_requires_a_reference_at_a_memory_only_read_argument() {
    let source = "def show(s: String):\n    print(s)\n\ndef main():\n    var s = String(\"a\")\n    show(s)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the call through ref.make verifies: {errors:?}"
    );
    let main = function_named(&mut mir, "main");
    let argument = leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == "show" => Some(args[0]),
            _ => None,
        })
        .expect("main calls show");
    load_in_place_of_address(main, argument);
    expect_finding(
        &mir,
        "argument 0 of 'show' is received by address but passes a",
    );
}

/// A keyword operand at a by-address parameter must be a reference too, as
/// upstream's `lit.call` takes a `!lit.ref` wherever the signature does,
/// however the call site spelled the operand: the copy initializer's
/// `copy=` and a method's keyword-only memory-only `read` argument alike.
#[test]
fn verifier_requires_a_reference_at_a_keyword_by_address_argument() {
    let source = "struct Big(Copyable):\n    var a: String\n\n    def __init__(out self, a: String):\n        self.a = a\n\n    def __init__(out self, *, copy: Self):\n        self.a = copy.a\n\n    def take(self, *, other: Big) -> Int:\n        return other.a.byte_length()\n\ndef main():\n    var b = Big(\"q\")\n    var c = Big(copy=b)\n    print(b.take(other=c))\n";
    let mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the keyword operands through ref.make verify: {errors:?}"
    );
    for (method, needle) in [
        (
            false,
            "keyword argument 'copy' of 'Big.__copyinit__' is received by address but passes a Big value",
        ),
        (
            true,
            "keyword argument 'other' of 'Big.take' is received by address but passes a Big value",
        ),
    ] {
        let mut mir = mir.clone();
        let main = function_named(&mut mir, "main");
        let argument = leaf_instructions_mut(&mut main.body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::Call { func, kwargs, .. }
                    if !method && func.0.contains("__copyinit__") =>
                {
                    Some(kwargs[0].1)
                }
                MirInstr::MethodCall {
                    method: name,
                    kwargs,
                    ..
                } if method && name == "take" => Some(kwargs[0].1),
                _ => None,
            })
            .expect("main passes the keyword operand");
        load_in_place_of_address(main, argument);
        expect_finding(&mir, needle);
    }
}

/// Every operand a call binds is compared against its parameter's type,
/// a keyword one as a positional one, as upstream's `lit.call` verifier
/// checks every operand against its callee's signature.
#[test]
fn verifier_compares_a_keyword_operand_type_against_its_parameter() {
    let source = "struct Big(Copyable):\n    var a: String\n\n    def __init__(out self, a: String):\n        self.a = a\n\n    def __init__(out self, *, copy: Self):\n        self.a = copy.a\n\ndef main():\n    var b = Big(\"q\")\n    var c = Big(copy=b)\n    print(c.a)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the keyword operand verifies: {errors:?}"
    );
    let main = function_named(&mut mir, "main");
    let instructions = leaf_instructions_mut(&mut main.body);
    let string = instructions
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0.starts_with("Big.__init__") => {
                Some(args[0])
            }
            _ => None,
        })
        .expect("main constructs Big from a String");
    instructions
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, kwargs, .. } if func.0.contains("__copyinit__") => Some(kwargs),
            _ => None,
        })
        .expect("main copies Big")[0]
        .1 = string;
    expect_finding(
        &mir,
        "keyword argument 'copy' of 'Big.__copyinit__' has type String, declared Big",
    );
}

/// A call of a variadic callee passes its pack as one spread operand,
/// which binds the collector, so the keyword operands after it bind their
/// parameters and are compared against them.
#[test]
fn verifier_compares_a_variadic_call_keyword_operand_type() {
    let source = "def g(*xs: Int, tag: Int) -> Int:\n    return tag\n\ndef main():\n    var s = String(\"q\")\n    print(g(1, 2, tag=3))\n    print(s)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(errors.is_empty(), "the variadic call verifies: {errors:?}");
    let main = function_named(&mut mir, "main");
    let string = main
        .reg_types
        .iter()
        .filter(|(_, ty)| ty.to_string() == "String")
        .map(|(register, _)| Reg(*register))
        .min_by_key(|register| register.0)
        .expect("main holds a String register");
    leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call {
                func,
                kwargs,
                spread: Some(_),
                ..
            } if func.0 == "g" => Some(kwargs),
            _ => None,
        })
        .expect("main calls g with its pack spread")[0]
        .1 = string;
    expect_finding(
        &mir,
        "keyword argument 'tag' of 'g' has type String, declared Int",
    );
}

/// A memory-only `read self` receiver is received by address, as
/// upstream's `lit.call` takes a `!lit.ref` of it: every verify point, the
/// whole program's and one loaded function's alike, rejects a method call or
/// a subscript accessor passing the value read from the receiver's place.
#[test]
fn verifier_requires_a_reference_at_a_memory_only_read_receiver() {
    let source = "struct M(Movable):\n    var s: String\n\n    def __init__(out self, s: String):\n        self.s = s\n\n    def show(self):\n        print(self.s)\n\n    def __getitem__(self, i: Int) -> Int:\n        return i\n\ndef main():\n    var m = M(\"a\")\n    m.show()\n    print(m[1])\n";
    let mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the receivers through ref.make verify: {errors:?}"
    );
    let receiver = |mir: &mut MirProgram, method: bool| {
        let main = function_named(mir, "main");
        let receiver = leaf_instructions_mut(&mut main.body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::MethodCall {
                    method: name, recv, ..
                } if method && name == "show" => Some(*recv),
                MirInstr::Index {
                    base,
                    call: Some(_),
                    ..
                } if !method => Some(*base),
                _ => None,
            })
            .expect("main calls the receiver's method");
        load_in_place_of_address(main, receiver);
    };
    for (method, needle) in [
        (
            true,
            "receiver of 'M.show' is received by address but passes a",
        ),
        (false, "receiver of 'M.__getitem__"),
    ] {
        let mut mir = mir.clone();
        receiver(&mut mir, method);
        let errors = verify(&mir);
        assert!(
            errors.iter().any(|error| error.contains(needle)),
            "expected a finding containing '{needle}', got {errors:?}"
        );
        let main = function_named(&mut mir, "main").clone();
        let errors = verify_function_in("main", &main, &mir.declarations);
        assert!(
            errors.iter().any(|error| error.contains(needle)),
            "a loaded function's verify applies the receiver rule: {errors:?}"
        );
    }
}

/// A call through a trait bound binds the requirement the bound declares,
/// as upstream's `lit.call` of `@Trait::method` is verified against the
/// requirement's signature: the verifier rejects a value where the
/// requirement takes the receiver (`mut self`, or a `read self` of a
/// memory-only type parameter) or an argument (`mut n`) by address, at a
/// method call and a subscript alike.
#[test]
fn verifier_requires_a_reference_at_a_requirement_operand() {
    let source = "trait Shape:\n    def area(self) -> Int: ...\n    def grow(mut self, by: Int): ...\n    def bump(self, mut n: Int): ...\n    def __getitem__(self, i: Int) -> Int: ...\n\ndef use_it[T: Shape](mut t: T) -> Int:\n    t.grow(2)\n    var n = 1\n    t.bump(n)\n    return t.area() + n + t[3]\n";
    let mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the requirement calls through ref.make verify: {errors:?}"
    );
    let operand = |mir: &mut MirProgram, target: &str| {
        let use_it = function_named(mir, "use_it");
        let operand = leaf_instructions_mut(&mut use_it.body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::MethodCall {
                    method, recv, args, ..
                } if method == target => Some(if target == "bump" { args[0] } else { *recv }),
                MirInstr::Index {
                    base,
                    call: Some(_),
                    ..
                } if target == "__getitem__" => Some(*base),
                _ => None,
            })
            .expect("use_it calls the requirement");
        load_in_place_of_address(use_it, operand);
    };
    for (target, needle) in [
        (
            "grow",
            "receiver of '__trait_dispatch.grow$ov$Int' is received by address but passes a",
        ),
        ("area", "receiver of '__trait_dispatch.area"),
        ("bump", "argument 0 of '__trait_dispatch.bump"),
        ("__getitem__", "receiver of '__trait_dispatch.__getitem__"),
    ] {
        let mut mir = mir.clone();
        operand(&mut mir, target);
        expect_finding(&mir, needle);
        let use_it = function_named(&mut mir, "use_it").clone();
        let errors = verify_function_in("use_it", &use_it, &mir.declarations);
        assert!(
            errors.iter().any(|error| error.contains(needle)),
            "a loaded function's verify applies the requirement rule: {errors:?}"
        );
    }
}

/// A memory-only `read` index of a subscript accessor is its place's
/// address, as upstream's accessor `lit.call` takes a `!lit.ref` wherever
/// the accessor's signature does: the verifier rejects the value read from
/// the place at a getter (`index.get`), a multi-index getter
/// (`index.multi`), and a setter (`index.multi_set`) alike.
#[test]
fn verifier_requires_a_reference_at_a_memory_only_read_subscript_argument() {
    let source = "struct M(Movable):\n    var n: Int\n\n    def __init__(out self):\n        self.n = 0\n\n    def __getitem__(self, k: String) -> Int:\n        return k.byte_length()\n\n    def __getitem__(self, a: String, b: String) -> Int:\n        return a.byte_length() + b.byte_length()\n\n    def __setitem__(mut self, k: String, value: Int):\n        self.n = value\n\ndef main():\n    var m = M()\n    var k = String(\"a\")\n    print(m[k])\n    print(m[k, k])\n    m[k] = 2\n";
    let mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(
        errors.is_empty(),
        "the subscript arguments through ref.make verify: {errors:?}"
    );
    for (shape, needle) in [
        ("get", "argument 0 of 'M.__getitem__"),
        ("multi", "argument 1 of 'M.__getitem__"),
        ("set", "argument 0 of 'M.__setitem__"),
    ] {
        let mut mir = mir.clone();
        let main = function_named(&mut mir, "main");
        let argument = leaf_instructions_mut(&mut main.body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::Index {
                    index,
                    call: Some(_),
                    ..
                } if shape == "get" => Some(*index),
                MirInstr::MultiIndex { args, .. } if shape == "multi" => match args.as_slice() {
                    [_, MirSubscriptArg::Index(second)] => Some(*second),
                    _ => None,
                },
                MirInstr::MultiSet { args, .. } if shape == "set" => match args.as_slice() {
                    [MirSubscriptArg::Index(key)] => Some(*key),
                    _ => None,
                },
                _ => None,
            })
            .expect("main indexes m");
        load_in_place_of_address(main, argument);
        expect_finding(&mir, needle);
    }
}

/// A register-passable `read self` receiver is passed by value.
#[test]
fn verifier_accepts_a_value_at_a_register_passable_read_receiver() {
    let source = "@fieldwise_init\nstruct P(Copyable, Movable, TrivialRegisterPassable):\n    var v: Int\n\n    def get(self) -> Int:\n        return self.v\n\ndef main():\n    var p = P(1)\n    print(p.get())\n";
    let mut mir = lower_linked_source(source);
    assert_eq!(verify(&mir), Vec::<String>::new());
    let main = function_named(&mut mir, "main");
    let receiver = leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall { method, recv, .. } if method == "get" => Some(*recv),
            _ => None,
        })
        .expect("main calls P.get");
    assert!(
        !matches!(
            main.reg_types.get(&receiver.0).map(Ty::kind),
            Some(TyKind::Ref(_))
        ),
        "a register-passable receiver is passed by value"
    );
}

/// A generic `read x: T` takes its argument by address in the template,
/// as the declared `T` is not proven register-passable, a static method
/// called through its type included; the `Int` instance takes the value,
/// which the concrete program's verification accepts.
#[test]
fn verifier_accepts_a_value_at_a_register_passable_instance_read() {
    let source = "@fieldwise_init\nstruct S(Copyable):\n    @staticmethod\n    def show[T: Copyable](x: T):\n        pass\n\ndef main():\n    S.show(3)\n";
    let compiled = mojito::Compiler::default()
        .compile_source(source, Path::new("verify_test.mojo"))
        .expect("compile");
    let template = compiled.elaborated_mir();
    assert_eq!(verify(template), Vec::<String>::new());
    let main = template
        .functions
        .iter()
        .find_map(|(name, function)| (name == "main").then_some(function))
        .expect("main is lowered");
    let mut body = main.body.clone();
    let passed = leaf_instructions_mut(&mut body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == "S.show" => Some(args[0]),
            _ => None,
        })
        .expect("main calls S.show");
    assert!(
        matches!(
            main.reg_types.get(&passed.0).map(Ty::kind),
            Some(TyKind::Ref(_))
        ),
        "the template passes the declared `T` slot by address"
    );
    compiled
        .concrete_mir()
        .expect("the Int instance takes the value");
}

/// Turn the `ref.make` defining `function`'s `argument` into a load of
/// its place, as if the operand were passed by value.
fn load_in_place_of_address(function: &mut MirFunction, argument: Reg) {
    let address = leaf_instructions_mut(&mut function.body)
        .into_iter()
        .find(|instruction| {
            matches!(instruction, MirInstr::MakeRef { dest, .. } if *dest == argument)
        })
        .expect("the argument is a ref.make of its place");
    if let MirInstr::MakeRef { dest, place } = &*address {
        let referent = place.ty.clone().expect("a typed place");
        *address = MirInstr::LoadPlace {
            dest: *dest,
            place: place.clone(),
        };
        function.reg_types.insert(argument.0, referent);
    }
}

fn function_named<'p>(program: &'p mut MirProgram, name: &str) -> &'p mut MirFunction {
    program
        .functions
        .iter_mut()
        .find_map(|(function, body)| (function == name).then_some(body))
        .expect("the function is lowered")
}

#[test]
fn verifier_requires_a_reference_at_an_indirect_mut_argument() {
    let source = "def bump(mut v: Int):\n    v += 1\n\ndef apply(f: def(mut Int) thin -> None, mut x: Int):\n    f(x)\n\ndef main():\n    var i = 3\n    apply(bump, i)\n    print(i)\n";
    let mut mir = lower_linked_source(source);
    let errors = verify(&mir);
    assert!(errors.is_empty(), "the lowered call verifies: {errors:?}");
    let apply = function_named(&mut mir, "apply");
    let argument = leaf_instructions_mut(&mut apply.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::CallIndirect { args, .. } => Some(args[0]),
            _ => None,
        })
        .expect("apply calls through its function value");
    load_in_place_of_address(apply, argument);
    expect_finding(
        &mir,
        "argument 0 of indirect callable is received by address but passes a Int value",
    );
}

#[test]
fn verifier_requires_a_reference_at_an_indirect_memory_only_read_argument() {
    // `Big` holds a `String`, so it is memory-only and `f`'s `read` slot
    // takes a reference, positionally and by keyword alike.
    let source = "struct Big(Movable):\n    var s: String\n    def __init__(out self):\n        self.s = String(\"x\")\n\ndef size(b: Big) -> Int:\n    return b.s.byte_length()\n\ndef apply(f: def(Big) thin -> Int, b: Big) -> Int:\n    return f(b)\n\ndef named(b: Big) -> Int:\n    var f = size\n    return f(b=b)\n\ndef main():\n    var b = Big()\n    print(apply(size, b) + named(b))\n";
    for (caller, operand) in [("apply", 0usize), ("named", 0)] {
        let mut mir = lower_linked_source(source);
        let errors = verify(&mir);
        assert!(errors.is_empty(), "the lowered call verifies: {errors:?}");
        let body = function_named(&mut mir, caller);
        let argument = leaf_instructions_mut(&mut body.body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::CallIndirect { args, kwargs, .. } => args
                    .first()
                    .copied()
                    .or_else(|| kwargs.first().map(|(_, register)| *register)),
                _ => None,
            })
            .expect("the caller calls through its function value");
        load_in_place_of_address(body, argument);
        expect_finding(
            &mir,
            &format!(
                "argument {operand} of indirect callable is received by address but passes a Big value"
            ),
        );
    }
}

#[test]
fn verifier_requires_references_at_a_subscript_s_by_address_operands() {
    const SOURCE: &str = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(ref self, mut index: Int) -> Int:\n        index += 1\n        return self.value\n\ndef main():\n    var box = Box(40)\n    var index = 0\n    print(box[index])\n";
    let operands = |program: &mut MirProgram| {
        leaf_instructions_mut(&mut function_named(program, "main").body)
            .into_iter()
            .find_map(|instruction| match instruction {
                MirInstr::Index {
                    base,
                    index,
                    call: Some(_),
                    ..
                } => Some((*base, *index)),
                _ => None,
            })
            .expect("main subscripts the box")
    };
    let valid = lower_linked_source(SOURCE);
    let errors = verify(&valid);
    assert!(
        errors.is_empty(),
        "the lowered subscript verifies: {errors:?}"
    );

    let mut loaded_index = valid.clone();
    let (_, index) = operands(&mut loaded_index);
    load_in_place_of_address(function_named(&mut loaded_index, "main"), index);
    expect_finding(
        &loaded_index,
        "selected subscript mut/ref argument Positional(0) passes no reference",
    );

    let mut loaded_base = valid;
    let (base, _) = operands(&mut loaded_base);
    load_in_place_of_address(function_named(&mut loaded_base, "main"), base);
    expect_finding(
        &loaded_base,
        "selected subscript reference receiver passes no reference",
    );
}

#[test]
fn verifier_requires_an_immutable_def_bound_receiver() {
    let mut program = lower_linked_source(DEF_BOUND_APPLY);
    assert_eq!(apply_findings(&program), Vec::<String>::new());
    let recv = match bound_call(&mut program) {
        MirInstr::MethodCall { recv, .. } => Some(*recv),
        _ => None,
    }
    .expect("the bound call is a method call");
    let apply = function_named(&mut program, "apply");
    let found = apply.reg_types[&recv.0].clone();
    let referent = match found.kind() {
        TyKind::Ref(reference) => reference.referent.clone(),
        _ => found,
    };
    apply
        .reg_types
        .insert(recv.0, reference_ty(referent, mojito::Mutability::Mutable));
    assert!(
        apply_findings(&program)
            .iter()
            .any(|error| error.contains("`__call__` receiver is a mutable reference")),
        "{:?}",
        apply_findings(&program)
    );
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
    first_index_call(&mut wrong_type).arguments[0].parameter_ty = Ty::string_literal_of("s");
    expect_finding(&wrong_type, "subscript source has type Int");
    expect_finding(
        &wrong_type,
        "does not match 'Box.__getitem__' declaration Int",
    );

    let mut wrong_result = lower_linked_source(SOURCE);
    first_index_call(&mut wrong_result).result_ty = Ty::string_literal_of("s");
    expect_finding(&wrong_result, "subscript result has type Int");
    expect_finding(
        &wrong_result,
        "selected result type StringLiteral[\"s\"] does not match 'Box.__getitem__' declaration Int",
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
    // A lane read is `simd.extract`; a compile-time list subscript of the
    // vector is rejected.
    let lane_read_as = |intrinsic: MirIntrinsicSubscript| {
        let mut mir = lower_linked_source(
            "def main():\n    var lanes = SIMD[DType.int, 2](1, 2)\n    print(Int(mlir_value=__mlir_op.`pop.simd.extractelement`(lanes._mlir_value, Int(0).__mlir_index__())))\n",
        );
        let instruction = mir
            .functions
            .iter_mut()
            .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
            .find(|instruction| matches!(instruction, MirInstr::SimdExtract { .. }))
            .expect("SIMD lane read lowered");
        let MirInstr::SimdExtract { dest, value, index } = *instruction else {
            unreachable!();
        };
        *instruction = MirInstr::Index {
            dest,
            base: value,
            index,
            call: None,
            intrinsic: Some(intrinsic),
        };
        mir
    };
    expect_finding(
        &lane_read_as(MirIntrinsicSubscript::ComptimeList),
        "ComptimeList is incompatible with checked base type",
    );

    let source = "@fieldwise_init\nstruct Access:\n    var value: Int\n    def __getitem__(self, index: Int) -> Int:\n        return self.value\n\ndef main():\n    print(Access(1)[0])\n";
    let mut conflicting = lower_source(source);
    conflicting
        .functions
        .iter_mut()
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
        .find(|instruction| matches!(instruction, MirInstr::SimdSelect { .. }))
        .expect("SIMD select lowered");
    if let MirInstr::SimdSelect { mask, on_true, .. } = select {
        *mask = *on_true;
    }
    expect_finding(&mir, "simd.select mask has DType.int32 lanes, not bool");
}

#[test]
fn verifier_reads_simd_lane_operands_as_storage() {
    let source = "def main():\n    var v = SIMD[DType.int32, 4](1, 2, 3, 4)\n    print(Int32(mlir_value=__mlir_op.`pop.simd.extractelement`(v._mlir_value, Int(1).__mlir_index__())))\n";
    let mir = lower_linked_source(source);
    let main_errors: Vec<String> = verify(&mir)
        .into_iter()
        .filter(|error| error.contains("'main'"))
        .collect();
    assert!(main_errors.is_empty(), "{main_errors:?}");

    // Upstream's `pop.simd.extractelement` verifies a `!pop.simd` operand,
    // never the `SIMD` struct around it.
    let mut struct_operand = mir;
    let (_, main) = struct_operand
        .functions
        .iter_mut()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let vector = leaf_instructions_mut(&mut main.body)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::SimdExtract { value, .. } => Some(*value),
            _ => None,
        })
        .expect("SIMD lane read lowered");
    let structure = mojito::types::storage_simd(&main.reg_types[&vector.0])
        .expect("the lane read's vector is its !kgen.simd storage");
    main.reg_types.insert(vector.0, structure);
    expect_finding(
        &struct_operand,
        "simd.extract vector is the struct SIMD[DType.int32, 4], not its !kgen.simd storage",
    );
}

#[test]
fn verifier_types_select() {
    let mut mir = lower_linked_source(
        "def main():\n    var c = 3 > 2\n    print(__mlir_op.`pop.select`(c.__mlir_bool__(), Int(1), Int(2)))\n",
    );
    let select = mir
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        lower_linked_source(
            "def main():\n    var x = Int32(7)\n    print(__mlir_op.`pop.call_llvm_intrinsic`[intrin=\"llvm.ctpop\".value, _type=Int32, hasSideEffects=False.__mlir_i1__()](x))\n",
        )
    };
    let with_call = |edit: &dyn Fn(&mut MirInstr)| {
        let mut mir = lowered();
        let call = mir
            .functions
            .iter_mut()
            .filter(|(name, _)| name == "main")
            .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
            .any(|error| error.contains("intrinsic") && error.contains("'main'")),
        "a closed call in the table is concrete"
    );
    concrete_finding(
        &with_call(&|call| {
            if let MirInstr::CallLlvmIntrinsic { intrinsic, .. } = call {
                *intrinsic = Const::Value(CtValue::KgenString("llvm.x86.sse.stmxcsr".to_string()));
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
        .referent = Ty::string_literal_of("s");
    expect_finding(
        &wrong_referent,
        "reference-result referent StringLiteral[\"s\"] does not match 'Box.__getitem__' declaration Item",
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
        Some(TyKind::Tuple(vec![Ty::int(), Ty::string_literal_of("s")]).into()),
    );
    place.project(Proj::ConstIndex(2), Ty::int());
    let f = function(
        block(
            vec![MirInstr::LoadPlace {
                dest: Reg(0),
                place,
            }],
            MirTerm::Return(None),
        ),
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
            block(
                vec![MirInstr::LoadPlace {
                    dest: Reg(0),
                    place,
                }],
                MirTerm::Return(None),
            ),
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
    let pointer = Ty::pointer(
        Ty::int(),
        mojito::Origin::Untracked { mutable: true },
        mojito::Mutability::Mutable,
        mojito::types::AddressSpaceSlot::GENERIC,
    );

    let mut wrong_element = MirPlace::root(0, Some(pointer.clone()));
    wrong_element.project(Proj::Index(Reg(0)), Ty::string_literal_of("s"));
    let wrong_element = function(
        block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_element,
            }],
            MirTerm::Return(None),
        ),
        2,
        &[(0, Ty::int()), (1, Ty::string_literal_of("s"))],
    );
    expect_finding(
        &program(wrong_element),
        "place dynamic element typed StringLiteral",
    );

    let mut wrong_index = MirPlace::root(0, Some(pointer));
    wrong_index.project(Proj::Index(Reg(0)), Ty::int());
    let wrong_index = function(
        block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: wrong_index,
            }],
            MirTerm::Return(None),
        ),
        2,
        &[(0, Ty::string_literal_of("s")), (1, Ty::int())],
    );
    expect_finding(&program(wrong_index), "has non-Indexer type StringLiteral");

    // An `Int` is a width-one `SIMD`, whose lane a subscript reads.
    let mut non_indexed = MirPlace::root(0, Some(Ty::bool()));
    non_indexed.project(Proj::Index(Reg(0)), Ty::int());
    let non_indexed = function(
        block(
            vec![MirInstr::LoadPlace {
                dest: Reg(1),
                place: non_indexed,
            }],
            MirTerm::Return(None),
        ),
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
        block(
            vec![MirInstr::EstablishLoans {
                reference: 0,
                loans: Vec::new(),
                marker: Reg(0),
                dest_interior: None,
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(0, Ty::none())],
    );
    expect_finding(&program(empty), "loan generation has no owner loans");

    let malformed = function(
        block(
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
        ),
        1,
        &[(0, Ty::none())],
    );
    expect_finding(&program(malformed), "has no interior segment");

    let immutable = reference_ty(Ty::int(), mojito::Mutability::Immutable);
    let mut escalated = function(
        block(
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
        ),
        1,
        &[(0, Ty::none())],
    );
    escalated.var_tys.insert(0, immutable);
    expect_finding(
        &program(escalated),
        "mutable loan recovers permission unavailable through its source capability",
    );

    let mismatched_root = function(
        block(
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
        ),
        1,
        &[(0, Ty::none())],
    );
    expect_finding(
        &program(mismatched_root),
        "executable place roots at slot 0",
    );

    let unnamed_refresh = function(
        block(
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
        ),
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
        block(
            vec![MirInstr::MakeClosure {
                dest: Reg(0),
                function: "missing$nested".to_string(),
                captures: Vec::new(),
                arguments: Vec::new(),
            }],
            MirTerm::Return(None),
        ),
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "undeclared lifted function 'missing$nested'");
}

#[test]
fn verifier_rejects_a_top_level_try_yield() {
    let f = function(block(Vec::new(), MirTerm::TryYield(Vec::new())), 0, &[]);
    expect_finding(&program(f), "`try.yield` outside the region it ends");
}

#[test]
fn verifier_rejects_a_non_bool_if_condition() {
    let arm = || block(Vec::new(), MirTerm::Yield(Vec::new()));
    let f = function(
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                },
                MirInstr::If {
                    results: Vec::new(),
                    cond: Reg(0),
                    then: Box::new(arm()),
                    elifs: Vec::new(),
                    else_args: Vec::new(),
                    orelse: Box::new(arm()),
                },
            ],
            MirTerm::Return(None),
        ),
        1,
        &[(0, Ty::int())],
    );
    expect_finding(&program(f), "`if` condition has type Int");
}

#[test]
fn verifier_rejects_return_type_mismatches() {
    let f = function(
        block(
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Str("no".to_string()),
            }],
            MirTerm::Return(Some(Reg(0))),
        ),
        1,
        &[(0, Ty::string_literal_of("s"))],
    );
    expect_finding(
        &program(f),
        "return of StringLiteral[\"s\"] from a function returning Int",
    );
}

#[test]
fn verifier_rejects_runtime_pack_types_in_function_body_slots() {
    let mut f = function(block(Vec::new(), MirTerm::Return(None)), 0, &[]);
    f.n_params = 1;
    f.param_types = vec![TyKind::RuntimePack(vec![Ty::int(), Ty::string_literal_of("s")]).into()];
    f.var_tys.insert(
        0,
        TyKind::RuntimePack(vec![Ty::int(), Ty::string_literal_of("s")]).into(),
    );
    expect_finding(&program(f), "retains ABI-only RuntimePack type");
}

#[test]
fn verifier_rejects_unprotected_raises_in_nonraising_functions() {
    let f = function(
        block(
            vec![MirInstr::Const {
                dest: Reg(0),
                k: Const::Str("boom".to_string()),
            }],
            MirTerm::ErrorReturn,
        ),
        1,
        &[(0, Ty::string_literal_of("s"))],
    );
    expect_finding(&program(f), "unprotected raise in nonraising function");
}

#[test]
fn verifier_rejects_mismatched_declared_call_arguments() {
    use mojito::mir::MirFunctionDeclaration;
    let f = function(
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Str("no".to_string()),
                },
                MirInstr::Call {
                    conventions: mojito::mir::MirArgConventions::default(),
                    dest: Reg(1),
                    func: FuncRef::named("callee"),
                    raises: None,
                    args: vec![Reg(0)],
                    kwargs: Vec::new(),
                    param_arg_regs: Vec::new(),
                    capture_accesses: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
            MirTerm::Return(None),
        ),
        2,
        &[(0, Ty::string_literal_of("s")), (1, Ty::int())],
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
        "argument 0 of 'callee' has type StringLiteral[\"s\"], declared Int",
    );
    prog.declarations.functions[0].param_conventions.clear();
    expect_finding(&prog, "has 0 parameter conventions, expected 1");
}

#[test]
fn verifier_rejects_malformed_direct_compile_time_arguments() {
    use mojito::mir::MirFunctionDeclaration;
    let declaration = value_parameter("n", Ty::int());
    let f = function(
        block(
            vec![MirInstr::Call {
                conventions: mojito::mir::MirArgConventions::default(),
                dest: Reg(1),
                func: FuncRef::named("callee"),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
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
        ),
        2,
        &[(0, Ty::string_literal_of("s")), (1, Ty::int())],
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
    expect_finding(&prog, "register type StringLiteral[\"s\"], declared Int");

    // An argument binds the declaration it names, never a position: one
    // naming none is rejected, and so is a second naming the same one.
    let MirInstr::Call { param_arg_regs, .. } = &mut prog.functions[0].1.body.instrs[0] else {
        unreachable!()
    };
    param_arg_regs[0].name = None;
    expect_finding(
        &prog,
        "compile-time argument does not match a parameter declaration",
    );
    let MirInstr::Call { param_arg_regs, .. } = &mut prog.functions[0].1.body.instrs[0] else {
        unreachable!()
    };
    *param_arg_regs = vec![
        MirParamArg {
            name: Some("n".to_string()),
            value: Some(Reg(1)),
            expr: None,
        };
        2
    ];
    expect_finding(
        &prog,
        "compile-time parameter 'n' is supplied more than once",
    );

    let MirInstr::Call { param_arg_regs, .. } = &mut prog.functions[0].1.body.instrs[0] else {
        unreachable!()
    };
    param_arg_regs.clear();
    expect_finding(
        &prog,
        "required compile-time value parameter 'n' is missing",
    );
}

#[test]
fn verifier_checks_compile_time_arguments_under_the_call_and_receiver_arguments() {
    // `h`'s declared contract names the earlier parameter `d` (free) or the
    // receiver's `Self.dtype` (method, on a struct or on `SIMD`): each
    // register is checked against it closed under those arguments.
    const FREE: &str = "def run[d: DType, h: def(SIMD[d, 2]) thin -> None]():\n    h(SIMD[d, 2](1, 2))\n\ndef show(v: SIMD[DType.int32, 2]):\n    print(v)\n\ndef main():\n    run[DType.int32, show]()\n";
    const METHOD: &str = "struct C[dtype: DType]:\n    def __init__(out self):\n        pass\n\n    def run[h: def(Scalar[Self.dtype]) thin -> None](self):\n        h(Scalar[Self.dtype](3))\n\ndef show(v: Scalar[DType.int32]):\n    print(v)\n\ndef main():\n    C[DType.int32]().run[show]()\n";
    const SIMD_METHOD: &str = "def addi[width: Int](lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]) -> SIMD[DType.int32, width]:\n    return lhs + rhs\n\ndef main():\n    print(SIMD[DType.int32, 4](1, 2, 3, 4).reduce[addi, 2]())\n";
    assert_eq!(verify(&lower_linked_source(FREE)), Vec::<String>::new());
    assert_eq!(verify(&lower_linked_source(METHOD)), Vec::<String>::new());
    assert_eq!(
        verify(&lower_linked_source(SIMD_METHOD)),
        Vec::<String>::new()
    );
}

#[test]
fn verifier_rejects_malformed_indirect_compile_time_arguments() {
    let declarations = vec![value_parameter("n", Ty::int())];
    let callable = generic_callable(declarations.clone());
    let f = function(
        block(
            vec![MirInstr::CallIndirect {
                dest: Reg(2),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
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
        ),
        3,
        &[
            (0, callable),
            (1, Ty::string_literal_of("s")),
            (2, Ty::int()),
        ],
    );
    let mut prog = program(f);
    expect_finding(&prog, "register type StringLiteral[\"s\"], declared Int");
    expect_finding(
        &prog,
        "a call through a generic callable carries no instantiation",
    );

    let MirInstr::CallIndirect {
        param_arg_regs,
        param_decls,
        ..
    } = &mut prog.functions[0].1.body.instrs[0]
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
fn verifier_accepts_an_indirect_call_carrying_its_instantiation() {
    // `F[3]()` through `F: def[n: Int]() thin -> Int` binds `n` as Mojo's
    // `#kgen.bind_params(F, 3)`: the closed argument has no register.
    let declarations = vec![value_parameter("n", Ty::int())];
    let callable = generic_callable(declarations.clone());
    let TyKind::GenericFunc(generic) = callable.kind() else {
        unreachable!()
    };
    let instantiated: Ty = TyKind::Func(Box::new(generic.func.clone())).into();
    let f = function(
        block(
            vec![MirInstr::CallIndirect {
                dest: Reg(1),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: Some("n".to_string()),
                    value: None,
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: Some(instantiated),
                instantiated_args: vec![TyArg::Val(CtValue::Int(3))],
                spread: None,
            }],
            MirTerm::Return(None),
        ),
        2,
        &[(0, callable), (1, Ty::int())],
    );
    assert_eq!(verify(&program(f)), Vec::<String>::new());
}

#[test]
fn verifier_substitutes_retained_dependent_callable_arguments() {
    let declarations = vec![value_parameter("index", Ty::int())];
    let f = function(
        block(
            vec![MirInstr::CallIndirect {
                dest: Reg(3),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: vec![Reg(2)],
                kwargs: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: Some("index".to_string()),
                    value: Some(Reg(1)),
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::int())),
                instantiated_args: vec![TyArg::Val(CtValue::Int(0))],
                spread: None,
            }],
            MirTerm::Return(None),
        ),
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
        block(
            vec![MirInstr::CallIndirect {
                dest: Reg(3),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: vec![Reg(2)],
                kwargs: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: vec![MirParamArg {
                    name: Some("index".to_string()),
                    value: Some(Reg(1)),
                    expr: None,
                }],
                param_decls: declarations,
                instantiated_contract: Some(concrete_callable(Ty::int())),
                instantiated_args: vec![TyArg::Val(CtValue::Int(1))],
                spread: None,
            }],
            MirTerm::Return(None),
        ),
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
    } = &mut prog.functions[0].1.body.instrs[0]
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
    expect_finding(
        &prog,
        "a call through a generic callable carries no instantiation",
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
        error: Some(Ty::string_literal_of("s")),
        conventions: Vec::new(),
        ref_params: Vec::new(),
        ref_return: Some(Box::new(mojito::origin::RefSig {
            origin: mojito::origin::SigOrigin::Static,
            mutability: mojito::origin::SigMutability::Mutable,
            address_space: mojito::types::AddressSpaceSlot::GENERIC,
        })),
        transfers: TransferSet::default(),
    }));
    let result = TyKind::Ref(mojito::RefTy {
        referent: Ty::int(),
        origin: mojito::Origin::Untracked { mutable: false },
        mutability: mojito::Mutability::Immutable,
        address_space: mojito::types::AddressSpaceSlot::GENERIC,
    });
    let f = function(
        block(
            vec![MirInstr::CallIndirect {
                dest: Reg(1),
                callee: Reg(0),
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_contract: None,
                instantiated_args: Vec::new(),
                spread: None,
            }],
            MirTerm::Return(None),
        ),
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
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Int(1),
                },
                MirInstr::Call {
                    conventions: mojito::mir::MirArgConventions::default(),
                    dest: Reg(1),
                    func: FuncRef::named("callee"),
                    raises: None,
                    args: vec![Reg(0)],
                    kwargs: Vec::new(),
                    param_arg_regs: Vec::new(),
                    capture_accesses: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
            MirTerm::Return(None),
        ),
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
            block(
                vec![MirInstr::MethodCall {
                    conventions: mojito::mir::MirArgConventions::default(),
                    dest: Reg(1),
                    recv: Reg(0),
                    method: "__next__".to_string(),
                    resolved: Some("__trait_dispatch.__next__".to_string()),
                    raises: None,
                    reference_result,
                    result_adapter,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    recv_writes: true,
                    receiver_convention: None,
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: None,
                    refined_bound: None,
                }],
                MirTerm::Return(None),
            ),
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
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Carrier[origin: Origin[mut=True]]:\n    var slot: RefBox[Self.origin]\n\n@fieldwise_init\nstruct Two[origin: Origin[mut=True]]:\n    var a: Carrier[Self.origin]\n    var b: List[Int]\n\ndef stash_into_a(mut t: Two, var box: RefBox):\n    t.a.slot = box^\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var t = Two(Carrier(RefBox(whole)), List[Int]())\n    var local: List[Int] = [9]\n    ref alias = local\n    stash_into_a(t, RefBox(alias))\n    print(1)\n";
    let mut prog = lower_source(source);
    let domain = prog
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
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
            block(
                vec![MirInstr::Store {
                    place: MirPlace::root(0, Some(storage)),
                    src: Reg(0),
                }],
                MirTerm::Return(None),
            ),
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
            reference_ty(Ty::string_literal_of("s"), mojito::Mutability::Mutable),
        ),
        "store of ref StringLiteral[\"s\"] into storage of type Int",
    );

    // A capturing closure fits unqualified callable storage; an unqualified
    // callable does not fit capturing storage.
    let capturing = with_environment(mojito::CallableEnvironment::Capturing(
        mojito::CaptureOriginSet::empty(),
    ));
    let unqualified = with_environment(mojito::CallableEnvironment::Default);
    clean(&store(unqualified.clone(), capturing.clone()));
    expect_finding(&store(capturing, unqualified), "store of def(");
}

#[test]
fn verifier_rejects_a_method_call_spread_of_a_non_pack_register() {
    let spread_call = |resolved: Option<String>| {
        program(function(
            block(
                vec![MirInstr::MethodCall {
                    conventions: mojito::mir::MirArgConventions::default(),
                    dest: Reg(2),
                    recv: Reg(0),
                    method: "take".to_string(),
                    resolved,
                    raises: None,
                    reference_result: None,
                    result_adapter: None,
                    args: vec![Reg(1)],
                    kwargs: Vec::new(),
                    recv_writes: false,
                    receiver_convention: None,
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: Some(0),
                    refined_bound: None,
                }],
                MirTerm::Return(None),
            ),
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
            block(
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
            ),
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
fn slot_end_findings(blocks: MirBlock) -> Vec<String> {
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

/// A raise point: a raising call writing its error into [`try_region`]'s
/// error slot, `$v1`, and its error region, whose `then` region raises to
/// it.
fn raising_call() -> Vec<MirInstr> {
    vec![
        MirInstr::Call {
            conventions: mojito::mir::MirArgConventions::default(),
            dest: Reg(2),
            func: FuncRef::named("boom"),
            raises: Some(Raises {
                ty: Ty::int(),
                flag: Reg(3),
                error: 1,
            }),
            args: Vec::new(),
            kwargs: Vec::new(),
            param_arg_regs: Vec::new(),
            capture_accesses: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        },
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(3),
            then: Box::new(block(
                Vec::new(),
                MirTerm::TryRaise {
                    label: TRY,
                    values: Vec::new(),
                },
            )),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: Box::new(block(Vec::new(), MirTerm::Yield(Vec::new()))),
        },
    ]
}

/// The label of [`try_region`]'s `try`, which [`raising_call`] raises to.
const TRY: TryLabel = TryLabel(0);

/// A `try` labelled [`TRY`] around `body`, with `handler` as its handler.
fn try_region(body: MirBlock, handler: Option<MirBlock>) -> MirInstr {
    MirInstr::Try {
        label: TRY,
        error: Some(1),
        results: Vec::new(),
        body: Box::new(body),
        handler_args: Vec::new(),
        handler: handler.map(Box::new),
        else_args: Vec::new(),
        orelse: None,
    }
}

/// A drop or consume of a slot every path already dropped, consumed, or
/// moved destroys its value twice.
#[test]
fn verifier_rejects_a_drop_of_an_ended_slot() {
    let doubled = [define_x(), vec![DROP_X, DROP_X]].concat();
    let findings = slot_end_findings(block(doubled, MirTerm::Return(None)));
    assert_eq!(
        findings,
        [
            "MIR function 'test': drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
    let consumed = [define_x(), vec![DROP_X, MirInstr::ConsumeVar { var: 0 }]].concat();
    let findings = slot_end_findings(block(consumed, MirTerm::Return(None)));
    assert!(
        findings[0].contains("var.consume of slot $v0"),
        "{findings:?}"
    );
    let moved = || block(vec![move_x()], MirTerm::Yield(Vec::new()));
    let both_arms = block(
        [define_x(), vec![if_region(moved(), moved()), DROP_X]].concat(),
        MirTerm::Return(None),
    );
    assert_eq!(slot_end_findings(both_arms).len(), 1);
}

/// A slot one path leaves empty — ended in one arm, or stored in one arm
/// only after a `var.declare` — is not held on every path, as upstream's
/// `UninitializedValueScan` rejects a destroy of a value some path has not
/// initialized.
#[test]
fn verifier_rejects_a_drop_of_a_slot_some_path_does_not_hold() {
    let one_arm = block(
        [
            define_x(),
            vec![
                if_region(
                    block(vec![move_x()], MirTerm::Yield(Vec::new())),
                    block(Vec::new(), MirTerm::Yield(Vec::new())),
                ),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    );
    assert_eq!(
        slot_end_findings(one_arm),
        [
            "MIR function 'test': drop.var of slot $v0 ('x') that some path reaching it does not hold"
        ]
    );
    let declared = block(
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
    );
    assert_eq!(slot_end_findings(declared).len(), 1);
    let never_stored = block(vec![DROP_X], MirTerm::Return(None));
    assert_eq!(slot_end_findings(never_stored).len(), 1);
}

/// The findings of `blocks` about owned registers ended twice, in a
/// function whose parameter `x` and local `y` hold a struct `S` whose drop
/// runs code; `%r0` is `S` and `%r1` is `Bool`.
fn register_end_findings(blocks: MirBlock) -> Vec<String> {
    register_end_findings_in(blocks, &[], MirDeclarations::default())
}

/// [`register_end_findings`] over `declarations`, the registers `more`
/// typed besides `%r0` and `%r1`.
fn register_end_findings_in(
    blocks: MirBlock,
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
fn register_read_findings(blocks: MirBlock) -> Vec<String> {
    let s = Ty::new(TyKind::Struct("S".into(), Vec::new()));
    owned_register_findings(blocks, &[(2, s)], MirDeclarations::default())
        .into_iter()
        .filter(|finding| finding.contains("reads owned register"))
        .collect()
}

/// Every verifier finding of `blocks` in [`register_end_findings`]'
/// function.
fn owned_register_findings(
    blocks: MirBlock,
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
    assert!(register_end_findings(block(once, MirTerm::Return(None))).is_empty());
    let each_arm = [
        copy_x_and_condition(),
        vec![if_region(
            yields(vec![STORE_R0_IN_Y]),
            yields(vec![STORE_R0_IN_Y]),
        )],
    ]
    .concat();
    assert!(register_end_findings(block(each_arm, MirTerm::Return(None))).is_empty());
    let twice = [copy_x_and_condition(), vec![STORE_R0_IN_Y, STORE_R0_IN_Y]].concat();
    assert_eq!(
        register_end_findings(block(twice, MirTerm::Return(None))),
        [
            "MIR function 'test': var.store ends owned register %r0, which some path reaching it has ended"
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
        register_end_findings(block(after_one_arm, MirTerm::Return(None))),
        [
            "MIR function 'test': drop.reg ends owned register %r0, which some path reaching it has ended"
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
    assert!(register_read_findings(block(before, MirTerm::Return(None))).is_empty());
    let slot = [copy_x_and_condition(), vec![STORE_R0_IN_Y, load_y]].concat();
    assert!(register_read_findings(block(slot, MirTerm::Return(None))).is_empty());
    let after = [copy_x_and_condition(), vec![STORE_R0_IN_Y, copy_r0.clone()]].concat();
    assert_eq!(
        register_read_findings(block(after, MirTerm::Return(None))),
        [
            "MIR function 'test': value.copy reads owned register %r0, which some path reaching it has ended"
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
        register_read_findings(block(after_one_arm, MirTerm::Return(None))),
        [
            "MIR function 'test': value.copy reads owned register %r0, which some path reaching it has ended"
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
            param_decls: vec![TraitDeclaration::self_binder("Absorber")],
            refines: Vec::new(),
            requirements: vec![TraitRequirement {
                name: "absorb".into(),
                arity: 1,
                dispatch: dispatch.into(),
                param_names: vec!["t".into()],
                param_types: vec![Ty::new(TyKind::Struct("S".into(), Vec::new()))],
                defaults: vec![None],
                param_conventions: vec![Some(mojito::ast::ArgConvention::Var)],
                variadic_index: None,
                required: vec![true],
                positional_only: None,
                keyword_only: None,
                variadic: None,
                kw_variadic: None,
                variadic_convention: None,
                receiver_convention: None,
                param_decls: Vec::new(),
                return_type: Ty::none(),
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
        trait_param: None,
    });
    let absorb = MirInstr::MethodCall {
        conventions: mojito::mir::MirArgConventions::default(),
        dest: Reg(3),
        recv: Reg(2),
        method: "absorb".into(),
        resolved: Some(dispatch.into()),
        raises: None,
        reference_result: None,
        result_adapter: None,
        args: vec![Reg(0)],
        kwargs: Vec::new(),
        recv_writes: false,
        receiver_convention: None,
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
            block(instrs, MirTerm::Return(None)),
            &[(2, bound), (3, Ty::none())],
            declarations,
        ),
        [
            "MIR function 'test': drop.reg ends owned register %r0, which some path reaching it has ended"
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
            block(borrowed, MirTerm::Return(None)),
            &[(2, s.clone()), (3, pair)],
            MirDeclarations::default(),
        )
        .is_empty()
    );
    let owned = [copy_x_and_condition(), vec![tuple(vec![Reg(0)]), DROP_R0]].concat();
    assert_eq!(
        register_end_findings_in(
            block(owned, MirTerm::Return(None)),
            &[(3, Ty::new(TyKind::Tuple(vec![s])))],
            MirDeclarations::default(),
        ),
        [
            "MIR function 'test': drop.reg ends owned register %r0, which some path reaching it has ended"
        ]
    );
}

/// The `deinit`-receiver call of `S.__del__` on `recv`, its result `dest`.
fn deinit_call(dest: u32, recv: u32) -> MirInstr {
    MirInstr::MethodCall {
        conventions: mojito::mir::MirArgConventions::default(),
        dest: Reg(dest),
        recv: Reg(recv),
        method: "__deinit__".into(),
        resolved: Some("S.__del__".into()),
        raises: None,
        reference_result: None,
        result_adapter: None,
        args: Vec::new(),
        kwargs: Vec::new(),
        recv_writes: true,
        receiver_convention: Some(mojito::ast::ArgConvention::Deinit),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
        instantiated_args: Vec::new(),
        spread: None,
        refined_bound: None,
    }
}

/// A memory-only value is destroyed through its storage, as upstream's
/// `CheckLifetimes` tracks only a register-passable value as an SSA value:
/// a `drop.reg`, named or unnamed, or a `deinit`-receiver call of a
/// memory-only register is rejected, while one of a register-passable register, or a
/// `deinit` call on a slot's storage a forwarding `var.use` hands over, is
/// accepted.
#[test]
fn verifier_rejects_a_destroy_of_a_memory_only_register() {
    let findings = |instrs: Vec<MirInstr>| -> Vec<String> {
        owned_register_findings(
            block(instrs, MirTerm::Return(None)),
            &[(2, Ty::int()), (3, Ty::none())],
            MirDeclarations::default(),
        )
        .into_iter()
        .filter(|finding| finding.contains("memory-only type"))
        .collect()
    };
    let named_drop = |reg| MirInstr::Drop {
        reg: Reg(reg),
        destructor: Some("S.__del__".into()),
    };
    assert_eq!(
        findings([copy_x_and_condition(), vec![named_drop(0)]].concat()),
        [
            "MIR function 'test': drop.reg destroys %r0 of memory-only type S, which upstream destroys through its storage"
        ]
    );
    let unnamed_drop = MirInstr::Drop {
        reg: Reg(0),
        destructor: None,
    };
    assert_eq!(
        findings([copy_x_and_condition(), vec![unnamed_drop]].concat()),
        [
            "MIR function 'test': drop.reg destroys %r0 of memory-only type S, which upstream destroys through its storage"
        ]
    );
    assert_eq!(
        findings([copy_x_and_condition(), vec![deinit_call(3, 0)]].concat()),
        [
            "MIR function 'test': call.method deinit receiver destroys %r0 of memory-only type S, which upstream destroys through its storage"
        ]
    );
    let int = MirInstr::Const {
        dest: Reg(2),
        k: Const::Int(1),
    };
    assert!(findings(vec![int, named_drop(2)]).is_empty());
    let forwarded = MirInstr::UseVar {
        dest: Reg(0),
        var: 0,
        mode: UseMode::Forward,
        constructor: None,
    };
    assert!(findings(vec![forwarded, deinit_call(3, 0)]).is_empty());
}

/// A `deinit`-receiver call names the storage it consumes by its operand,
/// upstream's `DeinitMem` `!lit.ref`: one whose receiver is a `ref.make`
/// of `x` passes.
#[test]
fn verifier_accepts_a_deinit_receiver_named_by_its_operand() {
    let s = Ty::new(TyKind::Struct("S".into(), Vec::new()));
    let x = MirPlace::root(0, Some(s.clone()));
    let addressed = vec![
        MirInstr::MakeRef {
            dest: Reg(2),
            place: x,
        },
        deinit_call(3, 2),
    ];
    let findings: Vec<String> = owned_register_findings(
        block(addressed, MirTerm::Return(None)),
        &[
            (2, reference_ty(s, mojito::Mutability::Mutable)),
            (3, Ty::none()),
        ],
        MirDeclarations::default(),
    )
    .into_iter()
    .filter(|finding| finding.contains("deinit receiver"))
    .collect();
    assert_eq!(findings, Vec::<String>::new());
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
    let both_stored = block(
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
    );
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
    let restored = block(restored, MirTerm::Return(None));
    assert_eq!(slot_end_findings(restored), Vec::<String>::new());
    // The body moves `x` after its last raise, so its handler still owns it.
    let body = block(
        [raising_call(), vec![move_x()]].concat(),
        MirTerm::TryYield(Vec::new()),
    );
    let handler = block(vec![DROP_X], MirTerm::TryYield(Vec::new()));
    let raising = block(
        [define_x(), vec![try_region(body, Some(handler))]].concat(),
        MirTerm::Return(None),
    );
    assert_eq!(slot_end_findings(raising), Vec::<String>::new());
}

/// The part-end findings `verify` reports for a function over slot 0
/// (`x`) and the registers of [`slot_end_findings`].
fn part_end_findings(blocks: MirBlock) -> Vec<String> {
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
        part_end_findings(block(whole, MirTerm::Return(None))),
        [
            "MIR function 'test': drop.var of slot $v0 ('x') whose part `a` some path reaching it has ended"
        ]
    );
    let one_arm = block(
        [
            define_x(),
            vec![
                if_region(yields(vec![move_x_field("a")]), yields(Vec::new())),
                DROP_X,
            ],
        ]
        .concat(),
        MirTerm::Return(None),
    );
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
    let findings = part_end_findings(block(part, MirTerm::Return(None)));
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
        part_end_findings(block(refilled, MirTerm::Return(None))),
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
        part_end_findings(block(consumed, MirTerm::Return(None))),
        Vec::<String>::new()
    );
}

const LOOP: Option<LoopLabel> = Some(LoopLabel(0));

/// An `if` on r1 with `then` and `orelse` as its arms.
fn if_region(then: MirBlock, orelse: MirBlock) -> MirInstr {
    MirInstr::If {
        results: Vec::new(),
        cond: Reg(1),
        then: Box::new(then),
        elifs: Vec::new(),
        else_args: Vec::new(),
        orelse: Box::new(orelse),
    }
}

/// A loop labelled [`LOOP`] around `body`.
fn loop_region(body: MirBlock) -> MirInstr {
    MirInstr::Loop {
        label: LOOP,
        operands: Vec::new(),
        args: Vec::new(),
        results: Vec::new(),
        body: Box::new(body),
    }
}

/// One block running `instrs` and ending in `term`, as a region.
const fn region(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
    block(instrs, term)
}

const fn yields(instrs: Vec<MirInstr>) -> MirBlock {
    region(instrs, MirTerm::Yield(Vec::new()))
}

const fn breaks(instrs: Vec<MirInstr>) -> MirBlock {
    region(
        instrs,
        MirTerm::Break {
            label: LOOP,
            values: Vec::new(),
        },
    )
}

/// `main`'s body: define `x`, run `ops`, then drop `x`.
fn defined_then_dropped(ops: Vec<MirInstr>) -> MirBlock {
    block(
        [define_x(), ops, vec![DROP_X]].concat(),
        MirTerm::Return(None),
    )
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
            "MIR function 'test': drop.var of slot $v0 ('x') that some path reaching it does not hold"
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
    let redropped = block(
        [define_x(), vec![redropped]].concat(),
        MirTerm::Return(None),
    );
    assert_eq!(slot_end_findings(redropped).len(), 1);
    let matched = MirInstr::Match {
        results: Vec::new(),
        cases: vec![MirCase {
            args: Vec::new(),
            body: region(vec![move_x()], MirTerm::MatchComplete(Vec::new())),
        }],
        else_args: Vec::new(),
        orelse: Box::new(yields(vec![move_x()])),
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
    let findings = slot_end_findings(block(
        [define_x(), vec![in_arm]].concat(),
        MirTerm::Return(None),
    ));
    assert_eq!(
        findings,
        [
            "MIR function 'test': drop.var of slot $v0 ('x') that some path reaching it does not hold"
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
    let refilled = block([define_x(), vec![looped]].concat(), MirTerm::Return(None));
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
        orelse: Box::new(yields(vec![DROP_X])),
    };
    let next = block([define_x(), vec![matched]].concat(), MirTerm::Return(None));
    assert_eq!(slot_end_findings(next), Vec::<String>::new());
}

#[test]
fn verifier_rejects_a_vector_bound_to_its_storage_slot() {
    // A `!kgen.simd` slot holds the storage, never the `SIMD` it stores, as
    // the checker types a literal conversion at the storage it names.
    let vector = mojito::types::canonical_simd_ty(mojito::ast::Dtype::Int32, 1);
    let storage = mojito::types::storage_ty(&vector);
    let f = function(
        block(
            vec![
                MirInstr::Const {
                    dest: Reg(0),
                    k: Const::Value(CtValue::Simd {
                        dtype: mojito::ast::Dtype::Int32,
                        lanes: vec![mojito::ct::CtLane::Int(5)],
                    }),
                },
                MirInstr::DefVar {
                    var: 0,
                    src: Reg(0),
                    binding_ty: Some(storage),
                    in_place: false,
                },
            ],
            MirTerm::Return(None),
        ),
        1,
        &[(0, vector)],
    );
    expect_finding(&program(f), "binding of Int32 to a slot of type");
}

const DEF_BOUND_APPLY: &str = "
def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)

def double(x: Int) -> Int:
    return x * 2

def main():
    print(apply(double, 5))
";

/// The `__call__` requirement call `apply`'s `f(x)` lowers to.
fn bound_call(program: &mut MirProgram) -> &mut MirInstr {
    program
        .functions
        .iter_mut()
        .filter(|(name, _)| name == "apply")
        .flat_map(|(_, function)| leaf_instructions_mut(&mut function.body))
        .find(|instruction| {
            matches!(instruction, MirInstr::MethodCall { method, .. } if method == "__call__")
        })
        .expect("a call through a `def(...)` bound lowers to its `__call__` requirement call")
}

fn apply_findings(program: &MirProgram) -> Vec<String> {
    verify(program)
        .into_iter()
        .filter(|error| error.contains("'apply'"))
        .collect()
}

#[test]
fn verifier_checks_a_def_bound_requirement_call() {
    let mut program = lower_linked_source(DEF_BOUND_APPLY);
    let (resolved, recv_writes) = match bound_call(&mut program) {
        MirInstr::MethodCall {
            resolved,
            recv_writes,
            ..
        } => (resolved.clone(), *recv_writes),
        _ => (None, true),
    };
    assert_eq!(
        resolved.as_deref(),
        Some("__trait_dispatch.__call__$ov$Int")
    );
    assert!(
        !recv_writes,
        "a `def(...)` bound's `__call__` reads its receiver"
    );
    assert_eq!(apply_findings(&program), Vec::<String>::new());

    let mut wrong_target = program.clone();
    if let MirInstr::MethodCall { resolved, .. } = bound_call(&mut wrong_target) {
        *resolved = Some("__trait_dispatch.__call__$ov$Bool".to_string());
    }
    assert!(
        apply_findings(&wrong_target)
            .iter()
            .any(|error| error.contains("does not match callable contract")),
        "{:?}",
        apply_findings(&wrong_target)
    );

    let mut writing = program.clone();
    if let MirInstr::MethodCall { recv_writes, .. } = bound_call(&mut writing) {
        *recv_writes = true;
    }
    assert!(
        apply_findings(&writing)
            .iter()
            .any(|error| error.contains("takes its receiver `read`"))
    );

    let mut indirect = program;
    let call = bound_call(&mut indirect);
    if let MirInstr::MethodCall {
        dest,
        recv,
        resolved,
        args,
        ..
    } = call.clone()
    {
        *call = MirInstr::CallIndirect {
            dest,
            callee: recv,
            resolved,
            raises: None,
            args,
            kwargs: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_contract: None,
            instantiated_args: Vec::new(),
            spread: None,
        };
    }
    assert!(
        apply_findings(&indirect)
            .iter()
            .any(|error| error.contains("is its `__call__` requirement call"))
    );
}

const GENERIC_DEF_BOUND_APPLY: &str = "
def apply[F: def[n: Int](Int) -> Int](f: F, x: Int) -> Int:
    return f[2](x)

def add[n: Int](value: Int) -> Int:
    return value + n

def main():
    print(apply(add, 40))
";

#[test]
fn verifier_checks_a_generic_def_bound_requirement_call() {
    let mut program = lower_linked_source(GENERIC_DEF_BOUND_APPLY);
    let (resolved, param_decls) = match bound_call(&mut program) {
        MirInstr::MethodCall {
            resolved,
            param_decls,
            ..
        } => (resolved.clone(), param_decls.len()),
        _ => (None, 0),
    };
    assert_eq!(
        resolved.as_deref(),
        Some("__trait_dispatch.__call__$ov$Int")
    );
    assert_eq!(
        param_decls, 1,
        "a generic bound's requirement call carries the bound's binders"
    );
    assert_eq!(apply_findings(&program), Vec::<String>::new());

    let mut unparameterized = program.clone();
    if let MirInstr::MethodCall { param_decls, .. } = bound_call(&mut unparameterized) {
        param_decls.clear();
    }
    assert!(
        apply_findings(&unparameterized)
            .iter()
            .any(|error| error.contains("the bound's compile-time parameters")),
        "{:?}",
        apply_findings(&unparameterized)
    );

    let mut uninstantiated = program.clone();
    if let MirInstr::MethodCall {
        instantiated_args, ..
    } = bound_call(&mut uninstantiated)
    {
        assert!(
            !instantiated_args.is_empty(),
            "a generic bound's requirement call carries its instantiation"
        );
        instantiated_args.clear();
    }
    assert!(
        apply_findings(&uninstantiated)
            .iter()
            .any(|error| error.contains("bound carries no instantiation")),
        "{:?}",
        apply_findings(&uninstantiated)
    );

    let mut indirect = program;
    let call = bound_call(&mut indirect);
    if let MirInstr::MethodCall {
        dest,
        recv,
        args,
        param_arg_regs,
        param_decls,
        ..
    } = call.clone()
    {
        *call = MirInstr::CallIndirect {
            dest,
            callee: recv,
            resolved: None,
            raises: None,
            args,
            kwargs: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs,
            param_decls,
            instantiated_contract: None,
            instantiated_args: Vec::new(),
            spread: None,
        };
    }
    assert!(
        apply_findings(&indirect)
            .iter()
            .any(|error| error.contains("is its `__call__` requirement call"))
    );
}
