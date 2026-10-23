#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::MirTerm;
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
            } else {
                result.push(instruction);
            }
        }
    }
    result
}

/// Every instruction of `blocks`, each region's own included.
fn nested_instructions(blocks: &[MirBlock]) -> Vec<&MirInstr> {
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .flat_map(|instruction| {
            std::iter::once(instruction).chain(
                mojito_mir::mir::instruction_regions(instruction)
                    .into_iter()
                    .flat_map(|region| nested_instructions(region)),
            )
        })
        .collect()
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
    let target = nested_instructions(&main.blocks)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                resolved: Some(target),
                ..
            } if method == "__next__" => Some(target),
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
    // `_ZeroStartingRange.__next__` is `@inline(.always)`, so it is inlined
    // into the loop's `try`, as upstream inlines it; a call left names its
    // instance.
    let nested = nested_instructions(&main.blocks);
    assert!(
        nested
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::Try { .. })),
        "range iteration steps inside a `try`"
    );
    for target in nested.iter().filter_map(|instruction| match instruction {
        MirInstr::MethodCall {
            method,
            resolved: Some(target),
            ..
        } if method == "__next__" => Some(target),
        _ => None,
    }) {
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == target),
            "`{target}` must name a specialized function"
        );
    }
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
fn nominal_len_inlines_the_library_overload_to_a_resolved_dunder_call() {
    let source = "@fieldwise_init\n\
                  struct Measured(Sized):\n\
                  \x20   var n: Int\n\
                  \n\
                  \x20   def __len__(self) -> Int:\n\
                  \x20       return self.n\n\
                  \n\
                  def main():\n\
                  \x20   print(len(Measured(3)))\n";
    let specialized = specialized_main(source);
    let main = function(&specialized, "main");
    let instrs = nested_instructions(&main.blocks);
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
        .expect("the `@inline(.always)` library `len` must inline to a resolved `__len__` call");
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| *name == resolved),
        "the inlined target `{resolved}` must be a specialized function"
    );
    assert!(
        !instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. } if func.0 == "len" || func.0.starts_with("len$ov$")
        )),
        "no `len` call may survive the inlining"
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
fn a_homogeneous_variadic_has_one_instance_for_every_arity() {
    let source = "def total(*values: Int) -> Int:\n\
                  \x20   var acc: Int = 0\n\
                  \x20   for value in values:\n\
                  \x20       acc = acc + value\n\
                  \x20   return acc\n\
                  \n\
                  def count[T: Copyable](*values: T) -> Int:\n\
                  \x20   return len(values)\n\
                  \n\
                  def main():\n\
                  \x20   print(total(), total(7), total(1, 2, 3))\n\
                  \x20   print(count(1), count(1, 2), count(True), count(True, False, True))\n";
    let specialized = specialized_main(source);
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let instances = |prefix: &str| {
        specialized
            .program
            .functions
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| name.starts_with(prefix))
            .collect::<Vec<_>>()
    };
    let totals = instances("total");
    assert_eq!(
        totals.len(),
        1,
        "one instance takes every arity: {totals:?}"
    );
    let counts = instances("count");
    assert_eq!(counts.len(), 2, "one instance per element type: {counts:?}");
    let total = function(&specialized, totals[0]);
    assert!(
        total
            .var_tys
            .values()
            .any(|ty| matches!(ty.kind(), TyKind::VariadicPack(element) if element == &Ty::int())),
        "the pack stays a run-time collector: {:?}",
        total.var_tys
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
    let program = compiled.drop_elaborated_mir();
    let mut specializer = Specializer::new(program, None);
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
    let program = compiled.drop_elaborated_mir();
    let mut specializer = Specializer::new(program, None);
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
    let program = compiled.drop_elaborated_mir();
    let specializer = Specializer::new(program, None);
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
    let program = compiled.drop_elaborated_mir();
    let specializer = Specializer::new(program, None);
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
    // The template carries the loop as a `comptime.for` op; each instance
    // replaces it with one copy of the body per index value, the reads of the
    // index folded, and keeps no loop over the index.
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
        nested_instructions(&function.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::ComptimeFor { .. }))
            .count()
    };
    let additions = |function: &MirFunction| {
        nested_instructions(&function.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::BinOp { .. }))
            .count()
    };
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "total")
        .map(|(_, function)| function)
        .expect("the template is emitted");
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
    // A loop over a user iterator keeps its `comptime.for` op in the
    // template; each instance steps the iterator through the `paramfor_*`
    // stubs and copies the body once per element.
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
    let iterator_loops = |function: &MirFunction| {
        nested_instructions(&function.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::ComptimeFor { .. }))
            .count()
    };
    let additions = |function: &MirFunction| {
        nested_instructions(&function.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::BinOp { .. }))
            .count()
    };
    let template = compiled
        .drop_elaborated_mir()
        .functions
        .iter()
        .find(|(name, _)| name == "total")
        .map(|(_, function)| function)
        .expect("the template is emitted");
    assert_eq!(iterator_loops(template), 1);
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
    assert_eq!(iterator_loops(instance), 0);
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
        .expect("the template is emitted");
    let index = nested_instructions(&template.blocks)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::ComptimeFor { binder, .. } => Some(binder.clone()),
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
    // The template carries the region as a `comptime.if` op; each instance
    // keeps the taken arm alone, with the destroy drop elaboration placed in
    // it, and the untaken arm is gone.
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
        .expect("the template is emitted");
    let comptime_branches = |function: &MirFunction| {
        nested_instructions(&function.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::ComptimeIf { .. }))
            .count()
    };
    let prints_else = |function: &MirFunction| {
        nested_instructions(&function.blocks)
            .into_iter()
            .any(|instruction| matches!(instruction, MirInstr::Const { k: mojito_mir::mir::Const::Str(text), .. } if text == "else"))
    };
    assert_eq!(comptime_branches(template), 1);
    assert!(prints_else(template));
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
    assert!(!prints_else(instance), "the untaken arm is gone");
    assert_eq!(
        nested_instructions(&instance.blocks)
            .into_iter()
            .filter(|instruction| matches!(instruction, MirInstr::If { .. }))
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
            MirInstr::Call { func, .. }
                if mojito_symbol::symbol::lifecycle_constructor(&func.0)
                    == Some(("Zero", "__init__"))
        )),
        "the `Zero` field constructs through the `__init__` its `Defaultable` witness names"
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
fn method_call_spread_passes_the_bound_pack_itself() {
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
        callee_removed_args: Vec::new(),
        removed_args: Vec::new(),
        lowered_calls: Vec::new(),
    };
    let before = format!("{:?}", function.blocks[0].instrs);
    expand_pack_spreads("fwd", &mut function).expect("check the method-call spread");
    assert_eq!(
        format!("{:?}", function.blocks[0].instrs),
        before,
        "the call passes the caller's pack, read whole, at its spread position"
    );
}

#[test]
fn spread_of_an_unbound_pack_is_the_instance_boundary() {
    let pack = Ty::new(TyKind::VariadicPack(Ty::int()));
    let collector = MirPlace::root(0, Some(pack.clone()));
    let mut call = dunder_method_call(
        Reg(3),
        Reg(1),
        "tagged",
        Some("Sink.tagged".into()),
        None,
        vec![Reg(0)],
    );
    if let MirInstr::MethodCall { spread, .. } = &mut call {
        *spread = Some(0);
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
        reg_types: HashMap::from([(0, pack)]),
        removed_params: Vec::new(),
        callee_removed_params: Vec::new(),
        callee_removed_args: Vec::new(),
        removed_args: Vec::new(),
        lowered_calls: Vec::new(),
    };
    let error = expand_pack_spreads("fwd", &mut function)
        .expect_err("a spread of a pack the bindings leave open is unsupported");
    assert_eq!(error.construct, "a spread's pack is still a parameter");
}

#[test]
fn a_struct_pack_binds_element_by_element_or_whole() {
    let pack = ParamDecl::Type {
        id: mojito_types::param_expr::ParamId::new("$test:Row", 0),
        name: "*Ts".to_string(),
        bounds: Vec::new(),
        trait_param: None,
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
/// (function, parameter) removed by hand, beside those
/// `remove-unused-params` removed.
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
    // `remove-unused-params` removes `n` itself: `Box` lays out an `Int`
    // whatever `n` is.
    let removed = specialized_with_removed(BOX_SOURCE, &[]).expect("specialize");
    assert_eq!(main_callees(&removed, "fb"), ["fb$mono", "fb$mono"]);
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
fn an_erased_pointer_field_points_to_none() {
    let source = "from std.memory.alloc import unsafe_alloc\n\
         struct PtrBox[T: Copyable & Deinitable](Copyable, Movable):\n\
         \x20   var k: Int\n\
         \x20   var p: UnsafePointer[Self.T, MutUntrackedOrigin]\n\
         \x20   def __init__(out self, k: Int):\n\
         \x20       self.k = k\n\
         \x20       self.p = unsafe_alloc[Self.T](1)\n\
         def fpb[T: Copyable & Deinitable](p: PtrBox[T]) -> Int:\n\
         \x20   return p.k\n\
         def main():\n\
         \x20   print(fpb(PtrBox[Int](8)))\n\
         \x20   print(fpb(PtrBox[Float64](9)))\n";
    let specialized =
        specialized_with_removed(source, &[("fpb", "T")]).expect("one `fpb` for both boxes");
    assert_eq!(main_callees(&specialized, "fpb"), ["fpb$mono", "fpb$mono"]);
    let declared = specialized
        .program
        .declarations
        .structs
        .iter()
        .find(|declaration| {
            mojito_types::types::struct_template(&declaration.name) == "PtrBox"
                && declaration.name.contains("REMOVED")
        })
        .expect("the erased instance is declared");
    assert!(
        matches!(
            declared.fields.as_slice(),
            [(_, k), (_, p)] if *k == Ty::int()
                && matches!(p.kind(), TyKind::Pointer { element, .. } if *element == Ty::none())
        ),
        "the pointer field points to `None`: {:?}",
        declared.fields
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

/// The names of the instances of `template` the program holds, sorted.
fn instances_of(program: &SpecializedProgram, template: &str) -> Vec<String> {
    let mut names: Vec<String> = program
        .program
        .functions
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| {
            name.strip_prefix(template)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('$'))
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_receiver_method_that_reads_no_struct_parameter_is_one_instance() {
    let source = "struct Box[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   @no_inline\n\
         \x20   def get(self) -> Int:\n\
         \x20       return self.v\n\
         def main():\n\
         \x20   print(Box[1](1).get(), Box[2](2).get())\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(instances_of(&specialized, "Box.get"), ["Box.get$mono"]);
    let [receiver] = function(&specialized, "Box.get$mono")
        .param_types
        .as_slice()
    else {
        panic!("`get` takes its receiver");
    };
    assert!(
        mojito_types::types::erases_an_argument(peel_refs(receiver)),
        "the receiver erases `n`: {receiver}"
    );
}

#[test]
fn a_method_call_on_a_field_reads_the_root_struct_s_layout() {
    let source = "struct SBox[n: Int](Movable):\n\
         \x20   var s: String\n\
         \x20   def __init__(out self, var s: String):\n\
         \x20       self.s = s^\n\
         \x20   @no_inline\n\
         \x20   def take(var self) -> Int:\n\
         \x20       return self.s.byte_length()\n\
         def main():\n\
         \x20   print(SBox[1](\"ab\").take(), SBox[2](\"abc\").take())\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(instances_of(&specialized, "SBox.take"), ["SBox.take$mono"]);
}

/// A call of a member reads only the struct arguments the member kept, as
/// upstream's call passes the member's parameters as values: a method
/// call, a call on `self`, a subscript, an operator, and a prefix operator
/// whose member ignores `n` are one instance for every `n`, and a prefix
/// operator whose member reads `Self.n` one per `n`.
#[test]
fn a_member_call_reads_only_the_struct_arguments_the_member_kept() {
    let source = "struct NBox[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   @no_inline\n\
         \x20   def get(self) -> Int:\n\
         \x20       return self.v\n\
         \x20   @no_inline\n\
         \x20   def twice(self) -> Int:\n\
         \x20       return self.get() + self.get()\n\
         \x20   @no_inline\n\
         \x20   def __getitem__(self, i: Int) -> Int:\n\
         \x20       return self.v + i\n\
         \x20   @no_inline\n\
         \x20   def __add__(self, o: Self) -> Int:\n\
         \x20       return self.v + o.v\n\
         \x20   @no_inline\n\
         \x20   def __neg__(self) -> Int:\n\
         \x20       return Self.n - self.v\n\
         \x20   @no_inline\n\
         \x20   def __invert__(self) -> Int:\n\
         \x20       return self.v + 1\n\
         @no_inline\n\
         def fg[n: Int](b: NBox[n]) -> Int:\n\
         \x20   return b.get()\n\
         @no_inline\n\
         def fsub[n: Int](b: NBox[n]) -> Int:\n\
         \x20   return b[3]\n\
         @no_inline\n\
         def fop[n: Int](b: NBox[n]) -> Int:\n\
         \x20   return b + b\n\
         @no_inline\n\
         def fneg[n: Int](b: NBox[n]) -> Int:\n\
         \x20   return -b\n\
         @no_inline\n\
         def finv[n: Int](b: NBox[n]) -> Int:\n\
         \x20   return ~b\n\
         def main():\n\
         \x20   print(fg(NBox[1](3)), fg(NBox[2](4)))\n\
         \x20   print(NBox[1](1).twice(), NBox[2](2).twice())\n\
         \x20   print(fsub(NBox[1](1)), fsub(NBox[2](2)))\n\
         \x20   print(fop(NBox[1](1)), fop(NBox[2](2)))\n\
         \x20   print(fneg(NBox[1](3)), fneg(NBox[2](4)))\n\
         \x20   print(finv(NBox[1](3)), finv(NBox[2](4)))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    for template in ["fg", "fsub", "fop", "finv", "NBox.twice"] {
        assert_eq!(
            instances_of(&specialized, template),
            [format!("{template}$mono")]
        );
    }
    assert_eq!(
        instances_of(&specialized, "fneg"),
        ["fneg$mono$V1", "fneg$mono$V2"]
    );
}

/// An iteration step and a method whose result spells an erased type
/// argument read only the struct arguments the member kept too.
#[test]
fn an_iteration_and_an_erased_result_read_only_the_kept_arguments() {
    let source = "struct IntIter(Iterator, Movable):\n\
         \x20   comptime Element = Int\n\
         \x20   var i: Int\n\
         \x20   def __init__(out self, i: Int):\n\
         \x20       self.i = i\n\
         \x20   def __next__(mut self) raises StopIteration -> Int:\n\
         \x20       if self.i <= 0:\n\
         \x20           raise StopIteration()\n\
         \x20       self.i -= 1\n\
         \x20       return self.i\n\
         struct NBox[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   @no_inline\n\
         \x20   def __iter__(self) -> IntIter:\n\
         \x20       return IntIter(self.v)\n\
         struct TBox[T: AnyType](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   @no_inline\n\
         \x20   def again(self) -> Self:\n\
         \x20       return self.copy()\n\
         @no_inline\n\
         def fit[n: Int](b: NBox[n]) -> Int:\n\
         \x20   var s = 0\n\
         \x20   for x in b:\n\
         \x20       s += x\n\
         \x20   return s\n\
         @no_inline\n\
         def ft[T: AnyType](b: TBox[T]) -> Int:\n\
         \x20   return b.again().v\n\
         def main():\n\
         \x20   print(fit(NBox[1](3)), fit(NBox[2](4)))\n\
         \x20   print(ft(TBox[Int](5)), ft(TBox[String](6)))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(instances_of(&specialized, "fit"), ["fit$mono"]);
    assert_eq!(instances_of(&specialized, "ft"), ["ft$mono"]);
    assert_eq!(
        instances_of(&specialized, "TBox.again"),
        ["TBox.again$mono"]
    );
}

/// A destructor or copy constructor that reads no struct parameter is one
/// instance for every instance of its struct, as on the pin, and each
/// struct instance's member table names it.
#[test]
fn a_destructor_that_reads_no_struct_parameter_is_one_instance() {
    let source = "struct DBox[n: Int](Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   def __del__(deinit self):\n\
         \x20       print(self.v)\n\
         def main():\n\
         \x20   var a = DBox[1](1)\n\
         \x20   var b = DBox[2](2)\n\
         \x20   print(a.v, b.v)\n";
    assert_one_member_instance(source, "DBox", "__deinit__", "DBox.__deinit__$mono");
}

/// `b.copy()` names the constructor it runs and nothing else copies, so
/// neither member table lists it.
#[test]
fn a_copy_constructor_that_reads_no_struct_parameter_is_one_instance() {
    let source = "struct CBox[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   def __init__(out self, *, copy: Self):\n\
         \x20       self.v = copy.v + 100\n\
         def dup[n: Int](b: CBox[n]) -> Int:\n\
         \x20   var c = b.copy()\n\
         \x20   return c.v\n\
         def main():\n\
         \x20   print(dup(CBox[1](1)), dup(CBox[2](2)))\n";
    assert_one_instance_tabled(
        source,
        "CBox",
        "__copyinit__",
        "CBox.__copyinit__$mono",
        &[],
    );
}

/// A `CBox[n]` whose copy constructor adds `Self.n` when `reads_n`, and
/// whose `dup[n]` copy-constructs one explicitly.
fn copy_constructed_box_source(reads_n: bool) -> String {
    let added = if reads_n { " + Self.n" } else { "" };
    format!(
        "struct CBox[n: Int](Copyable, Movable):\n\
         \x20   var v: Int\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v\n\
         \x20   def __init__(out self, *, copy: Self):\n\
         \x20       self.v = copy.v{added}\n\
         def dup[n: Int](b: CBox[n]) -> Int:\n\
         \x20   return CBox[n](copy=b).v\n\
         def main():\n\
         \x20   print(dup(CBox[1](1)), dup(CBox[2](2)))\n"
    )
}

/// `CBox[n](copy=b)` passes the struct's parameters as its copy
/// constructor's, which restates them as an `__init__` does, so a copy
/// constructor that reads none is one instance.
#[test]
fn a_copy_construction_whose_constructor_reads_no_struct_parameter_is_one_instance() {
    assert_one_instance_tabled(
        &copy_constructed_box_source(false),
        "CBox",
        "__copyinit__",
        "CBox.__copyinit__$mono",
        &[],
    );
}

#[test]
fn a_copy_construction_whose_constructor_reads_its_struct_parameter_keeps_one_instance_each() {
    let specialized =
        specialized_with_removed(&copy_constructed_box_source(true), &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let mut instances = member_instances(&specialized, "CBox", "__copyinit__");
    instances.sort_unstable();
    assert_eq!(
        instances,
        ["CBox$mono$V1.__copyinit__", "CBox$mono$V2.__copyinit__"]
    );
}

/// A `WBox[n]` whose display and equality members read no struct
/// parameter.
const WBOX_SOURCE: &str = "@fieldwise_init\n\
     struct WBox[n: Int](Writable, Equatable, ImplicitlyCopyable):\n\
     \x20   var v: Int\n\
     \x20   @no_inline\n\
     \x20   def write_to(self, mut writer: Some[Writer]):\n\
     \x20       writer.write(\"W(\", self.v, \")\")\n\
     \x20   @no_inline\n\
     \x20   def __eq__(self, other: Self) -> Bool:\n\
     \x20       return self.v == other.v\n\
     def main():\n\
     \x20   var a = WBox[1](1)\n\
     \x20   var b = WBox[2](2)\n\
     \x20   print(a, b)\n\
     \x20   print(a == WBox[1](1), b != WBox[2](2))\n";

#[test]
fn a_display_member_that_reads_no_struct_parameter_is_one_instance() {
    assert_one_member_instance(WBOX_SOURCE, "WBox", "write_to", "WBox.write_to$mono");
}

#[test]
fn an_equality_member_that_reads_no_struct_parameter_is_one_instance() {
    assert_one_member_instance(WBOX_SOURCE, "WBox", "__eq__", "WBox.__eq__$mono");
}

#[test]
fn a_hash_member_that_hashes_a_field_is_one_instance() {
    let source = "from std.hash import Hasher\n\
         @fieldwise_init\n\
         struct HBox[n: Int](Hashable, ImplicitlyCopyable):\n\
         \x20   var v: Int\n\
         \x20   @no_inline\n\
         \x20   def __hash__[H: Hasher](self, mut hasher: H):\n\
         \x20       self.v.__hash__(hasher)\n\
         def main():\n\
         \x20   print(hash(HBox[1](1)) == hash(HBox[2](1)))\n";
    assert_one_member_instance(
        source,
        "HBox",
        "__hash__",
        "HBox.__hash__$mono$T$$module$std$hash$$ahash$AHasher$mono$V$0$$0$$0$$0$$$$SIMD$DType$uint64$$4$",
    );
}

#[test]
fn a_trait_requirement_that_reads_no_struct_parameter_is_one_table_instance() {
    let source = "trait Flier:\n\
         \x20   def fly(self) -> Int:\n\
         \x20       ...\n\
         @fieldwise_init\n\
         struct FBox[n: Int](Flier, ImplicitlyCopyable):\n\
         \x20   var v: Int\n\
         \x20   @no_inline\n\
         \x20   def fly(self) -> Int:\n\
         \x20       return self.v\n\
         def soar[F: Flier](f: F) -> Int:\n\
         \x20   return f.fly()\n\
         def main():\n\
         \x20   print(soar(FBox[1](1)), soar(FBox[2](2)))\n";
    assert_one_member_instance(source, "FBox", "fly", "FBox.fly$mono");
}

/// `source`'s `owner[1]` and `owner[2]` share one owner-less `instance` of
/// their `member`, which both member tables name.
fn assert_one_member_instance(source: &str, owner: &str, member: &str, instance: &str) {
    assert_one_instance_tabled(source, owner, member, instance, &[instance]);
}

/// `source`'s `owner[1]` and `owner[2]` share one owner-less `instance` of
/// their `member`, and each member table names `table` for it.
fn assert_one_instance_tabled(
    source: &str,
    owner: &str,
    member: &str,
    instance: &str,
    table: &[&str],
) {
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(member_instances(&specialized, owner, member), [instance]);
    for argument in ["V1", "V2"] {
        let name = format!("{owner}$mono${argument}");
        let declaration = specialized
            .program
            .declarations
            .structure(&name)
            .unwrap_or_else(|| panic!("`{name}` is declared"));
        assert_eq!(
            declaration
                .members_of(member)
                .map(|entry| entry.symbol.as_str())
                .collect::<Vec<_>>(),
            table,
            "`{name}`'s member table"
        );
    }
}

/// The concrete instances of `owner`'s method `member`, owner-less or
/// owned by one of `owner`'s instances.
fn member_instances<'s>(
    specialized: &'s SpecializedProgram,
    owner: &str,
    member: &str,
) -> Vec<&'s str> {
    specialized
        .program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| {
            mojito_symbol::symbol::split_method_symbol(name).is_some_and(|(of, method)| {
                (of == owner || of.starts_with(&format!("{owner}$mono$")))
                    && method.split('$').next() == Some(member)
            })
        })
        .collect()
}

/// The callees of `name`'s `call` instructions.
fn callees<'s>(specialized: &'s SpecializedProgram, name: &str) -> Vec<&'s str> {
    instructions(&function(specialized, name).blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, .. } => Some(func.0.as_str()),
            _ => None,
        })
        .collect()
}

/// A `Box[n]` whose constructor stores `v`, plus `n` when `reads_n`.
fn constructed_box_source(reads_n: bool) -> String {
    format!(
        "struct Box[n: Int](Movable):\n\
         \x20   var v: Int\n\
         \x20   @no_inline\n\
         \x20   def __init__(out self, v: Int):\n\
         \x20       self.v = v{}\n\
         def main():\n\
         \x20   var a = Box[1](1)\n\
         \x20   var b = Box[2](2)\n\
         \x20   print(a.v, b.v)\n",
        if reads_n { " + Self.n" } else { "" }
    )
}

/// A constructor that reads no struct parameter is one instance for every
/// struct instance, and each construction names it, as upstream's direct
/// call names the reduced constructor.
#[test]
fn a_constructor_that_reads_no_struct_parameter_is_one_instance() {
    let specialized =
        specialized_with_removed(&constructed_box_source(false), &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(
        member_instances(&specialized, "Box", "__init__"),
        ["Box.__init__$mono"]
    );
    assert_eq!(
        callees(&specialized, "main")
            .into_iter()
            .filter(|callee| callee.starts_with("Box"))
            .collect::<Vec<_>>(),
        ["Box.__init__$mono", "Box.__init__$mono"]
    );
}

#[test]
fn a_constructor_that_reads_its_struct_parameter_keeps_one_instance_per_struct_instance() {
    let specialized =
        specialized_with_removed(&constructed_box_source(true), &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let mut instances = member_instances(&specialized, "Box", "__init__");
    instances.sort_unstable();
    assert_eq!(instances, ["Box$mono$V1.__init__", "Box$mono$V2.__init__"]);
    assert_eq!(
        callees(&specialized, "main")
            .into_iter()
            .filter(|callee| callee.starts_with("Box"))
            .collect::<Vec<_>>(),
        ["Box$mono$V1.__init__", "Box$mono$V2.__init__"]
    );
}

/// A direct call of a function whose argument its body never uses names
/// the generator with that argument removed, and passes nothing for it; a
/// function value of it names the full generator, as upstream keeps the
/// original beside its `_REMOVED_ARG` clone.
#[test]
fn a_direct_call_names_the_generator_without_its_unused_argument() {
    let source = "@no_inline\n\
     def g(x: Int) -> Int:\n\
     \x20   return 8\n\
     def h(x: Int) -> Int:\n\
     \x20   return x\n\
     def main() raises:\n\
     \x20   var f = g\n\
     \x20   if Int(String(\"1\")) > 5:\n\
     \x20       f = h\n\
     \x20   print(f(1), g(2))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    assert_eq!(function(&specialized, "g").n_params, 1);
    assert_eq!(function(&specialized, "g$REMOVED$ARG").n_params, 0);
    let direct = instructions(&function(&specialized, "main").blocks)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == "g$REMOVED$ARG" => Some(args.len()),
            _ => None,
        });
    assert_eq!(direct, Some(0), "the direct call passes no argument");
}

/// A method whose body never uses its receiver leaves the generator a
/// resolved method call names without it, as upstream's
/// `k3(q::Holder)_REMOVED_ARG()`: the call passes no receiver, and nothing
/// left reads the struct's parameter, so every owner instance shares it.
#[test]
fn a_method_call_names_the_generator_without_its_unused_receiver() {
    let source = "struct Grid[n: Int](Copyable, Movable):\n\
     \x20   var k: Int\n\
     \x20   def __init__(out self, k: Int):\n\
     \x20       self.k = k\n\
     \x20   @no_inline\n\
     \x20   def k3(self) -> Int:\n\
     \x20       return 3\n\
     def main() raises:\n\
     \x20   var g1 = Grid[1](7)\n\
     \x20   var g2 = Grid[2](8)\n\
     \x20   print(g1.k3(), g2.k3())\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let name = "Grid.k3$REMOVED$ARG$mono";
    assert_eq!(function(&specialized, name).n_params, 0);
    let declaration = specialized
        .program
        .declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == name)
        .expect("the instance is declared");
    assert!(!declaration.has_receiver);
    let calls = instructions(&function(&specialized, "main").blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == name => Some(args.len()),
            MirInstr::MethodCall { method, .. } if method == "k3" => Some(usize::MAX),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls, [0, 0], "each call passes no receiver");
}

/// An argument a body only forwards to a callee's removed argument is
/// unused too, as upstream rewrote the call first: `fwd` has one reduced
/// instance for every `n`, and its call of `fa`'s passes no argument.
#[test]
fn a_forward_to_a_removed_argument_leaves_one_reduced_instance() {
    let source = "@fieldwise_init\n\
     struct Arr[n: Int](Copyable, Movable):\n\
     \x20   var v: SIMD[DType.int64, Self.n]\n\
     @no_inline\n\
     def fa[n: Int](a: Arr[n]) -> Int:\n\
     \x20   return 3\n\
     @no_inline\n\
     def fwd[n: Int](a: Arr[n]) -> Int:\n\
     \x20   return fa(a) + 1\n\
     def main() raises:\n\
     \x20   var a1 = Arr[1](SIMD[DType.int64, 1](1))\n\
     \x20   var a2 = Arr[2](SIMD[DType.int64, 2](2))\n\
     \x20   print(fwd(a1), fwd(a2))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let forwards = specialized
        .program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("fwd"))
        .collect::<Vec<_>>();
    assert_eq!(forwards, ["fwd$REMOVED$ARG$mono"]);
    let call = instructions(&function(&specialized, "fwd$REMOVED$ARG$mono").blocks)
        .into_iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0.starts_with("fa") => {
                Some((func.0.clone(), args.len()))
            }
            _ => None,
        });
    assert_eq!(call, Some(("fa$REMOVED$ARG$mono".to_string(), 0)));
}

/// A call omitting a removed argument evaluates no default for it, as
/// upstream's rewritten call passes nothing: `ign(1)` runs neither
/// `$default$ign$s` nor the read temporary's destructor, while `used(1)`
/// still evaluates its own.
#[test]
fn a_removed_argument_left_out_evaluates_no_default() {
    let source = "def make() -> String:\n\
     \x20   print(\"make\")\n\
     \x20   return String(\"made\")\n\
     @no_inline\n\
     def ign(a: Int, s: String = make()) -> Int:\n\
     \x20   return a\n\
     @no_inline\n\
     def used(a: Int, s: String = make()) -> Int:\n\
     \x20   return a + s.byte_length()\n\
     def main() raises:\n\
     \x20   print(ign(1), used(1))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let defaults = instructions(&function(&specialized, "main").blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, .. } if func.0.starts_with("$default$") => Some(func.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(defaults, ["$default$used$s"]);
}

#[test]
fn a_nested_body_ignoring_its_argument_is_called_as_its_reduced_generator() {
    let source = "@no_inline\n\
     def apply(f: def(Int) thin -> Int) -> Int:\n\
     \x20   return f(4)\n\
     def outer(k: Int) -> Int:\n\
     \x20   @no_inline\n\
     \x20   def cap(x: Int) {imm k} -> Int:\n\
     \x20       return k + 1\n\
     \x20   @no_inline\n\
     \x20   def ign(x: Int) -> Int:\n\
     \x20       return 3\n\
     \x20   var f = ign\n\
     \x20   return cap(1) + cap(2) + f(5) + apply(ign)\n\
     def main() raises:\n\
     \x20   print(outer(2))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let mut nested = specialized
        .program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("outer$"))
        .collect::<Vec<_>>();
    nested.sort_unstable();
    // `ign` escapes into `apply`, so its full instance stays beside the
    // reduced one its call through `f` names; `cap` is only called.
    assert_eq!(
        nested,
        [
            "outer$cap$REMOVED$ARG",
            "outer$ign",
            "outer$ign$REMOVED$ARG"
        ]
    );
    let calls = instructions(&function(&specialized, "outer").blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } => Some((func.0.clone(), args.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        [
            ("outer$cap$REMOVED$ARG".to_string(), 1),
            ("outer$cap$REMOVED$ARG".to_string(), 1),
            ("outer$ign$REMOVED$ARG".to_string(), 0),
            ("apply".to_string(), 1),
        ]
    );
}

#[test]
fn an_unread_borrowed_pack_leaves_one_reduced_instance_for_every_arity() {
    let source = "@no_inline\n\
     def fw(k: Int, *args: Int) -> Int:\n\
     \x20   return k\n\
     def main() raises:\n\
     \x20   print(fw(1, 2), fw(1, 2, 3))\n";
    let specialized = specialized_with_removed(source, &[]).expect("specialize");
    assert_eq!(
        mojito_mir::mir::verify::verify_concrete(&specialized.program),
        Vec::<String>::new()
    );
    let instances = specialized
        .program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("fw"))
        .collect::<Vec<_>>();
    assert_eq!(instances, ["fw$REMOVED$ARG"]);
    let calls = instructions(&function(&specialized, "main").blocks)
        .into_iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0.starts_with("fw") => Some(args.len()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls, [1, 1]);
}
