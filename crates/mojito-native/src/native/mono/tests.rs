#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

fn host_target() -> Option<NativeTarget> {
    NativeTarget::host()
}
use mojito_types::types::{ConstraintOperand, GenericConstraint};

fn specialized_main(source: &str) -> SpecializedProgram {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile iterator program");
    specialize(
        compiled.drop_elaborated_mir(),
        &["main".to_string()],
        host_target().as_ref(),
    )
    .expect("specialize iterator program")
}

fn instructions(blocks: &[MirBlock]) -> Vec<&MirInstr> {
    let mut result = Vec::new();
    for block in blocks {
        for instruction in &block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                result.extend(instructions(body));
                if let Some((_, blocks)) = handler {
                    result.extend(instructions(blocks));
                }
                if let Some(blocks) = orelse {
                    result.extend(instructions(blocks));
                }
                if let Some(blocks) = finalbody {
                    result.extend(instructions(blocks));
                }
            } else {
                result.push(instruction);
            }
        }
    }
    result
}

fn test_binder(name: &str) -> ParamRef {
    ParamRef {
        id: mojito_types::param_expr::ParamId::new(&format!("$test:{name}"), 0),
        name: name.into(),
    }
}

fn function<'a>(program: &'a SpecializedProgram, name: &str) -> &'a MirFunction {
    &program
        .program
        .functions
        .iter()
        .find(|(known, _)| known == name)
        .unwrap_or_else(|| panic!("specialized program lacks `{name}`"))
        .1
}

#[test]
fn raising_user_iterator_types_the_split_slot_and_retargets_its_operations() {
    let source = "@fieldwise_init\n\
                  struct RangeIter:\n\
                  \x20   var cur: Int\n\
                  \x20   var stop: Int\n\
                  \n\
                  \x20   def __next__(mut self) raises StopIteration -> Int:\n\
                  \x20       if self.cur >= self.stop:\n\
                  \x20           raise StopIteration()\n\
                  \x20       var v: Int = self.cur\n\
                  \x20       self.cur = self.cur + 1\n\
                  \x20       return v\n\
                  \n\
                  @fieldwise_init\n\
                  struct Countdown:\n\
                  \x20   var n: Int\n\
                  \n\
                  \x20   def __iter__(self) -> RangeIter:\n\
                  \x20       return RangeIter(0, self.n)\n\
                  \n\
                  def main():\n\
                  \x20   var total: Int = 0\n\
                  \x20   for x in Countdown(5):\n\
                  \x20       total = total + x\n\
                  \x20   print(total)\n";
    let specialized = specialized_main(source);
    let main = function(&specialized, "main");
    let instrs = instructions(&main.blocks);
    let (dest, prepare) = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::GetIter { dest, prepare, .. } => Some((*dest, prepare)),
            _ => None,
        })
        .expect("main normalizes its iterable");
    assert!(
        matches!(main.var_tys.get(&dest).map(Ty::kind), Some(TyKind::Struct(name, _)) if name == "RangeIter"),
        "the split iterator slot must be typed by the prepare chain: {:?}",
        main.var_tys.get(&dest)
    );
    for step in prepare {
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == step),
            "prepare step `{step}` must name a specialized function"
        );
    }
    let target = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::TryNext { call, .. } => Some(&call.target),
            _ => None,
        })
        .expect("user iteration advances through a raising `__next__`");
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| name == target),
        "`{target}` must name a specialized function"
    );
}

#[test]
fn raising_range_iteration_types_the_slot_and_reaches_its_operations() {
    let specialized = specialized_main("def main():\n    for x in range(3):\n        print(x)\n");
    let main = function(&specialized, "main");
    let instrs = instructions(&main.blocks);
    let dest = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::GetIter { dest, .. } => Some(*dest),
            _ => None,
        })
        .expect("range iteration normalizes its iterable");
    assert!(
        matches!(
            main.var_tys.get(&dest).map(Ty::kind),
            Some(TyKind::Struct(..))
        ),
        "the range iterator slot must be struct-typed: {:?}",
        main.var_tys.get(&dest)
    );
    let call = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::TryNext { call, .. } => Some(call),
            _ => None,
        })
        .expect("range iteration advances through a raising `__next__`");
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| name == &call.target),
        "`{}` must name a specialized function",
        call.target
    );
}

#[test]
fn generic_dispatch_iteration_unrolls_to_a_typed_concrete_chain() {
    let source = include_str!(
        "../../../../../assets/extensions/ok/generic_borrowed_dispatch_overloaded_iter.mojo"
    );
    let specialized = specialized_main(source);
    let first_count = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("first_count"))
        .expect("the generic loop body was specialized");
    let instrs = instructions(&first_count.1.blocks);
    let (dest, prepare) = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::GetIter { dest, prepare, .. } => Some((*dest, prepare)),
            _ => None,
        })
        .expect("the generic loop normalizes its iterable");
    assert!(
        !prepare
            .iter()
            .any(|step| step.starts_with("__trait_dispatch.")),
        "dispatch steps must resolve statically post-mono: {prepare:?}"
    );
    assert!(
        matches!(
            first_count.1.var_tys.get(&dest).map(Ty::kind),
            Some(TyKind::Struct(name, _)) if name.starts_with("CountIter")
        ),
        "the dispatched iterator slot must be concretely typed: {:?}",
        first_count.1.var_tys.get(&dest)
    );
}

#[test]
fn parameterized_associated_type_resolves_from_the_conformer_member() {
    let source = "def count[C: Iterable](items: C) -> Int:\n\
                  \x20   var it = items.__iter__()\n    return 1\n\n\
                  def main():\n    var l: List[Int] = [3, 4, 5]\n    print(count(l))\n";
    let specialized = specialized_main(source);
    let count = &specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("count"))
        .expect("the generic body was specialized")
        .1;
    let it = count
        .var_names
        .iter()
        .position(|slot| slot == "it")
        .expect("`it` slot") as u32;
    assert!(
        matches!(
            count.var_tys.get(&it).map(Ty::kind),
            Some(TyKind::Struct(name, _)) if name.contains("_ListIter")
        ),
        "`C.IteratorType[o]` must resolve to the List iterator instance: {:?}",
        count.var_tys.get(&it)
    );
}

#[test]
fn structural_inference_rejects_conflicting_solutions() {
    let parameter = Ty::new(TyKind::Param {
        binder: mojito_types::param_expr::ParamRef {
            id: mojito_types::param_expr::ParamId::new("$test:T", 0),
            name: "T".into(),
        },
        bounds: vec![],
        callable_bound: None,
    });
    let mut bindings = Bindings::default();
    unify(&parameter, &Ty::int(), &mut bindings).unwrap();
    assert!(
        unify(&parameter, &Ty::bool(), &mut bindings)
            .unwrap_err()
            .contains("conflicting")
    );
}

#[test]
fn dependent_lambda_calls_specialize_once_per_index_and_element_type() {
    let source = include_str!("../../../../../assets/ok/lambda_generic_comptime.mojo");
    let specialized = specialized_main(source);
    let lambda_instances = specialized
        .program
        .functions
        .iter()
        .filter(|(name, _)| name.contains("$$lambda$") && name.contains("$mono$"))
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(
        lambda_instances.len() >= 2,
        "explicit and callable-bound lambdas need specialized lifted bodies: {lambda_instances:?}"
    );
    assert!(specialized.program.functions.iter().all(|(_, function)| {
        !instructions(&function.blocks)
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::CallIndirect { .. }))
    }));
}

#[test]
fn same_spelled_binders_of_two_declarations_bind_apart() {
    let binder = |owner: &str| mojito_types::param_expr::ParamRef {
        id: mojito_types::param_expr::ParamId::new(owner, 0),
        name: "T".into(),
    };
    let parameter = |owner: &str| {
        Ty::new(TyKind::Param {
            binder: binder(owner),
            bounds: vec![],
            callable_bound: None,
        })
    };
    let mut bindings = Bindings::default();
    unify(&parameter("outer"), &Ty::int(), &mut bindings).unwrap();
    unify(&parameter("contract"), &Ty::bool(), &mut bindings).unwrap();
    assert_eq!(
        substitute_ty(&parameter("outer"), &bindings).unwrap(),
        TyKind::Int
    );
    assert_eq!(
        substitute_ty(&parameter("contract"), &bindings).unwrap(),
        TyKind::Bool
    );
    assert!(substitute_ty(&parameter("unbound"), &bindings).is_err());
}

#[test]
fn literal_actuals_merge_with_concrete_bindings_in_either_order() {
    let mut bindings = Bindings::default();
    // Receiver-first: `T := Int` from the concrete receiver, then a
    // literal-typed actual (`41 : IntLiteral`) — compatible, keeps `Int`.
    bind_type(&test_binder("T"), &Ty::int(), &mut bindings).unwrap();
    bind_type(&test_binder("T"), &Ty::int_literal(), &mut bindings).unwrap();
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&Ty::int()));

    // Result-last: the literal actual binds first, the concrete result
    // type upgrades it.
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::int_literal(), &mut bindings).unwrap();
    bind_type(&test_binder("T"), &Ty::int(), &mut bindings).unwrap();
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&Ty::int()));

    // Genuinely distinct concrete solutions still conflict, and the
    // message carries the structural forms (`Display` collapses
    // `IntLiteral` to `Int`).
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::int(), &mut bindings).unwrap();
    let error = bind_type(&test_binder("T"), &Ty::float64(), &mut bindings).unwrap_err();
    assert!(error.contains("conflicting"), "{error}");
    // Two different literal kinds conflict too.
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::int_literal(), &mut bindings).unwrap();
    assert!(bind_type(&test_binder("T"), &Ty::float_literal(), &mut bindings).is_err());
}

#[test]
fn value_constructor_literal_arguments_bind_against_the_receiver_solution() {
    // The owned_pointer_api shape: the receiver's type arguments solve
    // `T := Int`, then the literal-typed constructor argument must merge
    // rather than conflict ("`Int` and `Int`").
    let source = "struct Box[T: Movable & Deinitable]:\n\
                  \x20   var value: Self.T\n\
                  \n\
                  \x20   def __init__(out self, var value: Self.T):\n\
                  \x20       self.value = value^\n\
                  \n\
                  def main():\n\
                  \x20   var b = Box[Int](41)\n\
                  \x20   print(b.value)\n";
    let specialized = specialized_main(source);
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| name == "Box$mono$TInt.__init__"),
        "the constructor instance must materialize under the owner instance: {:?}",
        specialized
            .program
            .functions
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn nominal_len_rewrites_to_a_resolved_dunder_method_call() {
    let source = "@fieldwise_init\n\
                  struct Sized:\n\
                  \x20   var n: Int\n\
                  \n\
                  \x20   def __len__(self) -> Int:\n\
                  \x20       return self.n\n\
                  \n\
                  def main():\n\
                  \x20   print(len(Sized(3)))\n";
    let specialized = specialized_main(source);
    let main = function(&specialized, "main");
    let instrs = instructions(&main.blocks);
    let resolved = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                resolved: Some(resolved),
                ..
            } if method == "__len__" => Some(resolved.clone()),
            _ => None,
        })
        .expect("`len(nominal)` must rewrite to a resolved `__len__` call");
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| *name == resolved),
        "the rewritten target `{resolved}` must be a specialized function"
    );
    assert!(
        !instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. } if func.0 == "len"
        )),
        "no bare `len` builtin call may survive the rewrite"
    );
}

#[test]
fn colliding_instances_share_only_modulo_pointer_elements() {
    let pointer = |element: Ty| {
        Ty::new(TyKind::Pointer {
            element,
            origin: mojito_types::origin::PointerOrigin::Static,
        })
    };
    // The `_RawAlloc`/`List` shape: fields differing only behind a
    // pointer are one opaque word and drop inertly — benign to share.
    assert!(fields_equivalent(
        &[("ptr".into(), pointer(Ty::int()))],
        &[("ptr".into(), pointer(Ty::float64()))],
    ));
    // A payload-carrying difference (the `__UninitStorage` shape) is a
    // genuine layout/lifecycle hazard.
    assert!(!fields_equivalent(
        &[(
            "_storage".into(),
            TyKind::Struct("__UninitStorage".into(), vec![TyArg::Ty(Ty::int())]).into(),
        )],
        &[(
            "_storage".into(),
            TyKind::Struct(
                "__UninitStorage".into(),
                vec![TyArg::Ty(
                    TyKind::Struct("Recorder".into(), Vec::new()).into()
                )],
            )
            .into(),
        )],
    ));
    // Field names and non-pointer types stay strict.
    assert!(!fields_equivalent(
        &[("a".into(), Ty::int())],
        &[("b".into(), Ty::int())],
    ));
    assert!(!fields_equivalent(
        &[("a".into(), Ty::int())],
        &[("a".into(), Ty::float64())],
    ));
}

#[test]
fn substitution_resolves_nested_type_and_value_arguments() {
    let mut bindings = Bindings {
        structs: Rc::new(StructCatalog::generic(&["Buffer"])),
        ..Bindings::default()
    };
    bindings.types.insert(test_binder("T"), Ty::uint());
    bindings.values.insert(
        mojito_types::param_expr::ParamRef {
            id: mojito_types::param_expr::ParamId::new("Holder", 1),
            name: "n".into(),
        },
        CtValue::Int(4),
    );
    let ty = Ty::new(TyKind::Struct(
        "Buffer".into(),
        vec![
            TyArg::Ty(
                TyKind::Param {
                    binder: mojito_types::param_expr::ParamRef {
                        id: mojito_types::param_expr::ParamId::new("$test:T", 0),
                        name: "T".into(),
                    },
                    bounds: vec![],
                    callable_bound: None,
                }
                .into(),
            ),
            TyArg::Val(CtValue::Expr(ParamContext::detached().decl_ref(
                mojito_types::param_expr::ParamId::new("Holder", 1),
                "n",
                mojito_types::param_expr::MetaTy::int(),
            ))),
        ],
    ));
    let substituted = substitute_ty(&ty, &bindings).unwrap();
    let TyKind::Struct(name, args) = substituted.kind() else {
        panic!()
    };
    assert!(name.contains("$mono$"));
    assert_eq!(
        **args,
        vec![TyArg::Ty(Ty::uint()), TyArg::Val(CtValue::Int(4))]
    );
}

#[test]
fn value_binders_sharing_a_spelling_keep_their_own_solutions() {
    let mut bindings = Bindings {
        structs: Rc::new(StructCatalog::generic(&["Grid"])),
        ..Bindings::default()
    };
    let binder = |owner: &str| mojito_types::param_expr::ParamRef {
        id: mojito_types::param_expr::ParamId::new(owner, 0),
        name: "n".into(),
    };
    bindings.values.insert(binder("Grid"), CtValue::Int(4));
    bindings
        .values
        .insert(binder("Grid.resize"), CtValue::Int(9));
    let reference = |owner: &str| {
        TyArg::Val(CtValue::Expr(ParamContext::detached().decl_ref(
            mojito_types::param_expr::ParamId::new(owner, 0),
            "n",
            mojito_types::param_expr::MetaTy::int(),
        )))
    };
    let ty = Ty::new(TyKind::Struct(
        "Grid".into(),
        vec![reference("Grid.resize"), reference("Grid")],
    ));
    let substituted = substitute_ty(&ty, &bindings).unwrap();
    let TyKind::Struct(_, args) = substituted.kind() else {
        panic!()
    };
    assert_eq!(
        **args,
        vec![TyArg::Val(CtValue::Int(9)), TyArg::Val(CtValue::Int(4))]
    );
}

#[test]
fn distinct_instantiations_split_into_owner_named_instances() {
    // The `List.grow` shape: `refresh` reaches `set` through the bare
    // in-body `self` receiver, which must carry the owner instance's
    // binding for `T` rather than the shared template spelling.
    let source = "struct Pairing[T: Copyable & Movable & Deinitable]:\n\
                  \x20   var value: Self.T\n\
                  \n\
                  \x20   def __init__(out self, var value: Self.T):\n\
                  \x20       self.value = value^\n\
                  \n\
                  \x20   def get(self) -> Self.T:\n\
                  \x20       return self.value.copy()\n\
                  \n\
                  \x20   def refresh(mut self, var value: Self.T):\n\
                  \x20       self.set(value^)\n\
                  \n\
                  \x20   def set(mut self, var value: Self.T):\n\
                  \x20       self.value = value^\n\
                  \n\
                  def main():\n\
                  \x20   var a = Pairing[Int](1)\n\
                  \x20   var b = Pairing[Bool](True)\n\
                  \x20   a.refresh(3)\n\
                  \x20   b.refresh(False)\n\
                  \x20   print(a.get())\n\
                  \x20   print(b.get())\n";
    let specialized = specialized_main(source);
    // The calls reach the template's methods, each instantiated under its
    // owner, as the constructor is.
    for expected in [
        "Pairing$mono$TInt.refresh",
        "Pairing$mono$TBool.refresh",
        "Pairing$mono$TInt.set",
        "Pairing$mono$TBool.set",
        "Pairing$mono$TInt.__init__",
        "Pairing$mono$TBool.__init__",
    ] {
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == expected),
            "missing instance `{expected}`: {:?}",
            specialized
                .program
                .functions
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
        );
    }
    let field_ty = |instance: &str| {
        specialized
            .program
            .declarations
            .structs
            .iter()
            .find(|decl| decl.name == instance)
            .unwrap_or_else(|| panic!("missing struct instance `{instance}`"))
            .fields[0]
            .1
            .clone()
    };
    assert_eq!(field_ty("Pairing$mono$TInt"), Ty::int());
    assert_eq!(field_ty("Pairing$mono$TBool"), Ty::bool());
    assert!(
        !specialized
            .program
            .declarations
            .structs
            .iter()
            .any(|decl| decl.name == "Pairing"),
        "the shared template declaration must not survive canonicalization"
    );
}

#[test]
fn binding_solutions_ignore_reference_origins() {
    let referent = Ty::int();
    let first = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
        referent: referent.clone(),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Immutable,
    }));
    let second = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
        referent,
        origin: mojito_types::origin::Origin::Untracked { mutable: false },
        mutability: mojito_types::origin::Mutability::Immutable,
    }));
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &first, &mut bindings).unwrap();
    bind_type(&test_binder("T"), &second, &mut bindings).unwrap();
    // First solution wins; a mutability disagreement still conflicts.
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&first));
    let mutable = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
        referent: Ty::int(),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Mutable,
    }));
    assert!(bind_type(&test_binder("T"), &mutable, &mut bindings).is_err());
}

#[test]
fn variadic_arity_joins_the_instance_identity_and_reifies_the_pack() {
    let source = "def total(*values: Int) -> Int:\n\
                  \x20   var acc: Int = 0\n\
                  \x20   for value in values:\n\
                  \x20       acc = acc + value\n\
                  \x20   return acc\n\
                  \n\
                  def main():\n\
                  \x20   print(total(), total(7), total(1, 2, 3))\n";
    let specialized = specialized_main(source);
    let arities: Vec<&str> = specialized
        .program
        .functions
        .iter()
        .filter(|(name, _)| name.starts_with("total$mono$"))
        .map(|(name, _)| name.as_str())
        .collect();
    for expected in ["total$mono$V0", "total$mono$V1", "total$mono$V3"] {
        assert!(
            arities.contains(&expected),
            "each call-site arity gets its own instance: {arities:?}"
        );
    }
    let one = function(&specialized, "total$mono$V1");
    assert!(
        one.var_tys
            .values()
            .any(|ty| matches!(ty.kind(), TyKind::Tuple(elements) if elements == &[TyKind::Int])),
        "the pack parameter reifies to a one-element tuple: {:?}",
        one.var_tys
    );
}

#[test]
fn subscript_value_parameters_join_the_accessor_instance_identity() {
    let source = "def main():\n\
                  \x20   var pair: Tuple[Int, Int] = (10, 32)\n\
                  \x20   print(pair[0] + pair[1])\n";
    let specialized = specialized_main(source);
    let main = function(&specialized, "main");
    let targets: std::collections::HashSet<&str> = instructions(&main.blocks)
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Index {
                call: Some(call), ..
            } => Some(call.target.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        targets.len() >= 2,
        "distinct constant indexes must dispatch distinct accessor \
         instances: {targets:?}"
    );
}

#[test]
fn specialized_programs_verify_over_erased_contracts() {
    // A capturing closure stored in an instance whose element contract
    // erased its environment, and an indirect call through a callable
    // contract over the owner's binder.
    let sources = [
        "def main():\n\
         \x20   var k = 3\n\
         \x20   var scaled = [lambda (x: Int) {k} -> Int: x * k]\n\
         \x20   print(scaled[0](2))\n",
        "def main():\n\
         \x20   var values: Array[Int, 2] = [1, 2]\n\
         \x20   values^.deinit_with(lambda (var element: Int): print(element))\n",
    ];
    for source in sources {
        let specialized = specialized_main(source);
        assert_eq!(
            mojito_mir::mir::verify::verify(&specialized.program),
            Vec::<String>::new()
        );
    }
}

#[test]
fn concrete_verification_rejects_a_symbolic_instantiated_argument() {
    let source = "def same[T: Copyable](value: T) -> T:\n\
         \x20   return value.copy()\n\
         def forward[T: Copyable](value: T) -> T:\n\
         \x20   return same[T](value)\n\
         def main():\n\
         \x20   print(forward(3))\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile generic program");
    let parametric = compiled.drop_elaborated_mir();
    let findings = mojito_mir::mir::verify::verify_concrete(parametric);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("instantiated argument keeps symbolic type `T`")),
        "a parametric call instantiates its callee at its binder: {findings:?}"
    );
    let specialized =
        specialize(parametric, &["main".to_string()], host_target().as_ref()).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
}

#[test]
fn concrete_verification_separates_elaborated_from_parametric_mir() {
    let source = "struct Box[T: Copyable & Deinitable]:\n\
         \x20   var value: Self.T\n\
         \x20   def __init__(out self, var value: Self.T):\n\
         \x20       self.value = value^\n\
         \x20   def get(self) -> Self.T:\n\
         \x20       return self.value.copy()\n\
         def main():\n\
         \x20   print(Box(3).get())\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile generic program");
    let parametric = compiled.drop_elaborated_mir();
    assert_eq!(
        mojito_mir::mir::verify::verify(parametric),
        Vec::<String>::new()
    );
    let findings = mojito_mir::mir::verify::verify_concrete(parametric);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("keeps symbolic type `T`")),
        "a parametric body names its binder: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("keeps compile-time parameter `T`")),
        "a parametric declaration keeps its binder: {findings:?}"
    );
    let specialized =
        specialize(parametric, &["main".to_string()], host_target().as_ref()).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
}

const CONDITIONAL_MEMBERS: &str = "struct NoDefault(Movable):\n\
     \x20   var v: Int\n\
     \n\
     \x20   def __init__(out self, v: Int):\n\
     \x20       self.v = v\n\
     \n\
     struct Slot[T: Movable & Deinitable]:\n\
     \x20   var item: Self.T\n\
     \n\
     \x20   def __init__(out self) where conforms_to(Self.T, Defaultable):\n\
     \x20       self.item = Self.T()\n\
     \n\
     \x20   def __init__(out self, var item: Self.T):\n\
     \x20       self.item = item^\n\
     \n\
     def main():\n\
     \x20   var a = Slot[Int]()\n\
     \x20   print(a.item)\n\
     \x20   var b = Slot[NoDefault](NoDefault(7))\n\
     \x20   print(b.item.v)\n";

/// The nullary `Slot.__init__` overload's symbol.
fn nullary_slot_init() -> String {
    format!("Slot.__init__{}", mojito_symbol::symbol::OV_SEP)
}

/// The bindings of `Slot[argument]` over the compiled `CONDITIONAL_MEMBERS`.
fn slot_bindings(specializer: &Specializer<'_>, argument: Ty) -> Bindings {
    let template = &specializer.structs["Slot"];
    let mut bindings = specializer.base_bindings();
    bind_ty_args(&template.param_decls, &[TyArg::Ty(argument)], &mut bindings).unwrap();
    bindings
}

#[test]
fn discovered_member_its_instance_disproves_is_left_out() {
    let specialized = specialized_main(CONDITIONAL_MEMBERS);
    let nullary = |owner: &str| {
        let name = mojito_symbol::symbol::retarget_method_symbol(&nullary_slot_init(), owner)
            .expect("constructor symbol retargets");
        specialized
            .program
            .functions
            .iter()
            .any(|(known, _)| *known == name)
    };
    let owners: Vec<&str> = specialized
        .program
        .declarations
        .structs
        .iter()
        .map(|declaration| declaration.name.as_str())
        .filter(|name| name.starts_with("Slot$mono"))
        .collect();
    let [with, without] = ["Int", "NoDefault"].map(|argument| {
        *owners
            .iter()
            .find(|owner| owner.contains(argument))
            .unwrap_or_else(|| panic!("no `Slot[{argument}]` in {owners:?}"))
    });
    assert!(nullary(with), "`Slot[Int]` default-constructs");
    assert!(
        !nullary(without),
        "`Slot[NoDefault]` has no nullary constructor"
    );
    assert!(
        specialized
            .program
            .declarations
            .functions
            .iter()
            .all(|declaration| declaration.availability.is_empty()),
        "an instance's clauses are decided"
    );
}

#[test]
fn demanded_member_its_instance_disproves_is_an_error() {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(CONDITIONAL_MEMBERS, std::path::Path::new("mono_test.mojo"))
        .expect("compile conditional members");
    let mut specializer = Specializer::new(compiled.drop_elaborated_mir(), None);
    let available = slot_bindings(&specializer, Ty::int());
    specializer
        .enqueue(&nullary_slot_init(), available, Vec::new())
        .expect("`Int` is `Defaultable`");
    let unavailable = slot_bindings(
        &specializer,
        TyKind::Struct("NoDefault".into(), Vec::new()).into(),
    );
    let error = specializer
        .enqueue(&nullary_slot_init(), unavailable, Vec::new())
        .unwrap_err();
    assert!(error.construct.contains("unavailable"), "{error}");
}

#[test]
fn available_member_that_does_not_materialize_is_an_error() {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(CONDITIONAL_MEMBERS, std::path::Path::new("mono_test.mojo"))
        .expect("compile conditional members");
    let mut specializer = Specializer::new(compiled.drop_elaborated_mir(), None);
    // The empty tuple is `Defaultable`, and `T()` has no construction for it.
    let bindings = slot_bindings(&specializer, TyKind::Tuple(Vec::new()).into());
    specializer
        .enqueue(&nullary_slot_init(), bindings, Vec::new())
        .expect("the clause holds");
    let error = specializer.run(&[]).unwrap_err();
    assert!(
        error.construct.contains("default-constructing `Tuple[]`"),
        "{error}"
    );
}

const TRIVIAL_AND_VALUE_MEMBERS: &str = "from std.traits import IsTriviallyCopyable\n\
     \n\
     @fieldwise_init\n\
     struct Point(Copyable):\n\
     \x20   var x: Int\n\
     \n\
     struct Tracked(Copyable):\n\
     \x20   var x: Int\n\
     \n\
     \x20   def __init__(out self, x: Int):\n\
     \x20       self.x = x\n\
     \n\
     \x20   def __init__(out self, *, copy: Self):\n\
     \x20       self.x = copy.x\n\
     \n\
     struct Cell[T: Copyable & Deinitable, n: Int]:\n\
     \x20   var item: Self.T\n\
     \n\
     \x20   def __init__(out self, var item: Self.T):\n\
     \x20       self.item = item^\n\
     \n\
     \x20   def bits(self) -> Int where IsTriviallyCopyable[Self.T]:\n\
     \x20       return Self.n\n\
     \n\
     \x20   def wide(self) -> Int where Self.n > 2:\n\
     \x20       return Self.n\n\
     \n\
     \x20   def tight(self) -> Int where Self.n + 1 == 3:\n\
     \x20       return Self.n\n\
     \n\
     def main():\n\
     \x20   var a = Cell[Point, 2](Point(1))\n\
     \x20   print(a.bits(), a.tight())\n\
     \x20   var b = Cell[Tracked, 5](Tracked(2))\n\
     \x20   print(b.wide())\n";

#[test]
fn trivial_value_and_pack_clauses_are_decided() {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(
            TRIVIAL_AND_VALUE_MEMBERS,
            std::path::Path::new("mono_test.mojo"),
        )
        .expect("compile trivial and value members");
    let specializer = Specializer::new(compiled.drop_elaborated_mir(), None);
    let cell = |argument: &str, n: i64| {
        let template = &specializer.structs["Cell"];
        let mut bindings = specializer.base_bindings();
        let arguments = [
            TyArg::Ty(TyKind::Struct(argument.into(), Vec::new()).into()),
            TyArg::Val(CtValue::Int(n)),
        ];
        bind_ty_args(&template.param_decls, &arguments, &mut bindings).unwrap();
        bindings
    };
    let proven = |member: &str, bindings: &Bindings| match specializer
        .availability(&specializer.declarations[member], bindings)
    {
        Availability::Proven => true,
        Availability::Disproven(_) => false,
        Availability::Undecided => panic!("`{member}` stays undecided"),
    };
    assert!(
        proven("Cell.bits", &cell("Point", 2)),
        "fields of `Int` copy bitwise"
    );
    assert!(
        !proven("Cell.bits", &cell("Tracked", 2)),
        "a user copy is not trivial"
    );
    assert!(proven("Cell.wide", &cell("Point", 5)));
    assert!(!proven("Cell.wide", &cell("Point", 2)));
    assert!(proven("Cell.tight", &cell("Point", 2)));
    assert!(!proven("Cell.tight", &cell("Point", 5)));

    let pack = test_binder("Ts");
    let mut bindings = specializer.base_bindings();
    bindings.values.insert(
        pack.clone(),
        CtValue::Tuple(
            [
                Ty::int(),
                TyKind::Struct("Tracked".into(), Vec::new()).into(),
            ]
            .map(CtValue::Type)
            .to_vec(),
        ),
    );
    let mut declaration = specializer.declarations["Cell.bits"].clone();
    let mut decide = |clause: GenericConstraint| {
        declaration.availability = vec![clause];
        match specializer.availability(&declaration, &bindings) {
            Availability::Proven => true,
            Availability::Disproven(_) => false,
            Availability::Undecided => panic!("a pack clause stays undecided"),
        }
    };
    let trivially = |all| GenericConstraint::PackPredicate {
        param: pack.clone(),
        predicate: mojito_types::types::PackPredicateRef::Trivial(
            mojito_types::types::TrivialLifecycle::Copyable,
        ),
        all,
    };
    assert!(decide(trivially(false)), "`Int` copies bitwise");
    assert!(!decide(trivially(true)), "`Tracked` does not");
    assert!(decide(GenericConstraint::PackContains {
        param: pack.clone(),
        element: ConstraintOperand::Type(Ty::int()),
    }));
    assert!(decide(GenericConstraint::Eq(
        ConstraintOperand::PackLength(pack.clone()),
        ConstraintOperand::Value(CtValue::IntLiteral(2.into())),
    )));
    // The variadic `Tuple` template answers by its elements.
    let tuple = |element: &str| {
        GenericConstraint::Trivial(
            mojito_types::types::TrivialLifecycle::Copyable,
            ConstraintOperand::Type(
                TyKind::Struct(
                    "Tuple".into(),
                    vec![
                        TyArg::Ty(Ty::int()),
                        TyArg::Ty(TyKind::Struct(element.into(), Vec::new()).into()),
                    ],
                )
                .into(),
            ),
        )
    };
    assert!(decide(tuple("Point")));
    assert!(!decide(tuple("Tracked")));
}

#[test]
fn a_struct_conformance_is_decided_from_its_declaration_when_asked() {
    let source = "trait Named:\n\
         \x20   def name(self) -> String:\n\
         \x20       ...\n\
         \n\
         trait Titled(Named):\n\
         \x20   def title(self) -> String:\n\
         \x20       ...\n\
         \n\
         @fieldwise_init\n\
         struct Book(Copyable, Movable, Titled):\n\
         \x20   var n: Int\n\
         \n\
         \x20   def name(self) -> String:\n\
         \x20       return \"book\"\n\
         \n\
         \x20   def title(self) -> String:\n\
         \x20       return \"Book\"\n\
         \n\
         def main():\n\
         \x20   var xs: List[Int] = [1]\n\
         \x20   print(len(xs), Book(1).title())\n";
    let compiled = mojito::Compiler::default()
        .with_std_package(true)
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile conformance program");
    assert!(compiled.imported_package().is_some());
    let specializer = Specializer::new(compiled.drop_elaborated_mir(), None);
    let list = Ty::new(TyKind::Struct("List".into(), vec![TyArg::Ty(Ty::int())]));
    let book = Ty::new(TyKind::Struct("Book".into(), Vec::new()));
    let conforms =
        |ty: &Ty, trait_name: &str| specializer.type_conforms(ty, trait_name, &mut HashSet::new());
    // The package answered no clause ahead of use: `List` decides
    // `Iterable` from its declared conformances.
    assert!(
        specializer.structs["List"]
            .conformances
            .iter()
            .all(|row| row.trait_name != "Iterable")
    );
    assert_eq!(conforms(&list, "Iterable"), Some(true));
    assert_eq!(
        conforms(&list, "Absable"),
        Some(false),
        "an operation trait"
    );
    assert_eq!(
        conforms(&list, "Named"),
        Some(false),
        "the importer's trait"
    );
    assert_eq!(conforms(&book, "Named"), Some(true), "through `Titled`");
    assert_eq!(conforms(&book, "Hashable"), Some(false));
    assert_eq!(conforms(&book, "Copyable"), Some(true), "a lifecycle row");
}

#[test]
fn comptime_for_unrolls_one_copy_per_iteration() {
    // The template carries the loop as a `comptime_for` header; each instance
    // replaces it with one copy of the body per index value, the reads of the
    // index folded, and keeps no header.
    let source = "def total[n: Int]() -> Int:\n\
         \x20   var sum = 0\n\
         \x20   comptime for i in range(n):\n\
         \x20       sum += i\n\
         \x20   return sum\n\
         def main():\n\
         \x20   print(total[3]())\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile the loop");
    let comptime_fors = |function: &MirFunction| {
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, MirTerm::ComptimeFor { .. }))
            .count()
    };
    let additions = |function: &MirFunction| {
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter(|instruction| matches!(instruction, MirInstr::BinOp { .. }))
            .count()
    };
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "total")
        .map(|(_, function)| function)
        .expect("the template is lowered");
    assert_eq!(comptime_fors(template), 1);
    assert_eq!(additions(template), 1);
    let specialized = specialize(
        compiled.drop_elaborated_mir(),
        &["main".to_string()],
        host_target().as_ref(),
    )
    .expect("specialize the loop");
    let instance = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("total$"))
        .map(|(_, function)| function)
        .expect("the instance is emitted");
    assert_eq!(comptime_fors(instance), 0);
    assert_eq!(additions(instance), 3, "one body copy per iteration");
    assert!(
        instance.blocks.iter().flat_map(|block| &block.instrs).all(
            |instruction| !matches!(instruction, MirInstr::UseVar { var, .. }
                if instance.var_names.get(*var as usize).is_some_and(|name| name == "i"))
        ),
        "every read of the index folds to its value"
    );
}

#[test]
fn comptime_for_steps_an_iterator_one_copy_per_element() {
    // A loop over a user iterator keeps an iterator header in the template;
    // each instance steps the iterator through the `paramfor_*` stubs and
    // copies the body once per element.
    let source = "@fieldwise_init\n\
         struct Count(Iterator, Copyable):\n\
         \x20   comptime Element = Int\n\
         \x20   var n: Int\n\
         \x20   def __next__(mut self) raises StopIteration -> Int:\n\
         \x20       if self.n <= 0:\n\
         \x20           raise StopIteration()\n\
         \x20       self.n -= 1\n\
         \x20       return self.n\n\
         \x20   def __iter__(self) -> Self:\n\
         \x20       return self.copy()\n\
         def total[n: Int]() -> Int:\n\
         \x20   var sum = 0\n\
         \x20   comptime for x in Count(n):\n\
         \x20       sum += x\n\
         \x20   return sum\n\
         def main():\n\
         \x20   print(total[3]())\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile the loop");
    let iterator_headers = |function: &MirFunction| {
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, MirTerm::ComptimeFor { .. }))
            .count()
    };
    let additions = |function: &MirFunction| {
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter(|instruction| matches!(instruction, MirInstr::BinOp { .. }))
            .count()
    };
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "total")
        .map(|(_, function)| function)
        .expect("the template is lowered");
    assert_eq!(iterator_headers(template), 1);
    let specialized = specialize(
        compiled.drop_elaborated_mir(),
        &["main".to_string()],
        host_target().as_ref(),
    )
    .expect("specialize the loop");
    let instance = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("total$"))
        .map(|(_, function)| function)
        .expect("the instance is emitted");
    assert!(
        instance
            .blocks
            .iter()
            .all(|block| !matches!(block.term, MirTerm::ComptimeFor { .. }))
    );
    assert_eq!(additions(instance), 3, "one body copy per element");
}

#[test]
fn comptime_for_index_typed_local_takes_a_slot_per_copy() {
    // A `var` whose type names the index is one slot per copy, each at its
    // iteration's type, and the template's slot leaves the instance — even
    // when the range is empty.
    let source = "def total[n: Int]() -> Int:\n\
         \x20   var sum = 0\n\
         \x20   comptime for i in range(1, n):\n\
         \x20       var v = SIMD[DType.int32, i](7)\n\
         \x20       sum += Int(v.reduce_add())\n\
         \x20   return sum\n\
         def main():\n\
         \x20   print(total[3]())\n\
         \x20   print(total[1]())\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile the loop");
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "total")
        .map(|(_, function)| function)
        .expect("the template is lowered");
    let index = template
        .blocks
        .iter()
        .find_map(|block| match &block.term {
            MirTerm::ComptimeFor { binder, .. } => Some(binder.clone()),
            _ => None,
        })
        .expect("the template keeps the loop");
    let (slot, _) = template
        .var_tys
        .iter()
        .find(|(_, ty)| mojito_types::types::names_binder(ty, &index))
        .expect("the template types `v` over the index");
    let name = &template.var_names[*slot as usize];
    let specialized = specialize(
        compiled.drop_elaborated_mir(),
        &["main".to_string()],
        host_target().as_ref(),
    )
    .expect("specialize the loop");
    let instance = |suffix: &str| {
        specialized
            .program
            .functions
            .iter()
            .find(|(candidate, _)| candidate.starts_with("total$") && candidate.ends_with(suffix))
            .map(|(_, function)| function)
            .expect("the instance is emitted")
    };
    let copies = |function: &MirFunction| -> Vec<Ty> {
        assert_eq!(function.n_vars, function.var_names.len());
        assert!(
            function
                .var_tys
                .values()
                .all(|ty| !mojito_types::types::names_binder(ty, &index))
        );
        assert!(
            !function.var_names.contains(name),
            "the template's slot is retired"
        );
        let mut types: Vec<Ty> = function
            .var_names
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.starts_with(&format!("{name}$unroll")))
            .map(|(slot, _)| function.var_tys[&(slot as mojito_hir::hir::VarId)].clone())
            .collect();
        types.sort_by_key(ToString::to_string);
        types
    };
    let widths: Vec<String> = copies(instance("V3"))
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(widths, ["Int32", "SIMD[DType.int32, 2]"]);
    assert!(copies(instance("V1")).is_empty());
}

#[test]
fn comptime_branch_selection_keeps_the_taken_arm() {
    // The template carries the region as a `comptime_branch`; each instance
    // keeps the taken arm alone, with the destroy drop elaboration placed at
    // its entry, and the untaken arm's blocks are gone.
    let source = "struct Thing(Movable):\n\
         \x20   var s: String\n\
         \x20   def __init__(out self, var s: String):\n\
         \x20       self.s = s^\n\
         \x20   def __del__(deinit self):\n\
         \x20       print(\"del\", self.s)\n\
         def consume(var t: Thing):\n\
         \x20   print(\"consume\", t.s)\n\
         def f[n: Int](flag: Bool):\n\
         \x20   var a = Thing(String(\"a\"))\n\
         \x20   comptime if n > 0:\n\
         \x20       consume(a^)\n\
         \x20   else:\n\
         \x20       print(\"else\")\n\
         \x20   if flag:\n\
         \x20       print(\"flag\")\n\
         def main():\n\
         \x20   f[1](True)\n";
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile the region");
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "f")
        .map(|(_, function)| function)
        .expect("the template is lowered");
    let comptime_branches = |function: &MirFunction| {
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, MirTerm::ComptimeBranch { .. }))
            .count()
    };
    assert_eq!(comptime_branches(template), 1);
    let specialized = specialize(
        compiled.drop_elaborated_mir(),
        &["main".to_string()],
        host_target().as_ref(),
    )
    .expect("specialize the region");
    let instance = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("f$"))
        .map(|(_, function)| function)
        .expect("the instance is emitted");
    assert_eq!(comptime_branches(instance), 0);
    assert!(instance.blocks.len() < template.blocks.len());
    assert_eq!(
        instance
            .blocks
            .iter()
            .filter(|block| matches!(block.term, MirTerm::Branch { .. }))
            .count(),
        1,
        "only the runtime `if` remains"
    );
}

#[test]
fn defaultable_pack_instance_builds_its_elements() {
    let source = "def build[*Ts: Movable & Defaultable & Writable & Deinitable]() -> Int:\n\
                  \x20   comptime for i in range(len(Ts)):\n\
                  \x20       var value = Ts[i]()\n\
                  \x20       print(value)\n\
                  \x20   return Ts.length\n\
                  \n\
                  def main():\n\
                  \x20   print(build[Int, Bool]())\n";
    let specialized = specialized_main(source);
    let (_, instance) = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("build$mono$"))
        .expect("an instance of `build`");
    assert!(
        !instance.var_names.iter().any(|name| name == "*Ts"),
        "a type pack has no frame slot: {:?}",
        instance.var_names
    );
    let body = instructions(&instance.blocks);
    assert!(
        !body
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::ConstructTypeParam { .. })),
        "each element construction is written as its type's default"
    );
    assert!(
        body.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Const {
                k: Const::Bool(false),
                ..
            }
        )),
        "the `Bool` element constructs as `False`"
    );
}

#[test]
fn dispatched_copy_of_builtin_instance_is_value_copy() {
    let source = "def ident[T: Copyable](x: T) -> T:\n\
                  \x20   var y = x.copy()\n\
                  \x20   return y^\n\
                  \n\
                  def main():\n\
                  \x20   print(ident(3), ident(UInt8(7)), ident(String(\"s\")))\n";
    let specialized = specialized_main(source);
    let instances: Vec<_> = specialized
        .program
        .functions
        .iter()
        .filter(|(name, _)| name.starts_with("ident$mono$"))
        .collect();
    assert_eq!(instances.len(), 3, "one instance per argument type");
    for (name, instance) in instances {
        let body = instructions(&instance.blocks);
        let copy_call = body.iter().find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method, resolved, ..
            } if method == "copy" => Some(resolved.as_deref()),
            _ => None,
        });
        if matches!(instance.param_types[0].kind(), TyKind::Struct(..)) {
            assert!(
                copy_call.is_some_and(|target| target.is_some_and(|t| t.contains("String.copy"))),
                "{name}: a struct's copy stays its `copy` call"
            );
            continue;
        }
        assert_eq!(copy_call, None, "{name}: no dispatched copy call remains");
        assert!(
            body.iter()
                .any(|instruction| matches!(instruction, MirInstr::CopyValue { .. })),
            "{name}: the copy is a value copy"
        );
        assert!(
            !body
                .iter()
                .any(|instruction| matches!(instruction, MirInstr::DefVar { in_place: true, .. })),
            "{name}: a value copy's binding is not in place"
        );
    }
}

#[test]
fn dispatched_copy_through_reference_binding_is_value_copy() {
    let source = "def firsts[T: Copyable & Deinitable](*values: T) -> List[T]:\n\
                  \x20   var out = List[T]()\n\
                  \x20   for value in values:\n\
                  \x20       out.append(value.copy())\n\
                  \x20   return out^\n\
                  \n\
                  def main():\n\
                  \x20   print(len(firsts(1, 2)), len(firsts(String(\"x\"))))\n";
    let specialized = specialized_main(source);
    let instances: Vec<_> = specialized
        .program
        .functions
        .iter()
        .filter(|(name, _)| name.starts_with("firsts$mono$"))
        .collect();
    assert_eq!(instances.len(), 2, "one instance per element type");
    for (name, instance) in instances {
        let body = instructions(&instance.blocks);
        let copy_call = body.iter().find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method, resolved, ..
            } if method == "copy" => Some(resolved.as_deref()),
            _ => None,
        });
        if name.contains("String") {
            assert!(
                copy_call.is_some_and(|target| target.is_some_and(|t| t.contains("String.copy"))),
                "{name}: a struct's copy stays its `copy` call"
            );
            continue;
        }
        assert_eq!(copy_call, None, "{name}: no dispatched copy call remains");
        assert!(
            body.iter()
                .any(|instruction| matches!(instruction, MirInstr::CopyValue { .. })),
            "{name}: the copy is a value copy"
        );
        for instruction in &body {
            if let MirInstr::LoadPlace { dest, place } = instruction
                && place.is_whole_root()
            {
                assert!(
                    !matches!(
                        instance.reg_types.get(&dest.0).map(Ty::kind),
                        Some(TyKind::Ref(..))
                    ),
                    "{name}: a whole-root load reads the referent, not the handle"
                );
            }
        }
    }
}

#[test]
fn reflected_field_type_construction_closes_per_unrolled_copy() {
    let source = "struct Zero(Copyable, Defaultable, Writable):\n\
                  \x20   var v: Int\n\
                  \x20   def __init__(out self):\n\
                  \x20       self.v = 0\n\
                  \x20   def write_to(self, mut writer: Some[Writer]):\n\
                  \x20       writer.write(\"Zero\")\n\
                  \n\
                  @fieldwise_init\n\
                  struct Holder(Copyable):\n\
                  \x20   var first: Zero\n\
                  \x20   var second: Float64\n\
                  \n\
                  def describe[T: AnyType]():\n\
                  \x20   comptime types = reflect[T].field_types()\n\
                  \x20   comptime for i in range(reflect[T].field_count()):\n\
                  \x20       comptime if conforms_to(types[i], Defaultable & Deinitable & Writable):\n\
                  \x20           print(types[i]())\n\
                  \n\
                  def main():\n\
                  \x20   describe[Holder]()\n";
    let compiled = mojito::Compiler::default()
        .with_snippet_module_scope()
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile reflection program");
    let mir = compiled.drop_elaborated_mir();
    let (_, template) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "describe")
        .expect("the `describe` template");
    assert!(
        instructions(&template.blocks)
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::ConstructType { .. })),
        "the template constructs the reflected field type"
    );
    let specialized =
        specialize(mir, &["main".to_string()], host_target().as_ref()).expect("specialize");
    let (_, instance) = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("describe$mono$"))
        .expect("an instance of `describe`");
    let body = instructions(&instance.blocks);
    assert!(
        !body
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::ConstructType { .. })),
        "each copy's construction is written as its field type's default"
    );
    assert!(
        body.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. } if func.0 == "Zero"
        )),
        "the `Zero` field constructs through its nullary initializer"
    );
    assert!(
        body.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Const {
                k: Const::Float(value),
                ..
            } if *value == 0.0
        )),
        "the `Float64` field constructs as `0.0`"
    );
}

#[test]
fn method_call_spread_expands_into_the_bound_pack_elements() {
    let pack = Ty::new(TyKind::Tuple(vec![Ty::int(), Ty::bool()]));
    let collector = MirPlace::root(0, Some(pack.clone()));
    let mut call = dunder_method_call(
        Reg(3),
        Reg(1),
        "tagged",
        Some("Sink.tagged".into()),
        None,
        vec![Reg(2), Reg(0)],
    );
    if let MirInstr::MethodCall {
        spread, arg_places, ..
    } = &mut call
    {
        *spread = Some(1);
        arg_places[1] = Some(collector.clone());
    }
    let mut function = MirFunction {
        blocks: vec![MirBlock {
            instrs: vec![
                MirInstr::LoadPlace {
                    dest: Reg(0),
                    place: collector,
                },
                call,
            ],
            term: MirTerm::Return(None),
        }],
        n_regs: 4,
        n_vars: 1,
        var_names: vec!["a".into()],
        n_params: 1,
        param_types: vec![pack.clone()],
        owned_params: vec![false],
        deinit_params: vec![false],
        ref_params: vec![false],
        returns_reference: false,
        var_tys: HashMap::from([(0, pack.clone())]),
        ret_ty: Some(Ty::none()),
        raises: false,
        error_ty: None,
        spans: mojito_mir::mir::SpanTable::default(),
        reg_types: HashMap::from([(0, pack), (2, Ty::int())]),
        removed_params: Vec::new(),
        callee_removed_params: Vec::new(),
    };
    expand_pack_spreads("fwd", &mut function).expect("expand the method-call spread");
    let instrs = &function.blocks[0].instrs;
    assert!(
        !instrs.iter().any(
            |instruction| matches!(instruction, MirInstr::LoadPlace { dest, .. } if *dest == Reg(0))
        ),
        "the collector's whole read must go once its elements are read"
    );
    let loaded: Vec<(Reg, MirPlace)> = instrs
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::LoadPlace { dest, place } => Some((*dest, place.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(loaded.len(), 2);
    let Some(MirInstr::MethodCall {
        args,
        arg_places,
        spread,
        ..
    }) = instrs.last()
    else {
        panic!("the method call must stay last");
    };
    assert_eq!(*spread, None);
    assert_eq!(args, &vec![Reg(2), loaded[0].0, loaded[1].0]);
    assert_eq!(arg_places.len(), 3);
    assert!(arg_places.iter().all(Option::is_none));
    assert_eq!(function.reg_types.get(&loaded[1].0.0), Some(&Ty::bool()));
}

#[test]
fn a_struct_pack_binds_element_by_element_or_whole() {
    let pack = ParamDecl::Type {
        id: mojito_types::param_expr::ParamId::new("$test:Row", 0),
        name: "*Ts".to_string(),
        bounds: Vec::new(),
        callable_bound: None,
        default: None,
        infer_only: false,
        variadic: true,
        constraints: Vec::new(),
    };
    let elements = vec![Ty::int(), Ty::bool(), Ty::float64()];
    let expected = CtValue::Tuple(
        elements
            .iter()
            .map(|element| CtValue::Type(element.clone()))
            .collect(),
    );
    let elementwise: Vec<TyArg> = elements.iter().cloned().map(TyArg::Ty).collect();
    let whole = vec![TyArg::Val(expected.clone())];
    let forwarded = vec![TyArg::Ty(TyKind::RuntimePack(elements.clone()).into())];
    for arguments in [elementwise, whole, forwarded] {
        let mut bindings = Bindings::default();
        bind_ty_args(std::slice::from_ref(&pack), &arguments, &mut bindings).unwrap();
        assert_eq!(bindings.values.get(&pack.binder()), Some(&expected));
        assert_eq!(
            bindings.types.get(&pack.binder()),
            Some(&Ty::new(TyKind::RuntimePack(elements.clone())))
        );
    }
}

const FAILED_INSTANCE: &str = r"
struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime q = [1, 2][i]
            print(q)

def reaches():
    S[5]().f()

def main():
    S[2]().f()
";

#[test]
fn failed_method_instance_is_reported_only_where_reached() {
    let compiler = mojito::Compiler::default()
        .with_snippet_module_scope()
        .with_exports(["reaches"]);
    let compiled = compiler
        .compile_source(FAILED_INSTANCE, std::path::Path::new("mono_test.mojo"))
        .expect("the check accepts every instance");
    let mir = compiled.drop_elaborated_mir();
    specialize(mir, &["main".to_string()], host_target().as_ref())
        .expect("`main` reaches no failed instance");
    let error = specialize(mir, &["reaches".to_string()], host_target().as_ref())
        .expect_err("`reaches` instantiates `S[5].f`");
    assert_eq!(error.kind, MonoErrorKind::Instantiation, "{error}");
    assert!(error.construct.contains("out of range"), "{error}");
}

#[test]
fn string_bearing_value_parameter_is_constructed_into_its_slot() {
    let specialized = specialized_main(
        "\
def g[p: Tuple[Int, String]]():
    print(p[1])

def main():
    g[(1, \"a\")]()
",
    );
    let (_, instance) = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("g$mono"))
        .expect("`g` was specialized");
    let instrs = instructions(&instance.blocks);
    assert!(
        instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. }
                if mojito_symbol::symbol::init_overload_struct(&func.0).is_some()
                    && mojito_symbol::symbol::overload_qualifier(&func.0)
                        .is_some_and(|qualifier| qualifier.ends_with("StringLiteral"))
        )),
        "the instance builds its `String` element: {instrs:?}"
    );
    assert!(
        instrs
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::DefVar { .. })),
        "the instance stores the value into the parameter's slot"
    );
    assert!(
        !instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Const {
                k: Const::Value(_),
                ..
            }
        )),
        "no string-bearing value is folded to a constant"
    );
}

#[test]
fn capturing_generic_callable_parameter_passes_only_its_environment() {
    let source = "def invoke[\n\
                  \x20   origins: OriginSet,\n\
                  \x20   //,\n\
                  \x20   callback: def[n: Int](Int) capturing[origins] -> Int,\n\
                  ]() -> Int:\n\
                  \x20   return callback[2](40)\n\
                  \n\
                  def main():\n\
                  \x20   var offset = 0\n\
                  \n\
                  \x20   @parameter\n\
                  \x20   def captured_add[n: Int](value: Int) -> Int:\n\
                  \x20       return value + n + offset\n\
                  \n\
                  \x20   print(invoke[captured_add]())\n";
    let specialized = specialized_main(source);
    let (name, instance) = specialized
        .program
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("invoke$mono"))
        .expect("`invoke` was specialized");
    // The instance folds the bound body and takes the captured `offset`
    // alone, by reference.
    assert_eq!(instance.ref_params, vec![true]);
    assert_eq!(instance.param_types, vec![Ty::int()]);
    let declaration = specialized
        .program
        .declarations
        .functions
        .iter()
        .find(|declaration| &declaration.lowered_name == name)
        .expect("the instance is declared");
    assert_eq!(declaration.ref_params, vec![true]);
    let instrs = instructions(&instance.blocks);
    assert!(
        instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, args, arg_places, .. }
                if func.0.starts_with("main$captured_add$mono")
                    && args.len() == 2
                    && arg_places.first().is_some_and(Option::is_some)
        )),
        "the call through the parameter is a direct call passing the environment: {instrs:?}"
    );
    assert!(
        !instrs
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::CallIndirect { .. })),
        "no indirect call survives: {instrs:?}"
    );
    let main = function(&specialized, "main");
    assert!(
        instructions(&main.blocks)
            .iter()
            .any(|instruction| matches!(
                instruction,
                MirInstr::Call { func, args, arg_places, .. }
                    if &func.0 == name
                        && args.len() == 1
                        && arg_places.first().is_some_and(Option::is_some)
            )),
        "`main` passes `offset`'s place, not the closure"
    );
}

/// Every struct origin argument `ty` carries, at any depth.
fn struct_origins(ty: &Ty) -> Vec<mojito_types::origin::Origin> {
    match ty.kind() {
        TyKind::Struct(_, arguments) => arguments
            .iter()
            .flat_map(|argument| match argument {
                TyArg::Ty(inner) => struct_origins(inner),
                TyArg::Origin(origin) => vec![origin.clone()],
                TyArg::Val(_) => Vec::new(),
            })
            .collect(),
        TyKind::Ref(reference) => struct_origins(&reference.referent),
        TyKind::Pointer { element, .. } => struct_origins(element),
        _ => Vec::new(),
    }
}

#[test]
fn an_instance_binds_origin_erased_types() {
    use mojito_types::origin::Origin;
    let source = "def keep[T: Copyable](value: T) -> T:\n\
         \x20   return value.copy()\n\
         def first[o: Origin](s: Span[Int, o]) -> Int:\n\
         \x20   return s[0]\n\
         def left() -> Int:\n\
         \x20   var xs: List[Int] = [1, 2]\n\
         \x20   return first(keep(Span(xs)))\n\
         def right() -> Int:\n\
         \x20   var ys: List[Int] = [3]\n\
         \x20   return first(keep(Span(ys)))\n\
         def main():\n\
         \x20   print(left(), right())\n";
    let instances = |specialized: &SpecializedProgram, template: &str| {
        let mut found = specialized
            .program
            .functions
            .iter()
            .filter(|(name, _)| name.starts_with(template))
            .map(|(name, function)| {
                let mut regs = function.reg_types.iter().collect::<Vec<_>>();
                regs.sort_by_key(|(reg, _)| **reg);
                (
                    name.clone(),
                    function.param_types.clone(),
                    regs.into_iter()
                        .map(|(reg, ty)| (*reg, ty.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    };
    let first_run = specialized_main(source);
    let keep = instances(&first_run, "keep");
    let [(_, params, _)] = keep.as_slice() else {
        panic!("one `keep` instance across both callers' origins: {keep:?}");
    };
    assert!(
        params
            .iter()
            .flat_map(struct_origins)
            .all(|origin| origin == Origin::Unbound),
        "`keep`'s bound `T` carries no caller origin: {params:?}"
    );
    let first = instances(&first_run, "first");
    let [(_, params, _)] = first.as_slice() else {
        panic!("one `first` instance: {first:?}");
    };
    assert!(
        params
            .iter()
            .flat_map(struct_origins)
            .any(|origin| matches!(origin, Origin::Param(_))),
        "`first` keeps its own origin binder: {params:?}"
    );
    let second_run = specialized_main(source);
    assert_eq!(keep, instances(&second_run, "keep"));
    assert_eq!(first, instances(&second_run, "first"));
}

/// Elaborate `source` from `main` with the parameters `removed` names
/// (function, parameter) removed by hand, as `remove-unused-params` would
/// once a struct spelling stops counting as a read.
fn specialized_with_removed(
    source: &str,
    removed: &[(&str, &str)],
) -> Result<SpecializedProgram, MonoError> {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile struct-parameter program");
    let mut program = compiled.drop_elaborated_mir().clone();
    for (function, parameter) in removed {
        let id = program
            .declarations
            .function(function)
            .and_then(|declaration| {
                declaration
                    .param_decls
                    .iter()
                    .find(|decl| decl.name() == *parameter)
            })
            .map_or_else(
                || panic!("`{function}` declares `{parameter}`"),
                |decl| decl.id().clone(),
            );
        program
            .functions
            .iter_mut()
            .find(|(name, _)| name == function)
            .unwrap_or_else(|| panic!("`{function}` has a body"))
            .1
            .removed_params
            .push(id);
    }
    specialize(&program, &["main".to_string()], host_target().as_ref())
}

/// The instances of `template` the concrete `main` calls, in call order.
fn main_callees(program: &SpecializedProgram, template: &str) -> Vec<String> {
    instructions(&function(program, "main").blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, .. } if func.0.starts_with(&format!("{template}$mono")) => {
                Some(func.0.clone())
            }
            _ => None,
        })
        .collect()
}

const BOX_SOURCE: &str = "struct Box[n: Int](Copyable, Movable):\n\
     \x20   var v: Int\n\
     \x20   def __init__(out self, v: Int):\n\
     \x20       self.v = v\n\
     def fb[n: Int](b: Box[n]) -> Int:\n\
     \x20   return b.v + 1\n\
     def main():\n\
     \x20   var b1 = Box[1](10)\n\
     \x20   var b2 = Box[2](20)\n\
     \x20   print(fb(b1))\n\
     \x20   print(fb(b2))\n";

#[test]
fn an_instance_erases_the_struct_argument_of_a_removed_parameter() {
    let specialized = specialized_with_removed(BOX_SOURCE, &[("fb", "n")])
        .expect("an instance takes an erased struct");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let callees = main_callees(&specialized, "fb");
    assert_eq!(callees, ["fb$mono", "fb$mono"], "one `fb` for both boxes");
    let erased: Ty = TyKind::Struct(
        "Box$mono$V$REMOVED$ARG".into(),
        vec![TyArg::Val(CtValue::RemovedArg)],
    )
    .into();
    assert_eq!(function(&specialized, "fb$mono").param_types, [erased]);
    let declared = specialized
        .program
        .declarations
        .structs
        .iter()
        .find(|declaration| declaration.name == "Box$mono$V$REMOVED$ARG")
        .expect("the erased instance is declared");
    assert_eq!(declared.fields, [("v".to_string(), Ty::int())]);
    // Without the removal each box keeps its own instance.
    let kept = specialized_with_removed(BOX_SOURCE, &[]).expect("specialize");
    assert_eq!(main_callees(&kept, "fb").len(), 2);
    assert_ne!(main_callees(&kept, "fb")[0], main_callees(&kept, "fb")[1]);
}

#[test]
fn an_erased_argument_leaves_instances_apart_on_the_kept_ones() {
    let source = "struct Pair[a: Int, b: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         def fp[a: Int, b: Int](p: Pair[a, b]) -> Int:\n\
         \x20   return a + p.v\n\
         def main():\n\
         \x20   print(fp(Pair[1, 2](10)))\n\
         \x20   print(fp(Pair[1, 5](20)))\n\
         \x20   print(fp(Pair[3, 2](30)))\n";
    let specialized =
        specialized_with_removed(source, &[("fp", "b")]).expect("an instance per kept `a`");
    let callees = main_callees(&specialized, "fp");
    assert_eq!(callees.len(), 3);
    assert_eq!(callees[0], callees[1], "`b` is erased: {callees:?}");
    assert_ne!(callees[0], callees[2], "`a` is kept: {callees:?}");
}

#[test]
fn an_erased_struct_whose_layout_reads_the_removed_parameter_is_an_error() {
    let source = "struct Arr[n: Int](Copyable, Movable):\n\
         \x20   var v: SIMD[DType.int64, Self.n]\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = SIMD[DType.int64, Self.n](v)\n\
         def fa[n: Int](a: Arr[n]) -> Int:\n\
         \x20   return 1\n\
         def main():\n\
         \x20   print(fa(Arr[2](10)))\n";
    let Err(error) = specialized_with_removed(source, &[("fa", "n")]) else {
        panic!("`Arr`'s layout needs `n`");
    };
    assert!(
        error.construct.contains("lays out parameter `n`"),
        "{error:?}"
    );
}

#[test]
fn a_struct_type_argument_erases_its_own_arguments() {
    let source = "struct Box[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         def fl[n: Int](l: List[Box[n]]) -> Int:\n\
         \x20   return len(l)\n\
         def main():\n\
         \x20   var l1 = List[Box[1]]()\n\
         \x20   l1.append(Box[1](1))\n\
         \x20   var l2 = List[Box[2]]()\n\
         \x20   print(fl(l1))\n\
         \x20   print(fl(l2))\n";
    let specialized =
        specialized_with_removed(source, &[("fl", "n")]).expect("one `fl` for both lists");
    assert_eq!(main_callees(&specialized, "fl"), ["fl$mono", "fl$mono"]);
    let [param] = function(&specialized, "fl$mono").param_types.as_slice() else {
        panic!("`fl` takes one parameter");
    };
    let TyKind::Struct(name, arguments) = peel_refs(param).kind() else {
        panic!("`fl` takes a list: {param}");
    };
    assert!(
        mojito_types::types::struct_template(name) == "List"
            && matches!(
                arguments.first(),
                Some(TyArg::Ty(element))
                    if matches!(element.kind(), TyKind::Struct(element, _)
                        if element == "Box$mono$V$REMOVED$ARG")
            ),
        "the list keeps its element type and erases the box's argument: {param}"
    );
}
