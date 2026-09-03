use super::*;
use crate::mir::text::write;

fn program_with(functions: Vec<(String, MirFunction)>) -> MirProgram {
    MirProgram {
        functions,
        declarations: MirDeclarations::default(),
        invariant_errors: Vec::new(),
    }
}

fn function_with(reg_types: Vec<Ty>, instrs: Vec<MirInstr>) -> MirFunction {
    MirFunction {
        blocks: vec![MirBlock {
            instrs,
            term: MirTerm::FallOff,
        }],
        n_regs: reg_types.len() as u32,
        n_vars: 0,
        var_names: Vec::new(),
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
        spans: SpanTable(HashMap::new()),
        reg_types: reg_types
            .into_iter()
            .enumerate()
            .map(|(index, ty)| (index as u32, ty))
            .collect(),
    }
}

/// Print → parse → print must reproduce the canonical text byte-for-byte.
fn assert_reprints(program: &MirProgram) {
    let text = write::program(program);
    let parsed =
        artifact(text.as_bytes(), "unit.mir".to_string()).expect("parse canonical artifact");
    assert_eq!(write::program(&parsed.program), text);
}

fn diagnostics(text: &str) -> Vec<String> {
    artifact(text.as_bytes(), "unit.mir".to_string())
        .expect_err("expected artifact diagnostics")
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

fn artifact_with_register_type(ty_text: &str) -> String {
    format!(
        "mojito-mir 1.0\nartifact {{\n  features: [],\n  files: [],\n  structs: [],\n  \
         decls: [],\n  functions: [\n    fn {{\n      name: main,\n      registers: 1,\n      \
         vars: 0,\n      var_names: [],\n      params: 0,\n      param_types: [],\n      \
         owned_params: [],\n      deinit_params: [],\n      ref_params: [],\n      \
         returns_reference: false,\n      var_types: [],\n      return_type: present(None),\n      \
         raises: false,\n      error_type: absent,\n      \
         register_types: [reg_type {{ reg: %r0, type: {ty_text} }}],\n      locations: [],\n      \
         blocks: []\n    }}\n  ]\n}}\n"
    )
}

#[test]
fn type_families_reprint_byte_identically() {
    let capturing = CallableEnvironment::Capturing(CaptureOriginSet::concrete([
        CaptureOrigin {
            origin: Origin::Place(OriginPlace {
                root: OwnerId(1),
                path: vec![OriginSeg::Field("data".into()), OriginSeg::AnyIndex],
            }),
            access: CaptureAccess::Write,
        },
        CaptureOrigin {
            origin: Origin::Param(OriginParamId(4)),
            access: CaptureAccess::Read,
        },
    ]));
    let func_ty = Ty::Func {
        environment: capturing,
        params: vec![Ty::Int],
        names: vec!["value".into()],
        ret: Box::new(Ty::Bool),
        required: vec![true],
        variadic: Some(Box::new(Ty::Int)),
        kw_variadic: None,
        positional_only: Some(0),
        keyword_only: None,
        raises: true,
        error: Some(Box::new(Ty::Error)),
        conventions: vec![Some(mojito_ast::ast::ArgConvention::Mut)],
        ref_params: Box::new(vec![Some(RefSig {
            origin: SigOrigin::Projected(
                Box::new(SigOrigin::Param(0)),
                vec![OriginSeg::Interior("element".into())],
            ),
            mutability: SigMutability::BoolParam(1),
        })]),
        ref_return: Some(Box::new(RefSig {
            origin: SigOrigin::Union(vec![SigOrigin::Self_, SigOrigin::Static]),
            mutability: SigMutability::Infer,
        })),
        transfers: TransferSet(vec![TransferEffect {
            dest: SigOrigin::Param(1),
            src: SigOrigin::Bound(Origin::Static),
            src_is_place: true,
            mutable: false,
        }]),
    };
    let generic_ty = Ty::GenericFunc {
        environment: CallableEnvironment::Thin,
        decls: vec![
            ParamDecl::Type {
                name: "T".into(),
                bounds: vec!["Copyable".into()],
                callable_bound: None,
                default: Some(Box::new(Ty::Int)),
                infer_only: true,
                variadic: false,
                constraints: vec![
                    GenericConstraint::WithMessage(
                        Box::new(GenericConstraint::And(
                            Box::new(GenericConstraint::Conforms {
                                param: "T".into(),
                                trait_name: "Movable".into(),
                            }),
                            Box::new(GenericConstraint::Trivial(
                                TrivialLifecycle::Copyable,
                                ConstraintOperand::Param("T".into()),
                            )),
                        )),
                        "T must copy".into(),
                    ),
                    GenericConstraint::Or(
                        Box::new(GenericConstraint::Not(Box::new(GenericConstraint::Bool(
                            false,
                        )))),
                        Box::new(GenericConstraint::ConformsPack {
                            param: "Ts".into(),
                            trait_name: "Movable".into(),
                        }),
                    ),
                    GenericConstraint::PackPredicate {
                        param: "Ts".into(),
                        predicate: PackPredicateRef::Alias("IsNice".into()),
                        all: true,
                    },
                    GenericConstraint::PackPredicate {
                        param: "Ts".into(),
                        predicate: PackPredicateRef::Trivial(TrivialLifecycle::Deinitable),
                        all: false,
                    },
                    GenericConstraint::PackContains {
                        param: "Ts".into(),
                        element: ConstraintOperand::Type(Ty::Int),
                    },
                    GenericConstraint::Le(
                        ConstraintOperand::Value(CtValue::Int(3)),
                        ConstraintOperand::PackLength("Ts".into()),
                    ),
                    GenericConstraint::Ne(
                        ConstraintOperand::Param("T".into()),
                        ConstraintOperand::Type(Ty::Bool),
                    ),
                ],
            },
            ParamDecl::Value {
                name: "width".into(),
                ty: Box::new(Ty::Int),
                default: Some(CtExpr::Add(
                    Box::new(CtExpr::Param("n".into())),
                    Box::new(CtExpr::Value(CtValue::Int(1))),
                )),
                callable_default: Some(CallableDefault::If {
                    condition: CtExpr::Value(CtValue::Bool(true)),
                    then_value: Box::new(CallableDefault::Symbol("default_fn".into())),
                    else_value: Box::new(CallableDefault::Parameter("F".into())),
                }),
                infer_only: false,
                variadic: true,
                constraints: Vec::new(),
            },
        ],
        params: Vec::new(),
        names: Vec::new(),
        ret: Box::new(Ty::None),
        required: Vec::new(),
        variadic: None,
        kw_variadic: Some(Box::new(Ty::Int)),
        positional_only: None,
        keyword_only: Some(0),
        raises: false,
        error: None,
        conventions: Vec::new(),
        ref_params: Box::new(vec![None]),
        ref_return: None,
        transfers: TransferSet(Vec::new()),
    };
    let struct_ty = Ty::Struct(
        "Fancy".into(),
        vec![
            TyArg::Ty(Ty::Tuple(vec![Ty::Int, Ty::Bool])),
            TyArg::Val(CtValue::Struct {
                name: "Layout".into(),
                fields: vec![
                    (
                        "shape".into(),
                        CtValue::Tuple(vec![CtValue::Int(2), CtValue::UInt(3)]),
                    ),
                    ("tag".into(), CtValue::Str("row".into())),
                ],
            }),
            TyArg::Val(CtValue::List(vec![
                CtValue::Float(0x3ff0000000000000),
                CtValue::IntLiteral(parse_int_literal("-12345678901234567890").unwrap()),
                CtValue::FloatLiteral(FloatLiteral::parse_exact("157/50").unwrap()),
                CtValue::Dtype(Dtype::Float32),
                CtValue::Type(Box::new(Ty::Int)),
                CtValue::Reflected(Box::new(Ty::Bool)),
                CtValue::Param("N".into()),
            ])),
            TyArg::Origin(Origin::union([
                Origin::Param(OriginParamId(0)),
                Origin::SelfParam,
            ])),
        ],
    );
    let program = program_with(vec![(
        "types".into(),
        function_with(
            vec![
                Ty::Overload(vec![Ty::Int, Ty::Bool]),
                Ty::Param {
                    name: "T".into(),
                    bounds: vec!["Copyable".into(), "Movable".into()],
                    callable_bound: Some(Box::new(func_ty.clone())),
                },
                Ty::Assoc {
                    base: Box::new(Ty::Param {
                        name: "C".into(),
                        bounds: vec!["Iterable".into()],
                        callable_bound: None,
                    }),
                    name: "IteratorType".into(),
                    args: vec![TyArg::Origin(Origin::SelfParam)],
                },
                Ty::Dependent(DependentType::Indexed {
                    elements: vec![Ty::Int, Ty::Bool],
                    index: CtExpr::FloorDiv(
                        Box::new(CtExpr::Neg(Box::new(CtExpr::Param("i".into())))),
                        Box::new(CtExpr::Value(CtValue::Int(2))),
                    ),
                }),
                func_ty,
                generic_ty,
                struct_ty,
                Ty::Simd {
                    dtype: Dtype::Float32,
                    width: 4,
                },
                Ty::ComptimeList(Box::new(Ty::Int)),
                Ty::RuntimePack(vec![Ty::Int, Ty::Bool]),
                Ty::VariadicPack(Box::new(Ty::Int)),
                Ty::Variant(vec![Ty::Int, Ty::None]),
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::Place {
                        place: OriginPlace {
                            root: OwnerId(0),
                            path: vec![OriginSeg::Subtree],
                        },
                        mutable: true,
                    },
                },
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::Param {
                        id: OriginParamId(2),
                        mutability: Mutability::Param(OriginParamId(3)),
                        interior: vec!["element".into()],
                        subtree: true,
                    },
                },
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::SelfPlace {
                        mutability: Mutability::Immutable,
                        interior: Vec::new(),
                        subtree: false,
                    },
                },
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::Static,
                },
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::Untracked { mutable: false },
                },
                Ty::Pointer {
                    element: Box::new(Ty::Int),
                    origin: PointerOrigin::UnsafeAny { mutable: true },
                },
                Ty::Ref(RefTy {
                    referent: Box::new(Ty::Struct("List".into(), vec![TyArg::Ty(Ty::Int)])),
                    origin: Origin::Untracked { mutable: true },
                    mutability: Mutability::Mutable,
                }),
            ],
            Vec::new(),
        ),
    )]);
    assert_reprints(&program);
}

#[test]
fn constant_families_reprint_byte_identically() {
    let constants = vec![
        Const::Int(i64::MIN),
        Const::Float(-0.0),
        Const::IntLiteral(parse_int_literal("-123456789012345678901234567890").unwrap()),
        Const::FloatLiteral(FloatLiteral::parse_exact("-1/3").unwrap()),
        Const::FloatLiteral(FloatLiteral::parse_exact("-0.0").unwrap()),
        Const::FloatLiteral(FloatLiteral::parse_exact("42.0").unwrap()),
        Const::Bool(true),
        Const::Str("line\n\"quoted\"\t\u{7}".into()),
        Const::Function("needs quoting!".into()),
        Const::None,
    ];
    let reg_types = vec![Ty::Int; constants.len()];
    let instrs = constants
        .into_iter()
        .enumerate()
        .map(|(index, k)| MirInstr::Const {
            dest: Reg(index as u32),
            k,
        })
        .collect();
    let program = program_with(vec![("consts".into(), function_with(reg_types, instrs))]);
    assert_reprints(&program);
}

#[test]
fn unknown_value_grammar_tags_are_diagnosed() {
    assert!(
        diagnostics(&artifact_with_register_type("Frob"))
            .iter()
            .any(|message| message.contains("unknown type `Frob`"))
    );
    assert!(
        diagnostics(&artifact_with_register_type(
            "simd { dtype: int99, width: 4 }"
        ))
        .iter()
        .any(|message| message.contains("unknown dtype `int99`"))
    );
    assert!(
        diagnostics(&artifact_with_register_type("simd { dtype: int }"))
            .iter()
            .any(|message| message.contains("missing required field `width`"))
    );
    assert!(
        diagnostics(&artifact_with_register_type(
            "struct_type { name: Box, arguments: [value_arg(ct_float_literal(1/0))] }"
        ))
        .iter()
        .any(|message| message.contains("expected exact float literal"))
    );
    assert!(
        diagnostics(&artifact_with_register_type(
            "param { name: T, bounds: [], callable_bound: absent, extra: 1 }"
        ))
        .iter()
        .any(|message| message.contains("unknown field `extra`"))
    );
}

fn sample_place(root: u32) -> MirPlace {
    MirPlace {
        root,
        root_ty: Some(Ty::Struct("List".into(), vec![TyArg::Ty(Ty::Int)])),
        proj: vec![
            Proj::Field("data".into()),
            Proj::Index(Reg(7)),
            Proj::ConstIndex(1),
            Proj::Variant(0),
            Proj::UninitPayload,
        ],
        projection_tys: vec![Ty::Int, Ty::Int, Ty::Int, Ty::Int, Ty::Int],
        ty: Some(Ty::Int),
        through: Some(9),
    }
}

fn sample_subscript_call() -> MirSubscriptCall {
    MirSubscriptCall {
        target: "List::__getitem__".into(),
        raises: Some(Ty::Error),
        result_ty: Ty::Int,
        receiver_requires_place: true,
        receiver_convention: Some(ArgConvention::Mut),
        arguments: vec![
            CheckedCallArgument {
                source: CheckedCallArgumentSource::Positional(0),
                parameter_ty: Ty::Int,
                requires_place: false,
                convention: None,
            },
            CheckedCallArgument {
                source: CheckedCallArgumentSource::Keyword(1),
                parameter_ty: Ty::Bool,
                requires_place: true,
                convention: Some(ArgConvention::Ref),
            },
            CheckedCallArgument {
                source: CheckedCallArgumentSource::Default,
                parameter_ty: Ty::Int,
                requires_place: false,
                convention: Some(ArgConvention::Imm),
            },
        ],
        capture_accesses: vec![MirCaptureAccess {
            root: 3,
            path: vec![OriginSeg::Field("buffer".into()), OriginSeg::AnyIndex],
            access: CaptureAccess::Write,
        }],
        reference_result: Some(RefTy {
            referent: Box::new(Ty::Int),
            origin: Origin::Place(OriginPlace {
                root: OwnerId(3),
                path: vec![OriginSeg::Interior("element".into())],
            }),
            mutability: Mutability::Mutable,
        }),
        param_arg_regs: vec![
            MirParamArg {
                name: Some("T".into()),
                value: None,
            },
            MirParamArg {
                name: None,
                value: Some(Reg(5)),
            },
        ],
        param_decls: vec![ParamDecl::Type {
            name: "T".into(),
            bounds: vec!["Copyable".into()],
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }],
    }
}

fn sample_iterator_call() -> CheckedIteratorCall {
    CheckedIteratorCall {
        target: "_ListIter::__next__".into(),
        result_ty: Ty::Int,
        reference_result: Some(RefTy {
            referent: Box::new(Ty::Int),
            origin: Origin::SelfParam,
            mutability: Mutability::Immutable,
        }),
        raises: Some(Ty::Error),
        result_adapter: Some(CheckedResultAdapter::CopyIteratorReference),
    }
}

#[test]
fn instruction_families_reprint_byte_identically() {
    let place = sample_place;
    let interior = |root: u32| MirInteriorOrigin {
        root,
        path: vec![OriginSeg::Interior("element".into()), OriginSeg::Subtree],
    };
    let instrs = vec![
        MirInstr::EstablishLoans {
            reference: 0,
            loans: vec![
                MirLoan {
                    place: place(1),
                    mutable: true,
                    interior: Some(interior(1)),
                },
                MirLoan {
                    place: place(2),
                    mutable: false,
                    interior: None,
                },
            ],
            marker: Reg(0),
            dest_interior: Some(interior(0)),
        },
        MirInstr::InvalidateInteriors {
            base: interior(2),
            except: Some(3),
            include_base_generation: true,
            marker: Reg(1),
        },
        MirInstr::MakeRef {
            dest: Reg(2),
            place: place(0),
        },
        MirInstr::ReadRef {
            dest: Reg(3),
            reference: Reg(2),
        },
        MirInstr::CopyValue {
            dest: Reg(4),
            value: Reg(3),
        },
        MirInstr::WriteRef {
            reference: Reg(2),
            value: Reg(4),
        },
        MirInstr::MakeClosure {
            dest: Reg(5),
            function: "outer::lambda#1".into(),
            captures: vec![
                MirClosureCapture {
                    place: place(0),
                    mode: MirCaptureMode::Reference,
                },
                MirClosureCapture {
                    place: place(1),
                    mode: MirCaptureMode::Copy,
                },
                MirClosureCapture {
                    place: place(2),
                    mode: MirCaptureMode::Move,
                },
            ],
        },
        MirInstr::KeepAlive { var: 1 },
        MirInstr::MovePlace {
            dest: Reg(6),
            place: place(1),
        },
        MirInstr::DefVar {
            var: 2,
            src: Reg(6),
            binding_ty: Some(Ty::Int),
        },
        MirInstr::DefVar {
            var: 2,
            src: Reg(6),
            binding_ty: None,
        },
        MirInstr::UnOp {
            op: PrefixOp::Neg,
            dest: Reg(7),
            a: Reg(6),
        },
        MirInstr::UnOp {
            op: PrefixOp::Not,
            dest: Reg(8),
            a: Reg(7),
        },
        MirInstr::BinOp {
            op: InfixOp::FloorDiv,
            dest: Reg(9),
            a: Reg(7),
            b: Reg(8),
            resolved: Some("Tuple::__contains__".into()),
        },
        MirInstr::Call {
            dest: Reg(10),
            func: FuncRef("std::print".into()),
            raises: Some(Ty::Error),
            args: vec![Reg(1), Reg(2)],
            kwargs: vec![("sep".into(), Reg(3))],
            arg_places: vec![None, Some(place(1))],
            kwarg_places: vec![Some(place(2))],
            capture_accesses: vec![MirCaptureAccess {
                root: 0,
                path: Vec::new(),
                access: CaptureAccess::Read,
            }],
            param_arg_regs: vec![MirParamArg {
                name: Some("T".into()),
                value: Some(Reg(4)),
            }],
        },
        MirInstr::CallIndirect {
            dest: Reg(11),
            callee: Reg(5),
            resolved: Some("Adder::__call__".into()),
            raises: None,
            args: vec![Reg(1)],
            kwargs: Vec::new(),
            callee_place: Some(place(3)),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: vec![ParamDecl::Value {
                name: "n".into(),
                ty: Box::new(Ty::Int),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            instantiated_contract: Some(Ty::Func {
                environment: CallableEnvironment::Default,
                params: vec![Ty::Int],
                names: vec!["x".into()],
                ret: Box::new(Ty::Int),
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
                transfers: TransferSet(Vec::new()),
            }),
            instantiated_args: vec![TyArg::Ty(Ty::Int)],
        },
        MirInstr::MethodCall {
            dest: Reg(12),
            recv: Reg(0),
            method: "append".into(),
            resolved: Some("List::append".into()),
            raises: None,
            reference_result: Some(RefTy {
                referent: Box::new(Ty::Int),
                origin: Origin::Static,
                mutability: Mutability::Param(OriginParamId(0)),
            }),
            result_adapter: Some(CheckedResultAdapter::CopyIteratorReference),
            args: vec![Reg(4)],
            kwargs: vec![("count".into(), Reg(5))],
            recv_place: Some(place(0)),
            recv_writes: true,
            arg_places: vec![Some(place(1))],
            kwarg_places: vec![None],
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
        },
        MirInstr::PointerStorageTake {
            dest: Reg(13),
            pointer: Reg(1),
            index: Reg(2),
            element: Ty::Int,
        },
        MirInstr::PointerStorageDestroy {
            dest: Reg(14),
            pointer: Reg(1),
            index: Reg(2),
            element: Ty::Int,
        },
        MirInstr::UninitStorage {
            dest: Reg(15),
            init: Some(Reg(3)),
        },
        MirInstr::UninitStorage {
            dest: Reg(16),
            init: None,
        },
        MirInstr::UninitStorageTake {
            dest: Reg(17),
            storage: Reg(15),
            element: Ty::Int,
        },
        MirInstr::UninitStorageDestroy {
            dest: Reg(18),
            storage: Reg(15),
            element: Ty::Int,
        },
        MirInstr::GetField {
            dest: Reg(19),
            base: Reg(0),
            field: "size".into(),
        },
        MirInstr::Index {
            dest: Reg(20),
            base: Reg(0),
            index: Reg(1),
            base_place: Some(place(0)),
            index_place: None,
            call: Some(sample_subscript_call()),
            intrinsic: None,
        },
        MirInstr::Index {
            dest: Reg(21),
            base: Reg(0),
            index: Reg(1),
            base_place: None,
            index_place: None,
            call: None,
            intrinsic: Some(MirIntrinsicSubscript::Pointer),
        },
        MirInstr::Slice {
            dest: Reg(22),
            object: Reg(0),
            kind: SliceKind::StridedSlice,
            lower: Some(Reg(1)),
            upper: None,
            step: Some(Reg(2)),
            object_place: Some(place(0)),
            arg_places: vec![None, Some(place(1))],
            call: Some(sample_subscript_call()),
            intrinsic: Some(MirIntrinsicSubscript::Simd),
        },
        MirInstr::MultiIndex {
            dest: Reg(23),
            object: Reg(0),
            args: vec![
                MirSubscriptArg::Index(Reg(1)),
                MirSubscriptArg::Slice {
                    kind: SliceKind::ContiguousSlice,
                    lower: Some(Reg(2)),
                    upper: Some(Reg(3)),
                    step: None,
                },
            ],
            object_place: Some(place(0)),
            arg_places: vec![None],
            kwargs: vec![("byte".into(), MirSubscriptArg::Index(Reg(4)))],
            kwarg_places: vec![Some(place(1))],
            call: Some(sample_subscript_call()),
        },
        MirInstr::MultiSet {
            receiver: Reg(0),
            receiver_place: Some(place(0)),
            args: vec![MirSubscriptArg::Index(Reg(1))],
            arg_places: vec![None],
            value: Reg(2),
            value_place: Some(place(2)),
            value_keyword: true,
            call: sample_subscript_call(),
        },
        MirInstr::Store {
            place: place(0),
            src: Reg(1),
        },
        MirInstr::StoreRef {
            place: place(0),
            reference: Reg(2),
        },
        MirInstr::LoadPlace {
            dest: Reg(24),
            place: place(0),
        },
        MirInstr::MakeTuple {
            dest: Reg(25),
            elems: vec![Reg(1), Reg(2)],
            element_types: Some(vec![Ty::Int, Ty::Bool]),
        },
        MirInstr::MakeTuple {
            dest: Reg(26),
            elems: Vec::new(),
            element_types: None,
        },
        MirInstr::MakeVariant {
            dest: Reg(27),
            alternatives: vec![Ty::Int, Ty::None],
            index: 1,
            value: Reg(1),
        },
        MirInstr::VariantIs {
            dest: Reg(28),
            variant: Reg(27),
            index: 0,
        },
        MirInstr::VariantGet {
            dest: Reg(29),
            variant: Reg(27),
            index: 1,
        },
        MirInstr::VariantSet {
            dest: Reg(30),
            place: place(0),
            index: 0,
            value: Reg(1),
        },
        MirInstr::VariantTake {
            dest: Reg(31),
            variant: Reg(27),
            index: 1,
            checked: true,
        },
        MirInstr::VariantSetInitWith {
            dest: Reg(32),
            place: place(0),
            index: 0,
            factory: Reg(5),
        },
        MirInstr::VariantDeinitWith {
            dest: Reg(33),
            variant: Reg(27),
            handler: Reg(5),
            index: 1,
        },
        MirInstr::VariantReplace {
            dest: Reg(34),
            place: place(0),
            input_index: 0,
            output_index: 1,
            value: Reg(1),
            checked: false,
        },
        MirInstr::MakeSimd {
            dest: Reg(35),
            dtype: Dtype::Float32,
            width: 4,
            elems: vec![Reg(1), Reg(2), Reg(3), Reg(4)],
        },
        MirInstr::SimdCast {
            dest: Reg(36),
            value: Reg(35),
            dtype: Dtype::Int32,
            width: 4,
        },
        MirInstr::SimdShuffle {
            dest: Reg(37),
            value: Reg(35),
            mask: vec![3, 1, 2, 0],
        },
        MirInstr::Raise { src: Reg(1) },
        MirInstr::Drop { reg: Reg(2) },
        MirInstr::DropVar { var: 1 },
        MirInstr::ConsumeVar { var: 2 },
        MirInstr::ConsumePlace {
            place: place(0),
            marker: Reg(3),
        },
        MirInstr::Unsupported("no lowering for frobnication".into()),
        MirInstr::GetIter {
            source: 0,
            dest: 1,
            mode: IterationMode::Borrowed,
            prepare: vec!["__iter__".into()],
        },
        MirInstr::GetIter {
            source: 0,
            dest: 2,
            mode: IterationMode::Owned,
            prepare: Vec::new(),
        },
        MirInstr::HasNext {
            dest: Reg(38),
            iter: 1,
            method: Some("__has_next__".into()),
        },
        MirInstr::Next {
            dest: Reg(39),
            iter: 1,
            call: Some(sample_iterator_call()),
        },
        MirInstr::Next {
            dest: Reg(40),
            iter: 1,
            call: None,
        },
        MirInstr::TryNext {
            dest: Reg(41),
            yielded: Reg(42),
            iter: 1,
            call: sample_iterator_call(),
            exhaustion: Ty::Struct("StopIteration".into(), Vec::new()),
        },
    ];
    let mut function = function_with(vec![Ty::Int; 43], instrs);
    function.blocks.push(MirBlock {
        instrs: Vec::new(),
        term: MirTerm::ReturnWithCleanup {
            value: Some(Reg(0)),
            cleanup: vec![1, 2],
        },
    });
    function.blocks.push(MirBlock {
        instrs: Vec::new(),
        term: MirTerm::Branch {
            cond: Reg(0),
            then_b: 0,
            else_b: 1,
        },
    });
    function.blocks.push(MirBlock {
        instrs: Vec::new(),
        term: MirTerm::Jump(0),
    });
    function.blocks.push(MirBlock {
        instrs: Vec::new(),
        term: MirTerm::Return(None),
    });
    let program = program_with(vec![("instructions".into(), function)]);
    assert_reprints(&program);
}

#[test]
fn nested_try_regions_reprint_without_region_source_marks() {
    let region_block = |term: MirTerm| MirBlock {
        instrs: vec![MirInstr::KeepAlive { var: 0 }],
        term,
    };
    let inner_try = MirInstr::Try {
        body: vec![region_block(MirTerm::EscapeJump {
            target: 0,
            cleanup: vec![1],
        })],
        handler: None,
        orelse: None,
        finalbody: None,
        cleanup: Vec::new(),
    };
    let outer_try = MirInstr::Try {
        body: vec![
            MirBlock {
                instrs: vec![inner_try],
                term: MirTerm::Jump(1),
            },
            region_block(MirTerm::FallOff),
        ],
        handler: Some((Some(3), vec![region_block(MirTerm::FallOff)])),
        orelse: Some(vec![region_block(MirTerm::FallOff)]),
        finalbody: Some(vec![region_block(MirTerm::FallOff)]),
        cleanup: vec![4, 5],
    };
    let program = program_with(vec![(
        "regions".into(),
        function_with(Vec::new(), vec![outer_try]),
    )]);
    let text = write::program(&program);
    let parsed =
        artifact(text.as_bytes(), "unit.mir".to_string()).expect("parse canonical artifact");
    assert_eq!(write::program(&parsed.program), text);
    // Region-local blocks stay out of the source map: the enclosing
    // instruction path brackets them and the canonical verifier resolves
    // only function-level block paths.
    let paths: Vec<&str> = parsed.source_map.iter().map(|(path, _)| path).collect();
    assert_eq!(
        paths,
        [
            "artifact",
            "function/regions",
            "function/regions/bb0",
            "function/regions/bb0/instruction/0",
            "function/regions/bb0/terminator",
        ]
    );
}

fn artifact_with_instruction(instruction_text: &str) -> String {
    format!(
        "mojito-mir 1.0\nartifact {{\n  features: [],\n  files: [],\n  structs: [],\n  \
         decls: [],\n  functions: [\n    fn {{\n      name: main,\n      registers: 1,\n      \
         vars: 0,\n      var_names: [],\n      params: 0,\n      param_types: [],\n      \
         owned_params: [],\n      deinit_params: [],\n      ref_params: [],\n      \
         returns_reference: false,\n      var_types: [],\n      return_type: present(None),\n      \
         raises: false,\n      error_type: absent,\n      register_types: [],\n      \
         locations: [],\n      blocks: [bb0 {{ instructions: [{instruction_text}], \
         terminator: falloff {{}} }}]\n    }}\n  ]\n}}\n"
    )
}

#[test]
fn malformed_instruction_payloads_are_diagnosed() {
    assert!(
        diagnostics(&artifact_with_instruction("ref.make { dest: %r0 }"))
            .iter()
            .any(|message| message.contains("missing required field `place`"))
    );
    assert!(
        diagnostics(&artifact_with_instruction("frob.nicate { dest: %r0 }"))
            .iter()
            .any(|message| message.contains("unknown instruction `frob.nicate`"))
    );
    assert!(
        diagnostics(&artifact_with_instruction(
            "try { body: [bb1 { instructions: [], terminator: falloff {} }], \
             handler: absent, orelse: absent, finalbody: absent, cleanup: [] }"
        ))
        .iter()
        .any(|message| message.contains("expected `bb0` record, found `bb1`"))
    );
    assert!(
        diagnostics(&artifact_with_instruction(
            "index.multi { dest: %r0, object: %r0, args: [slice_arg { kind: sideways, \
             lower: absent, upper: absent, step: absent }], object_place: absent, \
             arg_places: [], kwargs: [], kwarg_places: [], call: absent }"
        ))
        .iter()
        .any(|message| message.contains("unknown slice kind `sideways`"))
    );
}

#[test]
fn declaration_metadata_reprints_byte_identically() {
    let boxed = MirStructDeclaration {
        name: "Box".into(),
        fields: vec![("value".into(), Ty::Int), ("tag".into(), Ty::Bool)],
        mut_self_methods: HashSet::from(["append".into(), "clear".into()]),
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
        explicit_destroy_message: Some("explicit destroy required".into()),
        explicit_destructors: HashMap::from([("_finish".into(), true), ("__del__".into(), false)]),
    };
    let zebra = MirStructDeclaration {
        name: "Zebra needs quoting!".into(),
        fields: Vec::new(),
        mut_self_methods: HashSet::new(),
        fieldwise_init: false,
        param_decls: Vec::new(),
        explicit_destroy_message: None,
        explicit_destructors: HashMap::new(),
    };
    let add = MirFunctionDeclaration {
        lowered_name: "add".into(),
        param_names: vec!["lhs".into(), "rhs".into(), "scale".into()],
        param_types: vec![Ty::Int, Ty::Int, Ty::Float64],
        defaults: vec![
            None,
            Some(CheckedConst::Int(parse_int_literal("-7").unwrap())),
            Some(CheckedConst::Float(
                FloatLiteral::parse_exact("157/50").unwrap(),
            )),
        ],
        required: vec![true, false, false],
        variadic: Some(Ty::Int),
        variadic_convention: Some(ArgConvention::Var),
        variadic_index: Some(3),
        kw_variadic: Some(Ty::Bool),
        kw_variadic_convention: Some(ArgConvention::Imm),
        kw_variadic_index: Some(4),
        positional_only: Some(1),
        keyword_only: Some(2),
        param_decls: vec![ParamDecl::Value {
            name: "n".into(),
            ty: Box::new(Ty::Int),
            default: Some(CtExpr::Value(CtValue::Int(2))),
            callable_default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }],
        has_receiver: true,
        receiver_convention: Some(ArgConvention::Mut),
        param_conventions: vec![None, Some(ArgConvention::Imm), Some(ArgConvention::Ref)],
        ret_ty: Ty::Int,
        returns_reference: true,
        raises: true,
        error_ty: Some(Ty::Error),
        ref_params: vec![false, false, true],
    };
    let other = MirFunctionDeclaration {
        lowered_name: "aaa_first".into(),
        param_names: vec!["flag".into(), "nothing".into()],
        param_types: vec![Ty::Bool, Ty::None],
        defaults: vec![Some(CheckedConst::Bool(true)), Some(CheckedConst::None)],
        required: vec![false, false],
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
        param_conventions: vec![None, None],
        ret_ty: Ty::None,
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: vec![false, false],
    };
    let mut program = program_with(vec![("main".into(), function_with(Vec::new(), Vec::new()))]);
    // Deliberately unsorted: the canonical writer sorts by name, so the
    // reprint equality also proves parse keeps the sorted order.
    program.declarations = MirDeclarations {
        structs: vec![zebra, boxed],
        functions: vec![add, other],
    };
    assert_reprints(&program);
}

#[test]
fn malformed_declaration_metadata_is_diagnosed() {
    let text = "mojito-mir 1.0\nartifact { features: [], files: [], structs: [struct { \
                name: Box, fields: [], mut_self_methods: [], fieldwise_init: false, \
                param_decls: [], explicit_destroy_message: absent, explicit_destructors: \
                [destructor { name: f, raises: true }, destructor { name: f, raises: \
                false }] }], decls: [decl { lowered_name: add }], functions: [] }\n";
    let messages = diagnostics(text);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("duplicate destructor `f`"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing required field `param_names`"))
    );
}

#[test]
fn func_types_reject_param_decls() {
    let text = artifact_with_register_type(
        "func { environment: default, param_decls: [type_param { name: T, bounds: [], \
         callable_bound: absent, default: absent, infer_only: false, variadic: false, \
         constraints: [] }], params: [], names: [], return_type: None, required: [], \
         variadic: absent, kw_variadic: absent, positional_only: absent, keyword_only: \
         absent, raises: false, error_type: absent, conventions: [], ref_params: [], \
         ref_return: absent, transfers: [] }",
    );
    assert!(
        diagnostics(&text)
            .iter()
            .any(|message| message.contains("`func` types take no param_decls"))
    );
}
