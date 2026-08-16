use mojito::SourceSpan;
use mojito::ast::ArgConvention;
use mojito::checked::CheckedConst;
use mojito::literal::{FloatLiteral, IntLiteral};
use mojito::mir::{
    self, Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    MirProgram, MirStructDeclaration, MirTerm, Reg, SpanTable, UseMode,
};
use mojito::types::{ParamDecl, Ty};
use std::collections::{HashMap, HashSet};

fn identity_program() -> MirProgram {
    let function = MirFunction {
        blocks: vec![MirBlock {
            instrs: vec![MirInstr::UseVar {
                dest: Reg(0),
                var: 0,
                mode: UseMode::Copy,
            }],
            term: MirTerm::Return(Some(Reg(0))),
        }],
        n_regs: 1,
        n_vars: 1,
        var_names: vec!["value".into()],
        n_params: 1,
        param_types: vec![Ty::Int],
        owned_params: vec![false],
        deinit_params: vec![false],
        ref_params: vec![false],
        returns_reference: false,
        var_tys: HashMap::from([(0, Ty::Int)]),
        ret_ty: Some(Ty::Int),
        raises: false,
        error_ty: None,
        spans: SpanTable(HashMap::from([(
            0,
            (
                SourceSpan::new(Some("identity.mojo".into()), (44, 49)),
                Some(0),
            ),
        )])),
        reg_types: HashMap::from([(0, Ty::Int)]),
    };
    MirProgram {
        functions: vec![("identity".into(), function)],
        declarations: MirDeclarations::default(),
        invariant_errors: Vec::new(),
    }
}

fn control_flow_program() -> MirProgram {
    let literal = |dest: u32, value: &str| {
        let literal = value
            .strip_prefix('-')
            .map_or_else(
                || IntLiteral::parse_radix(value, 10),
                |digits| IntLiteral::parse_radix(digits, 10).map(|value| value.neg()),
            )
            .expect("literal");
        vec![
            MirInstr::Const {
                dest: Reg(dest),
                k: Const::IntLiteral(literal),
            },
            MirInstr::MaterializeLiteral {
                dest: Reg(dest + 1),
                value: Reg(dest),
                target: Ty::Int,
            },
        ]
    };
    let function = MirFunction {
        blocks: vec![
            MirBlock {
                instrs: vec![MirInstr::UseVar {
                    dest: Reg(0),
                    var: 0,
                    mode: UseMode::Copy,
                }],
                term: MirTerm::Branch {
                    cond: Reg(0),
                    then_b: 1,
                    else_b: 2,
                },
            },
            MirBlock {
                instrs: literal(1, "7"),
                term: MirTerm::Return(Some(Reg(2))),
            },
            MirBlock {
                instrs: literal(3, "-3"),
                term: MirTerm::Return(Some(Reg(4))),
            },
        ],
        n_regs: 5,
        n_vars: 1,
        var_names: vec!["flag".into()],
        n_params: 1,
        param_types: vec![Ty::Bool],
        owned_params: vec![false],
        deinit_params: vec![false],
        ref_params: vec![false],
        returns_reference: false,
        var_tys: HashMap::from([(0, Ty::Bool)]),
        ret_ty: Some(Ty::Int),
        raises: false,
        error_ty: None,
        spans: SpanTable::default(),
        reg_types: HashMap::from([
            (0, Ty::Bool),
            (1, Ty::IntLiteral),
            (2, Ty::Int),
            (3, Ty::IntLiteral),
            (4, Ty::Int),
        ]),
    };
    MirProgram {
        functions: vec![("choose".into(), function)],
        declarations: MirDeclarations::default(),
        invariant_errors: Vec::new(),
    }
}

fn metadata_program() -> MirProgram {
    let mut program = identity_program();
    program.declarations = MirDeclarations {
        structs: vec![MirStructDeclaration {
            name: "Box".into(),
            fields: vec![("value".into(), Ty::Int)],
            mut_self_methods: HashSet::from(["set".into()]),
            fieldwise_init: true,
            param_decls: vec![ParamDecl::Type {
                name: "T".into(),
                bounds: vec!["Copyable".into()],
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            explicit_destroy_message: Some("call destroy() first".into()),
            explicit_destructors: HashMap::from([("_finish".into(), true)]),
        }],
        functions: vec![MirFunctionDeclaration {
            lowered_name: "scale".into(),
            param_names: vec!["value".into(), "by".into()],
            param_types: vec![Ty::Int, Ty::Float64],
            defaults: vec![
                None,
                Some(CheckedConst::Float(
                    FloatLiteral::parse_exact("157/50").unwrap(),
                )),
            ],
            required: vec![true, false],
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: Some(1),
            param_decls: Vec::new(),
            has_receiver: false,
            receiver_convention: None,
            param_conventions: vec![None, Some(ArgConvention::Read)],
            ret_ty: Ty::Int,
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: vec![false, false],
        }],
    };
    program
}

#[test]
fn minimal_program_has_a_stable_snapshot() {
    let text = mir::text::disassemble(&identity_program()).expect("disassemble verified MIR");
    assert_eq!(text, include_str!("snapshots/mir/minimal.mir"));
    assert_eq!(text, mir::text::disassemble(&identity_program()).unwrap());
}

#[test]
fn control_flow_and_literals_have_a_stable_snapshot() {
    let text = mir::text::disassemble(&control_flow_program()).expect("disassemble verified MIR");
    assert_eq!(text, include_str!("snapshots/mir/control_flow.mir"));
}

#[test]
fn declaration_metadata_has_a_stable_snapshot() {
    let text = mir::text::disassemble(&metadata_program()).expect("disassemble verified MIR");
    assert_eq!(text, include_str!("snapshots/mir/metadata.mir"));
}

#[test]
fn invalid_program_is_rejected_without_partial_text() {
    let program = MirProgram {
        functions: Vec::new(),
        declarations: MirDeclarations::default(),
        invariant_errors: vec!["lowering lost a checked fact".into()],
    };
    let error = mir::text::disassemble(&program).expect_err("invalid MIR must fail");
    assert_eq!(error.findings(), &["lowering lost a checked fact"]);
    assert!(error.to_string().contains("cannot disassemble MIR"));
}

#[test]
fn output_uses_lf_and_exactly_one_trailing_newline() {
    let text = mir::text::disassemble(&identity_program()).unwrap();
    assert!(!text.contains('\r'));
    assert!(text.ends_with('\n'));
    assert!(!text.ends_with("\n\n"));
}

#[test]
fn map_insertion_order_does_not_change_output() {
    let first = identity_program();
    let mut second = identity_program();
    second.functions[0].1.var_tys = [(0, Ty::Int)].into_iter().collect();
    second.functions[0].1.reg_types = [(0, Ty::Int)].into_iter().collect();
    assert_eq!(
        mir::text::disassemble(&first).unwrap(),
        mir::text::disassemble(&second).unwrap()
    );
}

#[test]
fn duplicate_function_identity_is_rejected() {
    let mut program = identity_program();
    let duplicate = identity_program().functions.pop().unwrap();
    program.functions.push(duplicate);
    let error = mir::text::disassemble(&program).expect_err("duplicate identity must fail");
    assert!(
        error
            .findings()
            .iter()
            .any(|finding| finding.contains("duplicate MIR function name"))
    );
}
