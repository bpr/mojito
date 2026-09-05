use super::*;

fn specialized_main(source: &str) -> SpecializedProgram {
    let compiler = mojito::Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, std::path::Path::new("mono_test.mojo"))
        .expect("compile iterator program");
    specialize(compiled.elaborated_mir(), &["main".to_string()])
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
fn bounded_user_iterator_types_the_split_slot_and_retargets_its_operations() {
    let source = "@fieldwise_init\n\
                  struct RangeIter:\n\
                  \x20   var cur: Int\n\
                  \x20   var stop: Int\n\
                  \n\
                  \x20   def __len__(self) -> Int:\n\
                  \x20       return self.stop - self.cur\n\
                  \n\
                  \x20   def __next__(mut self) -> Int:\n\
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
    let method = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::HasNext {
                method: Some(method),
                ..
            } => Some(method),
            _ => None,
        })
        .expect("bounded iteration reads a length method");
    assert!(
        specialized
            .program
            .functions
            .iter()
            .any(|(name, _)| name == method),
        "`{method}` must name a specialized function"
    );
    let target = instrs
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::Next {
                call: Some(call), ..
            } => Some(&call.target),
            _ => None,
        })
        .expect("bounded iteration advances through `__next__`");
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
    let source =
        include_str!("../../../../../assets/ok/generic_borrowed_dispatch_overloaded_iter.mojo");
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
        name: "T".into(),
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
fn callable_value_parameter_reaches_dependent_tuple_calls() {
    let source = include_str!("../../../../../assets/ok/container_owning_family_apis.mojo");
    let specialized = specialized_main(source);
    let toss_instances = specialized
        .program
        .functions
        .iter()
        .filter(|(name, _)| name.contains("main$toss") && name.contains("$mono$"))
        .count();
    assert!(
        toss_instances >= 2,
        "the callable value parameter must specialize for each Tuple element"
    );
}

#[test]
fn literal_actuals_merge_with_concrete_bindings_in_either_order() {
    let mut bindings = Bindings::default();
    // Receiver-first: `T := Int` from the concrete receiver, then a
    // literal-typed actual (`41 : IntLiteral`) — compatible, keeps `Int`.
    bind_type("T", &Ty::Int, &mut bindings).unwrap();
    bind_type("T", &Ty::IntLiteral, &mut bindings).unwrap();
    assert_eq!(bindings.types.get("T"), Some(&Ty::Int));

    // Result-last: the literal actual binds first, the concrete result
    // type upgrades it.
    let mut bindings = Bindings::default();
    bind_type("T", &Ty::IntLiteral, &mut bindings).unwrap();
    bind_type("T", &Ty::Int, &mut bindings).unwrap();
    assert_eq!(bindings.types.get("T"), Some(&Ty::Int));

    // Genuinely distinct concrete solutions still conflict, and the
    // message carries the structural forms (`Display` collapses
    // `IntLiteral` to `Int`).
    let mut bindings = Bindings::default();
    bind_type("T", &Ty::Int, &mut bindings).unwrap();
    let error = bind_type("T", &Ty::Float64, &mut bindings).unwrap_err();
    assert!(error.contains("conflicting"), "{error}");
    // Two different literal kinds conflict too.
    let mut bindings = Bindings::default();
    bind_type("T", &Ty::IntLiteral, &mut bindings).unwrap();
    assert!(bind_type("T", &Ty::FloatLiteral, &mut bindings).is_err());
}

#[test]
fn value_constructor_literal_arguments_bind_against_the_receiver_solution() {
    // The owned_pointer_api shape: the receiver's type arguments solve
    // `T := Int`, then the literal-typed constructor argument must merge
    // rather than conflict ("`Int` and `Int`").
    let source = "struct Box[T: Movable]:\n\
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
            Ty::Struct("__UninitStorage".into(), vec![TyArg::Ty(Ty::Int)]),
        )],
        &[(
            "_storage".into(),
            Ty::Struct(
                "__UninitStorage".into(),
                vec![TyArg::Ty(Ty::Struct("Recorder".into(), vec![]))],
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
    bindings.types.insert("T".into(), Ty::UInt);
    bindings.values.insert("n".into(), CtValue::Int(4));
    let ty = Ty::Struct(
        "Buffer".into(),
        vec![
            TyArg::Ty(Ty::Param {
                name: "T".into(),
                bounds: vec![],
                callable_bound: None,
            }),
            TyArg::Val(CtValue::Param("n".into())),
        ],
    );
    let Ty::Struct(name, args) = substitute_ty(&ty, &bindings).unwrap() else {
        panic!()
    };
    assert!(name.contains("$mono$"));
    assert_eq!(args, vec![TyArg::Ty(Ty::UInt), TyArg::Val(CtValue::Int(4))]);
}

#[test]
fn distinct_instantiations_split_into_owner_named_instances() {
    // The `List.grow` shape: `refresh` reaches `set` through the bare
    // in-body `self` receiver, which must carry the owner instance's
    // binding for `T` rather than the shared template spelling.
    let source = "struct Pairing[T: Copyable & Movable]:\n\
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
    // The calls reach the per-instantiation method clones (`refresh$y3:Int`),
    // each an instance of its owner; the constructor stays the template's.
    for expected in [
        "Pairing$mono$TInt.refresh$y3:Int",
        "Pairing$mono$TBool.refresh$y4:Bool",
        "Pairing$mono$TInt.set$y3:Int",
        "Pairing$mono$TBool.set$y4:Bool",
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
    bind_type("T", &first, &mut bindings).unwrap();
    bind_type("T", &second, &mut bindings).unwrap();
    // First solution wins; a mutability disagreement still conflicts.
    assert_eq!(bindings.types.get("T"), Some(&first));
    let mutable = Ty::Ref(mojito_types::origin::RefTy {
        referent: Box::new(Ty::Int),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Mutable,
    });
    assert!(bind_type("T", &mutable, &mut bindings).is_err());
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
            .any(|ty| matches!(ty, Ty::RuntimePack(elements) if elements == &[Ty::Int])),
        "the pack parameter reifies to a one-element runtime pack: {:?}",
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
