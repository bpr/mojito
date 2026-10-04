#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
        matches!(main.var_tys.get(&dest), Some(Ty::Struct(name, _)) if name == "RangeIter"),
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
        matches!(main.var_tys.get(&dest), Some(Ty::Struct(..))),
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
            first_count.1.var_tys.get(&dest),
            Some(Ty::Struct(name, _)) if name.starts_with("CountIter")
        ),
        "the dispatched iterator slot must be concretely typed: {:?}",
        first_count.1.var_tys.get(&dest)
    );
}

#[test]
fn structural_inference_rejects_conflicting_solutions() {
    let parameter = Ty::Param {
        binder: mojito_types::param_expr::ParamRef {
            id: mojito_types::param_expr::ParamId::new("$test:T", 0),
            name: "T".into(),
        },
        bounds: vec![],
        callable_bound: None,
    };
    let mut bindings = Bindings::default();
    unify(&parameter, &Ty::Int, &mut bindings).unwrap();
    assert!(
        unify(&parameter, &Ty::Bool, &mut bindings)
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
    let parameter = |owner: &str| Ty::Param {
        binder: binder(owner),
        bounds: vec![],
        callable_bound: None,
    };
    let mut bindings = Bindings::default();
    unify(&parameter("outer"), &Ty::Int, &mut bindings).unwrap();
    unify(&parameter("contract"), &Ty::Bool, &mut bindings).unwrap();
    assert_eq!(
        substitute_ty(&parameter("outer"), &bindings).unwrap(),
        Ty::Int
    );
    assert_eq!(
        substitute_ty(&parameter("contract"), &bindings).unwrap(),
        Ty::Bool
    );
    assert!(substitute_ty(&parameter("unbound"), &bindings).is_err());
}

#[test]
fn literal_actuals_merge_with_concrete_bindings_in_either_order() {
    let mut bindings = Bindings::default();
    // Receiver-first: `T := Int` from the concrete receiver, then a
    // literal-typed actual (`41 : IntLiteral`) — compatible, keeps `Int`.
    bind_type(&test_binder("T"), &Ty::Int, &mut bindings).unwrap();
    bind_type(&test_binder("T"), &Ty::IntLiteral, &mut bindings).unwrap();
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&Ty::Int));

    // Result-last: the literal actual binds first, the concrete result
    // type upgrades it.
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::IntLiteral, &mut bindings).unwrap();
    bind_type(&test_binder("T"), &Ty::Int, &mut bindings).unwrap();
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&Ty::Int));

    // Genuinely distinct concrete solutions still conflict, and the
    // message carries the structural forms (`Display` collapses
    // `IntLiteral` to `Int`).
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::Int, &mut bindings).unwrap();
    let error = bind_type(&test_binder("T"), &Ty::Float64, &mut bindings).unwrap_err();
    assert!(error.contains("conflicting"), "{error}");
    // Two different literal kinds conflict too.
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &Ty::IntLiteral, &mut bindings).unwrap();
    assert!(bind_type(&test_binder("T"), &Ty::FloatLiteral, &mut bindings).is_err());
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
    let pointer = |element: Ty| Ty::Pointer {
        element: Box::new(element),
        origin: mojito_types::origin::PointerOrigin::Static,
    };
    // The `_RawAlloc`/`List` shape: fields differing only behind a
    // pointer are one opaque word and drop inertly — benign to share.
    assert!(fields_equivalent(
        &[("ptr".into(), pointer(Ty::Int))],
        &[("ptr".into(), pointer(Ty::Float64))],
    ));
    // A payload-carrying difference (the `__UninitStorage` shape) is a
    // genuine layout/lifecycle hazard.
    assert!(!fields_equivalent(
        &[(
            "_storage".into(),
            Ty::Struct("__UninitStorage".into(), vec![TyArg::Ty(Ty::Int)].into()),
        )],
        &[(
            "_storage".into(),
            Ty::Struct(
                "__UninitStorage".into(),
                vec![TyArg::Ty(Ty::Struct("Recorder".into(), Vec::new().into()))].into(),
            ),
        )],
    ));
    // Field names and non-pointer types stay strict.
    assert!(!fields_equivalent(
        &[("a".into(), Ty::Int)],
        &[("b".into(), Ty::Int)],
    ));
    assert!(!fields_equivalent(
        &[("a".into(), Ty::Int)],
        &[("a".into(), Ty::Float64)],
    ));
}

#[test]
fn substitution_resolves_nested_type_and_value_arguments() {
    let mut bindings = Bindings {
        generic_templates: Rc::new(HashSet::from(["Buffer".to_string()])),
        ..Bindings::default()
    };
    bindings.types.insert(test_binder("T"), Ty::UInt);
    bindings.values.insert(
        mojito_types::param_expr::ParamRef {
            id: mojito_types::param_expr::ParamId::new("Holder", 1),
            name: "n".into(),
        },
        CtValue::Int(4),
    );
    let ty = Ty::Struct(
        "Buffer".into(),
        vec![
            TyArg::Ty(Ty::Param {
                binder: mojito_types::param_expr::ParamRef {
                    id: mojito_types::param_expr::ParamId::new("$test:T", 0),
                    name: "T".into(),
                },
                bounds: vec![],
                callable_bound: None,
            }),
            TyArg::Val(CtValue::Expr(ParamContext::detached().decl_ref(
                mojito_types::param_expr::ParamId::new("Holder", 1),
                "n",
                mojito_types::param_expr::MetaTy::int(),
            ))),
        ]
        .into(),
    );
    let Ty::Struct(name, args) = substitute_ty(&ty, &bindings).unwrap() else {
        panic!()
    };
    assert!(name.contains("$mono$"));
    assert_eq!(
        *args,
        vec![TyArg::Ty(Ty::UInt), TyArg::Val(CtValue::Int(4))]
    );
}

#[test]
fn value_binders_sharing_a_spelling_keep_their_own_solutions() {
    let mut bindings = Bindings {
        generic_templates: Rc::new(HashSet::from(["Grid".to_string()])),
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
    let ty = Ty::Struct(
        "Grid".into(),
        vec![reference("Grid.resize"), reference("Grid")].into(),
    );
    let Ty::Struct(_, args) = substitute_ty(&ty, &bindings).unwrap() else {
        panic!()
    };
    assert_eq!(
        *args,
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
    assert_eq!(field_ty("Pairing$mono$TInt"), Ty::Int);
    assert_eq!(field_ty("Pairing$mono$TBool"), Ty::Bool);
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
    let referent = Box::new(Ty::Int);
    let first = Ty::Ref(mojito_types::origin::RefTy {
        referent: referent.clone(),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Immutable,
    });
    let second = Ty::Ref(mojito_types::origin::RefTy {
        referent,
        origin: mojito_types::origin::Origin::Untracked { mutable: false },
        mutability: mojito_types::origin::Mutability::Immutable,
    });
    let mut bindings = Bindings::default();
    bind_type(&test_binder("T"), &first, &mut bindings).unwrap();
    bind_type(&test_binder("T"), &second, &mut bindings).unwrap();
    // First solution wins; a mutability disagreement still conflicts.
    assert_eq!(bindings.types.get(&test_binder("T")), Some(&first));
    let mutable = Ty::Ref(mojito_types::origin::RefTy {
        referent: Box::new(Ty::Int),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Mutable,
    });
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
            .any(|ty| matches!(ty, Ty::Tuple(elements) if elements == &[Ty::Int])),
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
    // erased its environment, a `ref` element type, and an indirect call
    // through a callable contract over the owner's binder.
    let sources = [
        "def main():\n\
         \x20   var k = 3\n\
         \x20   var scaled = [lambda (x: Int) {k} -> Int: x * k]\n\
         \x20   print(scaled[0](2))\n",
        "@fieldwise_init\n\
         struct RefList[origin: Origin[mut=True]]:\n\
         \x20   var values: List[ref[origin] Int]\n\
         \x20   def bump_first(mut self):\n\
         \x20       self.values[0] += 2\n\
         def main():\n\
         \x20   var keep = 4\n\
         \x20   ref a = keep\n\
         \x20   var refs = RefList([a])\n\
         \x20   refs.bump_first()\n\
         \x20   print(keep)\n",
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
fn concrete_verification_rejects_a_forwarded_compile_time_argument() {
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
            .any(|finding| finding.contains("keeps compile-time argument forwarding `T`")),
        "a parametric call forwards its binder: {findings:?}"
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

const NULLARY_SLOT_INIT: &str = "Slot.__init__$ov$";

/// The bindings of `Slot[argument]` over the compiled `CONDITIONAL_MEMBERS`.
fn slot_bindings(specializer: &Specializer<'_>, argument: Ty) -> Bindings {
    let template = specializer.structs["Slot"];
    let mut bindings = specializer.base_bindings();
    bind_ty_args(&template.param_decls, &[TyArg::Ty(argument)], &mut bindings).unwrap();
    bindings
}

#[test]
fn discovered_member_its_instance_disproves_is_left_out() {
    let specialized = specialized_main(CONDITIONAL_MEMBERS);
    let nullary = |owner: &str| {
        let name = mojito_symbol::symbol::retarget_method_symbol(NULLARY_SLOT_INIT, owner)
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
    let available = slot_bindings(&specializer, Ty::Int);
    specializer
        .enqueue(NULLARY_SLOT_INIT, available, Vec::new())
        .expect("`Int` is `Defaultable`");
    let unavailable = slot_bindings(
        &specializer,
        Ty::Struct("NoDefault".into(), Vec::new().into()),
    );
    let error = specializer
        .enqueue(NULLARY_SLOT_INIT, unavailable, Vec::new())
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
    let bindings = slot_bindings(&specializer, Ty::Tuple(Vec::new()));
    specializer
        .enqueue(NULLARY_SLOT_INIT, bindings, Vec::new())
        .expect("the clause holds");
    let error = specializer.run(&[]).unwrap_err();
    assert!(
        error.construct.contains("constructing type parameter"),
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
        let template = specializer.structs["Cell"];
        let mut bindings = specializer.base_bindings();
        let arguments = [
            TyArg::Ty(Ty::Struct(argument.into(), Vec::new().into())),
            TyArg::Val(CtValue::Int(n)),
        ];
        bind_ty_args(&template.param_decls, &arguments, &mut bindings).unwrap();
        bindings
    };
    let proven = |member: &str, bindings: &Bindings| match specializer
        .availability(specializer.declarations[member], bindings)
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
            [Ty::Int, Ty::Struct("Tracked".into(), Vec::new().into())]
                .map(|ty| CtValue::Type(Box::new(ty)))
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
        element: ConstraintOperand::Type(Ty::Int),
    }));
    assert!(decide(GenericConstraint::Eq(
        ConstraintOperand::PackLength(pack.clone()),
        ConstraintOperand::Value(CtValue::IntLiteral(2.into())),
    )));
    // The variadic `Tuple` template answers by its elements.
    let tuple = |element: &str| {
        GenericConstraint::Trivial(
            mojito_types::types::TrivialLifecycle::Copyable,
            ConstraintOperand::Type(Ty::Struct(
                "Tuple".into(),
                vec![
                    TyArg::Ty(Ty::Int),
                    TyArg::Ty(Ty::Struct(element.into(), Vec::new().into())),
                ]
                .into(),
            )),
        )
    };
    assert!(decide(tuple("Point")));
    assert!(!decide(tuple("Tracked")));
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
            MirTerm::ComptimeFor { index, .. } => Some(index.clone()),
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
