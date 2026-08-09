//! Phase 2 (HIR/CFG → flattened MIR) tests. They check that expressions flatten to
//! A-Normal Form, that writes lower through places, and that the program driver
//! produces one function per `def` / method.

use mojito::hir::Cfg;
use mojito::mir::{MirInstr, MirPlace, MirSubscriptArg, MirTerm, Proj, lower_cfg, lower_program};
use mojito::{Compiler, SemanticAdjustment, Ty, parse};
use std::path::Path;

#[test]
fn free_function_union_reference_result_retains_a_typed_handle_slot() {
    let source = include_str!("../assets/origin_ok/returned_union.mojo");
    let syntax = parse(source).expect("parse union-origin reference result");
    let checked = mojito::check_program(&syntax).expect("check union-origin reference result");
    let mir = mojito::mir::lower_checked_program(&checked);
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let selected = main
        .var_names
        .iter()
        .position(|name| name == "selected")
        .expect("selected slot") as u32;
    assert!(
        matches!(main.var_tys.get(&selected), Some(Ty::Ref(_))),
        "reference assignment must not overwrite its handle slot type: {main:#?}"
    );
}

/// Lower a single-block snippet and return that block's instructions.
fn instrs(src: &str) -> Vec<MirInstr> {
    let mir = lower_cfg(&Cfg::build(&parse(src).expect("parse error")));
    assert_eq!(
        mir.blocks.len(),
        1,
        "snippet should be one straight-line block"
    );
    mir.blocks.into_iter().next().unwrap().instrs
}

#[test]
fn nominal_membership_retains_the_borrowed_container_place() {
    let source = "from std.collections.set import Set\n\ndef main() raises:\n    var values: Set[_] = {1, 2}\n    print(1 in values, 3 not in values)\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile nominal membership");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    let membership_calls = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method, recv_place, ..
            } if method == "__contains__" => Some(recv_place),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(membership_calls.len(), 2, "{instructions:#?}");
    assert!(
        membership_calls.iter().all(|place| place.is_some()),
        "membership must borrow the caller's container place: {instructions:#?}"
    );
    assert!(
        !instructions.iter().any(|instruction| matches!(
            instruction,
            MirInstr::BinOp {
                op: mojito::ast::InfixOp::In | mojito::ast::InfixOp::NotIn,
                ..
            }
        )),
        "nominal membership must not lower to a value-only BinOp: {instructions:#?}"
    );
}

#[test]
fn consuming_nominal_element_read_retains_explicit_accessor_dispatch() {
    let source = include_str!("../conformance/fixtures/owned_nominal_element_copy.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("owned_nominal_element_copy.mojo"))
        .expect("compile owned nominal element copy");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let dispatches = mir
        .functions
        .iter()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| &function.blocks)
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::Index {
                call, intrinsic, ..
            } => Some((call.as_ref(), intrinsic)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispatches.len(), 1, "{mir:#?}");
    assert_eq!(
        dispatches[0].0.map(|call| call.target.as_str()),
        Some("List.__getitem__$ov$Int")
    );
    assert_eq!(*dispatches[0].1, None);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
}

#[test]
fn consuming_dict_element_arguments_retain_explicit_accessor_dispatch() {
    let source = "from std.collections.dict import Dict\n\ndef take(value: Int) -> Int:\n    return value\n\ndef main() raises:\n    var values = Dict[String, Int]()\n    values[\"one\"] = 10\n    var copied: Int = values[\"one\"]\n    print(copied, take(values[\"one\"]))\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("owned_dict_element_copy.mojo"))
        .expect("compile owned Dict element copies");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let dispatches = mir
        .functions
        .iter()
        .filter(|(name, _)| name == "main")
        .flat_map(|(_, function)| &function.blocks)
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::Index {
                call, intrinsic, ..
            } => Some((call.as_ref(), intrinsic)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispatches.len(), 2, "{mir:#?}");
    assert!(dispatches.iter().all(|(call, _)| call.is_some()));
    assert!(
        dispatches
            .iter()
            .all(|(call, _)| call.is_some_and(|call| call.target.starts_with("Dict.__getitem__")))
    );
    assert!(dispatches.iter().all(|(_, intrinsic)| intrinsic.is_none()));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
}

#[test]
fn projected_nominal_reference_executes_accessor_before_extending_the_place() {
    let source = include_str!("../conformance/fixtures/projected_subscript_reference.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("projected_subscript_reference.mojo"))
        .expect("compile projected nominal reference");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    let dict_calls = instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction,
                MirInstr::Index {
                    call: Some(call),
                    intrinsic: None,
                    ..
                } if call.target.starts_with("Dict.__getitem__")
            )
        })
        .count();
    assert_eq!(dict_calls, 4, "{instructions:#?}");
    assert!(
        instructions.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { loans, .. }
                    if loans.iter().any(|loan| {
                        loan.place.through.is_some()
                            && loan.place.proj.iter().any(
                                |projection| matches!(projection, Proj::Field(field) if field == "value")
                            )
                    })
            )
        }),
        "projected field must extend the materialized accessor handle: {instructions:#?}"
    );
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
}

#[test]
fn projected_pointer_actual_retains_one_accessor_handle_and_one_dynamic_index() {
    let source = include_str!("../conformance/fixtures/projected_pointer_subscript.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("projected_pointer_subscript.mojo"))
        .expect("compile projected pointer actuals");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();

    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    MirInstr::MethodCall { method, .. } if method == "next"
                )
            })
            .count(),
        2,
        "each source dynamic index must be evaluated exactly once: {instructions:#?}"
    );
    let retained = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::Call {
                func, arg_places, ..
            } if matches!(func.0.as_str(), "bump" | "observe") => {
                arg_places.first().and_then(Option::as_ref)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2, "{instructions:#?}");
    for place in retained {
        assert_eq!(place.through, Some(place.root), "{place:#?}");
        assert!(matches!(
            main.var_tys.get(&place.root),
            Some(Ty::Ref(reference))
                if matches!(reference.referent.as_ref(), Ty::Struct(name, _) if name == "Buffer")
        ));
        assert!(matches!(
            place.proj.as_slice(),
            [Proj::Field(field), Proj::Index(_)] if field == "data"
        ));
    }
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
}

#[test]
fn augmented_nominal_subscript_retains_both_calls_and_one_operand_evaluation() {
    let source = include_str!("../conformance/fixtures/augmented_subscript_contract.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("augmented_subscript_contract.mojo"))
        .expect("compile augmented nominal subscript");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();

    let position = |predicate: &dyn Fn(&MirInstr) -> bool| {
        instructions
            .iter()
            .position(|instruction| predicate(instruction))
            .expect("expected MIR instruction")
    };
    let index_call = position(&|instruction| {
        matches!(
            instruction,
            MirInstr::Index { call: Some(call), .. }
                if call.target.starts_with("Counter.__getitem__")
        )
    });
    let setter_call = position(&|instruction| {
        matches!(
            instruction,
            MirInstr::MultiSet { call, .. }
                if call.target.starts_with("Counter.__setitem__")
        )
    });
    let index_evaluation = position(
        &|instruction| matches!(instruction, MirInstr::Call { func, .. } if func.0 == "next_index"),
    );
    let rhs_evaluation = position(
        &|instruction| matches!(instruction, MirInstr::Call { func, .. } if func.0 == "rhs"),
    );
    assert!(
        index_evaluation < rhs_evaluation
            && rhs_evaluation < index_call
            && index_call < setter_call,
        "current Mojo order is index, RHS, getter, setter: {instructions:#?}"
    );

    let MirInstr::Index {
        base,
        index,
        base_place: Some(base_place),
        call: Some(getter),
        ..
    } = instructions[index_call]
    else {
        unreachable!()
    };
    let MirInstr::MultiSet {
        receiver,
        args,
        call: setter,
        ..
    } = instructions[setter_call]
    else {
        unreachable!()
    };
    assert_ne!(receiver, base);
    assert!(
        instructions[index_call + 1..setter_call]
            .iter()
            .any(|instruction| matches!(
                instruction,
                MirInstr::LoadPlace { dest, place }
                    if dest == receiver
                        && place.root == base_place.root
                        && place.proj.len() == base_place.proj.len()
            )),
        "a parametric `ref self` getter requires the retained receiver place to be reloaded before the setter: {instructions:#?}"
    );
    assert!(matches!(args.as_slice(), [MirSubscriptArg::Index(value)] if value == index));
    assert!(getter.target.starts_with("Counter.__getitem__"));
    assert!(setter.target.starts_with("Counter.__setitem__"));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
}

#[test]
fn augmented_mutating_index_reloads_typed_source_before_setter() {
    let source = include_str!("../conformance/fixtures/augmented_mutating_index.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("augmented_mutating_index.mojo"))
        .expect("compile augmented mutating index");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}\n{instructions:#?}",
        mir.invariant_errors
    );
    let errors = mojito::mir::verify::verify(&mir);
    assert!(errors.is_empty(), "{errors:?}\n{instructions:#?}");
    let getter = instructions
        .iter()
        .position(|instruction| matches!(instruction, MirInstr::Index { .. }))
        .expect("getter");
    let setter = instructions
        .iter()
        .position(|instruction| matches!(instruction, MirInstr::MultiSet { .. }))
        .expect("setter");
    let reload = instructions[getter + 1..setter]
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::LoadPlace { dest, place } => Some((dest, place)),
            _ => None,
        })
        .expect("mutated index must be reloaded before the setter");
    assert_eq!(reload.1.ty, Some(Ty::Int), "{instructions:#?}");
    assert_eq!(main.reg_types.get(&reload.0.0), Some(&Ty::Int));
}

#[test]
fn implicitly_copied_tuple_transform_receivers_do_not_retain_a_consuming_place() {
    let source = "def main():\n    var pair = Tuple(1, 2)\n    var suffix = Tuple(3)\n    var reversed = pair.reverse()\n    var joined = pair.concat(suffix)\n    print(pair, suffix, reversed, joined)\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile Tuple transforms");
    let checked_calls = compiled
        .checked()
        .expressions()
        .iter()
        .filter(|expression| {
            matches!(
                &expression.syntax.kind,
                mojito::ast::ExprKind::MethodCall { method, .. }
                    if matches!(method.as_str(), "reverse" | "concat")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(checked_calls.len(), 2);
    assert!(
        checked_calls.iter().all(|expression| {
            expression.adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    SemanticAdjustment::ImplicitlyCopyConsumingReceiver
                )
            })
        }),
        "{:#?}",
        checked_calls
    );

    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let calls = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method, recv_place, ..
            } if matches!(method.as_str(), "reverse" | "concat") => {
                Some((method.as_str(), recv_place))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "{calls:#?}");
    assert!(calls.iter().all(|(_, place)| place.is_none()), "{calls:#?}");
    let pair = main
        .var_names
        .iter()
        .position(|name| name == "pair")
        .expect("pair variable") as u32;
    assert!(
        !main
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(instruction, MirInstr::ConsumeVar { var } if *var == pair)),
        "an implicit receiver copy must not consume the source place"
    );
    let suffix = main
        .var_names
        .iter()
        .position(|name| name == "suffix")
        .expect("suffix variable") as u32;
    assert!(
        !main.blocks.iter().flat_map(|block| &block.instrs).any(
            |instruction| matches!(instruction, MirInstr::ConsumeVar { var } if *var == suffix)
        ),
        "an implicitly copied deinit argument must not consume its source place"
    );
}

#[test]
fn parameterized_implicit_receiver_copy_drops_the_source_place_metadata() {
    let source = "@fieldwise_init\nstruct Counter(ImplicitlyCopyable):\n    var value: Int\n    def take_param[n: Int](deinit self) -> Int:\n        return self.value + n\n\ndef main():\n    var counter = Counter(40)\n    print(counter.take_param[2]())\n    print(counter.value)\n";
    let mir = lower_program(&parse(source).expect("parse")).expect("checked lowering");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let call = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                recv_place,
                param_decls,
                ..
            } if method == "take_param" => Some((recv_place, param_decls)),
            _ => None,
        })
        .expect("parameterized method call");
    assert!(call.0.is_none());
    assert_eq!(call.1.len(), 1);
    let counter = main
        .var_names
        .iter()
        .position(|name| name == "counter")
        .expect("counter variable") as u32;
    assert!(
        !main.blocks.iter().flat_map(|block| &block.instrs).any(
            |instruction| matches!(instruction, MirInstr::ConsumeVar { var } if *var == counter)
        )
    );
}

#[test]
fn raising_iterator_lowers_to_typed_try_next() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct I:\n    var current: Int\n    var end: Int\n    def __next__(mut self) raises StopIteration -> Int:\n        if self.current >= self.end:\n            raise StopIteration(0)\n        var result = self.current\n        self.current += 1\n        return result\n\n@fieldwise_init\nstruct C:\n    var end: Int\n    def __iter__(self) -> I:\n        return I(0, self.end)\n\ndef main():\n    for value in C(2):\n        print(value)\n";
    let program = lower_program(&parse(source).expect("parse")).expect("checked lowering");
    let main = program
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .map(|(_, function)| function)
        .expect("main MIR");
    let instructions: Vec<_> = main.blocks.iter().flat_map(|block| &block.instrs).collect();
    let (dest, call) = instructions
        .iter()
        .copied()
        .find_map(|instruction| match instruction {
            MirInstr::TryNext {
                dest,
                call,
                exhaustion: mojito::Ty::Struct(name, arguments),
                ..
            } if name == "StopIteration" && arguments.is_empty() => Some((dest, call)),
            _ => None,
        })
        .expect("typed TryNext");
    assert_eq!(main.reg_types.get(&dest.0), Some(&mojito::Ty::Int));
    assert_eq!(&call.result_ty, &mojito::Ty::Int);
    assert!(call.reference_result.is_none());
    assert!(!instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::HasNext { .. } | MirInstr::Next { .. }
    )));
}

#[test]
fn reference_yielding_try_next_retains_a_typed_reference_destination() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct RefIter[o: Origin[mut=False]]:\n    var source: ref[o] Int\n    var done: Bool\n    def __next__(mut self) raises StopIteration -> ref[o] Int:\n        if self.done:\n            raise StopIteration(0)\n        self.done = True\n        return self.source\n\n@fieldwise_init\nstruct RefSource:\n    var value: Int\n    def __iter__(ref self) -> RefIter:\n        ref value = self.value\n        return RefIter(value, False)\n\ndef main():\n    var source = RefSource(42)\n    for item in source:\n        print(item)\n";
    let program = lower_program(&parse(source).expect("parse")).expect("checked lowering");
    let main = &program
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let (dest, call) = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::TryNext { dest, call, .. } => Some((dest, call)),
            _ => None,
        })
        .expect("reference-yielding TryNext");
    let destination_ty = main
        .reg_types
        .get(&dest.0)
        .expect("typed TryNext destination");
    assert!(matches!(
        destination_ty,
        mojito::Ty::Ref(reference) if reference.referent.as_ref() == &mojito::Ty::Int
    ));
    assert_eq!(&call.result_ty, destination_ty);
    assert_eq!(
        call.reference_result.as_ref(),
        match destination_ty {
            mojito::Ty::Ref(reference) => Some(reference),
            _ => None,
        }
    );
}

// Pins the erased machinery via the raw check_program seam; under the
// authoritative Compiler this program's closed applications monomorphize
// and the adapter appears only in retained-template residue.
#[test]
fn abstract_next_calls_retain_the_copyable_reference_adapter() {
    let source = include_str!("../conformance/fixtures/copyable_iterator_refinement.mojo");
    let program = lower_program(&parse(source).expect("parse")).expect("checked lowering");

    let take = &program
        .functions
        .iter()
        .find(|(name, _)| name == "take")
        .expect("generic take MIR")
        .1;
    let direct = take
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                resolved,
                reference_result,
                result_adapter,
                ..
            } if method == "__next__" => Some((resolved, reference_result, result_adapter)),
            _ => None,
        })
        .expect("abstract __next__ method call");
    assert!(
        direct
            .0
            .as_deref()
            .is_some_and(|target| target.starts_with("__trait_dispatch.__next__"))
    );
    assert!(direct.1.is_none(), "abstract contract has a value ABI");
    assert_eq!(
        *direct.2,
        Some(mojito::checked::CheckedResultAdapter::CopyIteratorReference)
    );

    let loop_source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    let loop_program =
        lower_program(&parse(loop_source).expect("parse")).expect("checked lowering");
    let first = &loop_program
        .functions
        .iter()
        .find(|(name, _)| name == "first")
        .expect("generic first MIR")
        .1;
    let loop_adapter = first
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Next {
                call: Some(call), ..
            }
            | MirInstr::TryNext { call, .. } => Some(call.result_adapter),
            _ => None,
        })
        .expect("abstract iterator next call");
    assert_eq!(
        loop_adapter,
        Some(mojito::checked::CheckedResultAdapter::CopyIteratorReference)
    );
}

#[test]
fn bounded_iterator_carries_its_checked_element_type_into_mir() {
    let source = "def main():\n    for value in range(2):\n        print(value)\n";
    let program = lower_program(&parse(source).expect("parse")).expect("checked lowering");
    let main = &program
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let element = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Next { dest, .. } => Some(*dest),
            _ => None,
        })
        .expect("bounded iterator Next");
    assert_eq!(main.reg_types.get(&element.0), Some(&mojito::Ty::Int));
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::DefVar {
                    src,
                    binding_ty: Some(mojito::Ty::Int),
                    ..
                } if *src == element
            ))
    );
}

#[test]
fn lowers_a_simple_function_body() {
    // `var x = 1 + 2; return x` — one block, flattened to ANF, returning a reg.
    let cfg = Cfg::build(&parse("var x: Int = 1 + 2\nreturn x\n").unwrap());
    let mir = lower_cfg(&cfg);

    assert_eq!(
        mir.blocks.len(),
        cfg.node_count(),
        "one MIR block per HIR block"
    );
    assert!(
        matches!(mir.blocks[0].term, MirTerm::Return(Some(_))),
        "returns a value"
    );
    // Const(1), Const(2), BinOp(+), DefVar(x), UseVar(x) ⇒ 5 instrs; regs r0..r3.
    assert_eq!(mir.blocks[0].instrs.len(), 5);
    assert_eq!(mir.n_regs, 4);

    // VarId consistency: the `DefVar` and the returned `UseVar` name the same var.
    let def = mir.blocks[0].instrs.iter().find_map(|i| match i {
        MirInstr::DefVar { var, .. } => Some(*var),
        _ => None,
    });
    let used = mir.blocks[0].instrs.iter().find_map(|i| match i {
        MirInstr::UseVar { var, .. } => Some(*var),
        _ => None,
    });
    assert_eq!(
        def, used,
        "def and use must refer to the same VarId (seeded interner)"
    );
}

#[test]
fn temps_carry_real_source_spans() {
    // End-to-end span propagation (lexer → parser → MIR): each temp's recorded
    // span must slice the exact source text of the expression it came from.
    let src = "return y + 100\n";
    let mir = lower_cfg(&Cfg::build(&parse(src).expect("parse error")));
    let spans = &mir.spans.0;

    // The `y` read and the `100` constant each get a fresh reg; find them and
    // confirm their spans point back at the real tokens (not the old `(0, 0)`).
    let const_reg = mir.blocks[0]
        .instrs
        .iter()
        .find_map(|i| match i {
            MirInstr::Const { dest, .. } => Some(dest.0),
            _ => None,
        })
        .expect("a Const temp");
    let (cspan, _) = &spans[&const_reg];
    assert_eq!(
        cspan.syntax, None,
        "MIR spans retain provenance, not semantic IDs"
    );
    assert_eq!(&src[cspan.span.0..cspan.span.1], "100");
    assert_ne!(
        cspan.span,
        (0, 0),
        "spans must be real, not the placeholder"
    );

    let use_reg = mir.blocks[0]
        .instrs
        .iter()
        .find_map(|i| match i {
            MirInstr::UseVar { dest, .. } => Some(dest.0),
            _ => None,
        })
        .expect("a UseVar temp");
    let (uspan, origin) = &spans[&use_reg];
    assert_eq!(&src[uspan.span.0..uspan.span.1], "y");
    assert!(origin.is_some(), "a variable read records its origin VarId");
}

#[test]
fn control_flow_block_count_matches_hir() {
    let cfg = Cfg::build(&parse("if a:\n    var x: Int = 1\nelse:\n    var y: Int = 2\n").unwrap());
    let mir = lower_cfg(&cfg);
    assert_eq!(mir.blocks.len(), cfg.node_count());
    // The entry block ends in a Branch on the flattened condition.
    assert!(matches!(mir.blocks[0].term, MirTerm::Branch { .. }));
}

#[test]
fn nested_calls_flatten_to_temps_in_order() {
    // `f(g(x))` ⇒ UseVar(x); Call g([x]); Call f([g_result]).
    let is = instrs("f(g(x))\n");
    assert_eq!(is.len(), 3);
    assert!(matches!(is[0], MirInstr::UseVar { .. }));
    match (&is[1], &is[2]) {
        (
            MirInstr::Call {
                func: g,
                args: ga,
                dest: gd,
                ..
            },
            MirInstr::Call {
                func: f, args: fa, ..
            },
        ) => {
            assert_eq!(g.0, "g");
            assert_eq!(f.0, "f");
            assert_eq!(ga.len(), 1);
            assert_eq!(
                fa,
                &vec![*gd],
                "outer call takes the inner call's result register"
            );
        }
        other => panic!("expected two Calls, got {other:?}"),
    }
}

#[test]
fn transfer_lowers_to_a_move_use() {
    // `x^` is a move out of the variable.
    let is = instrs("f(x^)\n");
    assert!(is.iter().any(|i| matches!(
        i,
        MirInstr::UseVar {
            mode: mojito::mir::UseMode::Move,
            ..
        }
    )));
}

#[test]
fn member_write_lowers_to_a_store_through_a_place() {
    // `p.x = 1` ⇒ Const(1); Store { place: p.x }.
    let is = instrs("p.x = 1\n");
    let store = is.iter().find_map(|i| match i {
        MirInstr::Store { place, .. } => Some(place),
        _ => None,
    });
    match store {
        Some(MirPlace { proj, .. }) => {
            assert!(matches!(proj.as_slice(), [Proj::Field(f)] if f == "x"));
        }
        None => panic!("expected a Store, got {is:?}"),
    }
}

#[test]
fn checked_mir_places_carry_root_projection_and_storage_types() {
    let program = parse(
        "@fieldwise_init\nstruct Cell:\n    var value: Int\n\ndef main():\n    var cell = Cell(1)\n    cell.value = 2\n",
    )
    .expect("parse");
    let mir = lower_program(&program).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let function = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .unwrap();
    let place = function
        .1
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Store { place, .. } => Some(place),
            _ => None,
        })
        .expect("typed store place");
    assert!(place.is_typed());
    assert!(matches!(place.root_ty, Some(mojito::Ty::Struct(ref name, _)) if name == "Cell"));
    assert_eq!(place.projection_tys, vec![mojito::Ty::Int]);
    assert_eq!(place.ty, Some(mojito::Ty::Int));
}

#[test]
fn index_write_lowers_to_a_store_with_an_index_projection() {
    // `xs[0] = 1` ⇒ the place is `xs[<reg>]`.
    let is = instrs("xs[0] = 1\n");
    let store = is.iter().find_map(|i| match i {
        MirInstr::Store { place, .. } => Some(place),
        _ => None,
    });
    assert!(
        matches!(store.map(|p| p.proj.as_slice()), Some([Proj::Index(_)])),
        "index write should Store through an Index projection, got {is:?}"
    );
}

#[test]
fn checked_indexer_normalization_becomes_explicit_typed_mir_calls() {
    let source = "@fieldwise_init\nstruct Offset(Indexer):\n    var value: Int\n    def __mlir_index__(self) -> Int:\n        return self.value\n\ndef main():\n    var values = [3, 7, 11]\n    values[Offset(1)] = 9\n    print(values[Offset(1)])\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile Indexer subscripts");
    let normalizations = compiled
        .checked()
        .expressions()
        .iter()
        .filter_map(|expression| {
            expression
                .adjustments
                .iter()
                .find_map(|adjustment| match adjustment {
                    SemanticAdjustment::IndexNormalization { target } => Some(target),
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        normalizations,
        ["Offset.__mlir_index__", "Offset.__mlir_index__"]
    );

    let lowered = mojito::mir::lower_checked_program(compiled.checked());
    let main = &lowered
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let calls = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::MethodCall {
                dest,
                method,
                resolved,
                args,
                ..
            } if method == "__mlir_index__" => Some((dest, resolved, args)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "{:#?}", main.blocks);
    assert!(calls.iter().all(|(dest, resolved, args)| {
        main.reg_types.get(&dest.0) == Some(&mojito::Ty::Int)
            && resolved.as_deref() == Some("Offset.__mlir_index__")
            && args.is_empty()
    }));
}

#[test]
fn nested_place_write_stacks_projections() {
    // `p.items[i].x = 1` ⇒ place proj = [Field(items), Index(i), Field(x)].
    let is = instrs("p.items[0].x = 1\n");
    let place = is
        .iter()
        .find_map(|i| match i {
            MirInstr::Store { place, .. } => Some(place),
            _ => None,
        })
        .expect("a Store");
    assert!(
        matches!(
            place.proj.as_slice(),
            [Proj::Field(a), Proj::Index(_), Proj::Field(b)] if a == "items" && b == "x"
        ),
        "got {:?}",
        place.proj
    );
}

#[test]
fn aug_assign_on_a_name_is_read_modify_write() {
    // `x += 1` ⇒ UseVar(x); Const(1); BinOp(+); DefVar(x) — one read, one write.
    let is = instrs("x += 1\n");
    assert_eq!(is.len(), 4);
    assert!(matches!(is[0], MirInstr::UseVar { .. }));
    assert!(matches!(is[3], MirInstr::DefVar { .. }));
    // The read and the write-back name the same variable.
    let read = match is[0] {
        MirInstr::UseVar { var, .. } => var,
        _ => unreachable!(),
    };
    let write = match is[3] {
        MirInstr::DefVar { var, .. } => var,
        _ => unreachable!(),
    };
    assert_eq!(read, write);
}

#[test]
fn aug_assign_through_a_place_loads_and_stores_the_same_place() {
    // `xs[0] += 1` ⇒ one LoadPlace + one Store, both over the SAME place — i.e. the
    // subscript index is flattened once and shared (not re-evaluated for the store).
    let is = instrs("xs[0] += 1\n");
    let idx_reg = |p: &MirPlace| match p.proj.as_slice() {
        [Proj::Index(r)] => *r,
        other => panic!("expected a single Index projection, got {other:?}"),
    };
    let loaded = is.iter().find_map(|i| match i {
        MirInstr::LoadPlace { place, .. } => Some(idx_reg(place)),
        _ => None,
    });
    let stored = is.iter().find_map(|i| match i {
        MirInstr::Store { place, .. } => Some(idx_reg(place)),
        _ => None,
    });
    assert!(
        loaded.is_some() && loaded == stored,
        "load and store share one index reg: {is:?}"
    );
}

#[test]
fn program_driver_makes_a_function_per_def_and_method() {
    let program = parse(
        "def f() -> Int:\n    return 1\n\n@fieldwise_init\nstruct P:\n    var x: Int\n    def get(self) -> Int:\n        return self.x\n\nvar top: Int = 0\n",
    )
    .unwrap();
    let mir = lower_program(&program).expect("type error");
    let names: Vec<&str> = mir.functions.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"f"), "a def becomes a function: {names:?}");
    assert!(
        names.contains(&"P.get"),
        "a method becomes Struct.method: {names:?}"
    );
    assert!(
        names.contains(&"__toplevel__"),
        "top-level stmts collect into __toplevel__: {names:?}"
    );
}

#[test]
fn checked_lowering_owns_typed_declarations_and_normalized_defaults() {
    let program = parse("def f(x: UInt = 3):\n    pass\n").expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let declaration = mir
        .declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == "f")
        .unwrap();
    assert_eq!(declaration.param_types, vec![mojito::Ty::UInt]);
    assert!(matches!(
        declaration.defaults.as_slice(),
        [Some(mojito::CheckedConst::Int(value))] if value.to_i64() == Some(3)
    ));
}

#[test]
fn executable_mir_carries_checked_binding_and_parameter_types() {
    let program =
        parse("def widen(x: UInt):\n    var y: Float64 = 3\n    print(x, y)\n").expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let function = mir
        .functions
        .iter()
        .find(|(name, _)| name == "widen")
        .expect("lowered function");

    assert_eq!(function.1.param_types, vec![mojito::Ty::UInt]);
    assert!(
        function
            .1
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::DefVar {
                    binding_ty: Some(mojito::Ty::Float64),
                    ..
                }
            ))
    );
}

#[test]
fn literal_materialization_is_explicit_in_typed_mir() {
    let program =
        parse("def main():\n    var value: Int = 18446744073709551623\n    print(value)\n")
            .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let instructions = main.blocks.iter().flat_map(|block| &block.instrs);
    assert!(instructions.clone().any(|instruction| matches!(
        instruction,
        MirInstr::Const {
            k: mojito::mir::Const::IntLiteral(value),
            ..
        } if value.to_string() == "18446744073709551623"
    )));
    assert!(instructions.clone().any(|instruction| matches!(
        instruction,
        MirInstr::MaterializeLiteral {
            target: mojito::Ty::Int,
            ..
        }
    )));
}

#[test]
fn production_tstrings_construct_the_lazy_specialization() {
    // Through the whole-program Compiler, a `t"…"` desugars into the concrete
    // `TString` specialization's construction — no eager `""+String(x)+…`
    // concatenation chain remains in the lowered main.
    let source = "def main():\n    var value: Int = 42\n    print(t\"answer={value}\")\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile the t-string program");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let instrs = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(
        instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. } if func.0.starts_with("TString$")
        )),
        "expected a TString specialization construction in main"
    );
    assert!(
        !instrs.iter().any(|instruction| matches!(
            instruction,
            MirInstr::Call { func, .. } if func.0 == "String"
        )),
        "the interpolation must be captured, not eagerly stringified"
    );
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn seam_tstrings_keep_the_eager_conversion_fallback() {
    // The stage-composed seam (raw parse -> check, no discovery pass) retains
    // the output-identical eager concatenation lowering for t-strings.
    let program = parse("def main():\n    var value: Int = 42\n    print(t\"answer={value}\")\n")
        .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let conversion = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Call { dest, func, .. } if func.0 == "String" => Some(*dest),
            _ => None,
        })
        .expect("interpolation conversion");
    assert_eq!(
        main.reg_types.get(&conversion.0),
        Some(&mojito::Ty::StringLiteral)
    );
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn inferred_generic_method_calls_retain_parameter_declarations() {
    let source = "@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def size[T: Copyable & Movable](self, **options: T) -> Int:\n        return self.bias + len(options)\n\ndef main():\n    var counter = Counter(10)\n    print(counter.size(left=1, right=2))\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile generic method");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let declaration = mir
        .declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == "Counter.size")
        .expect("generic method declaration");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let call_decls = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                resolved: Some(target),
                param_decls,
                ..
            } if target == "Counter.size" => Some(param_decls),
            _ => None,
        })
        .expect("selected generic method call");
    assert_eq!(call_decls, &declaration.param_decls);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn collection_and_range_literal_boundaries_are_explicit_in_typed_mir() {
    let program = parse(
        "def main():\n    var xs = List[Int](18446744073709551616)\n    var unique = Set[Int](18446744073709551616)\n    var table: Dict[Int, Int] = {18446744073709551616: 18446744073709551616}\n    for i in range(18446744073709551616):\n        pass\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let materializations = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .filter(|instruction| {
            matches!(
                instruction,
                MirInstr::MaterializeLiteral {
                    target: mojito::Ty::Int,
                    ..
                }
            )
        })
        .count();
    assert_eq!(materializations, 5);
}

#[test]
fn comprehension_binders_receive_distinct_mir_slots_from_outer_locals() {
    let program = parse(
        "def main():\n    var x = 100\n    var result = [x for x in range(2) for x in range(x + 1)]\n    print(x, result)\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let function = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main function");
    assert!(function.1.var_names.iter().any(|name| name == "x"));
    let binders = function
        .1
        .var_names
        .iter()
        .filter(|name| name.starts_with("$compx$"))
        .collect::<Vec<_>>();
    assert_eq!(binders.len(), 2, "{:?}", function.1.var_names);
    assert_ne!(binders[0], binders[1]);
}

#[test]
fn bounded_trait_calls_carry_the_requirement_error_contract_into_mir() {
    let program = parse(
        "trait Fallible:\n    def run(self) raises -> Int: ...\n\ndef invoke[T: Fallible](value: T) raises -> Int:\n    return value.run()\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let invoke = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "invoke")
        .expect("invoke function")
        .1;

    assert!(
        invoke
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::MethodCall {
                    method,
                    raises: Some(mojito::Ty::Error),
                    ..
                } if method == "run"
            ))
    );
}

#[test]
fn checked_declaration_types_are_keyed_by_source_site_not_type_syntax() {
    let program = parse(
        "def keep_any[T: AnyType](x: T):\n    pass\n\
         def keep_hashable[T: Hashable](x: T):\n    pass\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);

    let param_type = |name: &str| {
        mir.declarations
            .functions
            .iter()
            .find(|declaration| declaration.lowered_name.starts_with(name))
            .expect("function declaration")
            .param_types[0]
            .clone()
    };
    assert_eq!(
        param_type("keep_any"),
        mojito::Ty::Param {
            name: "T".into(),
            bounds: vec!["AnyType".into()],
            callable_bound: None,
        }
    );
    assert_eq!(
        param_type("keep_hashable"),
        mojito::Ty::Param {
            name: "T".into(),
            bounds: vec!["Hashable".into()],
            callable_bound: None,
        }
    );
}

#[test]
fn mir_declarations_carry_generic_free_and_method_keyword_collectors() {
    let program = parse(
        "def collect[T: Copyable & Movable](**options: T):\n    pass\n\nstruct Relay:\n    def collect[T: Copyable & Movable](self, **options: T):\n        pass\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let collector = |name: &str| {
        mir.declarations
            .functions
            .iter()
            .find(|declaration| declaration.lowered_name == name)
            .expect("keyword collector declaration")
    };

    for declaration in [collector("collect"), collector("Relay.collect")] {
        assert_eq!(declaration.kw_variadic_index, Some(0));
        assert_eq!(
            declaration.kw_variadic,
            Some(mojito::Ty::Param {
                name: "T".into(),
                bounds: vec!["Copyable".into(), "Movable".into()],
                callable_bound: None,
            })
        );
    }

    let collector_body = |name: &str| {
        &mir.functions
            .iter()
            .find(|(candidate, _)| candidate == name)
            .expect("keyword collector body")
            .1
    };
    let element = mojito::Ty::Param {
        name: "T".into(),
        bounds: vec!["Copyable".into(), "Movable".into()],
        callable_bound: None,
    };
    let body_type =
        mojito::Ty::Struct("StringDict".into(), vec![mojito::types::TyArg::Ty(element)]);
    for function in [collector_body("collect"), collector_body("Relay.collect")] {
        assert_eq!(function.param_types.last(), Some(&body_type));
        let collector_slot = function.n_params - 1;
        assert_eq!(
            function.var_tys.get(&(collector_slot as u32)),
            Some(&body_type)
        );
    }
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn compatibility_lowering_propagates_checker_errors() {
    let program = parse("def bad() -> Int:\n    return missing\n").expect("parse");
    assert!(matches!(
        lower_program(&program),
        Err(mojito::TypeError::UndefinedVariable(name)) if name == "missing"
    ));
}

#[test]
fn member_read_lowers_to_a_place_load() {
    // A pure field chain read (`p.a`) lowers to a `LoadPlace` (a place read), so
    // the ownership analysis sees which field is read (field-sensitivity). A
    // member of a temporary keeps the register-based `GetField`.
    let is = instrs("var p: Foo = mk()\nreturn p.a\n");
    let place = is.iter().find_map(|i| match i {
        MirInstr::LoadPlace { place, .. } => Some(place),
        _ => None,
    });
    match place {
        Some(MirPlace { proj, .. }) => {
            assert!(
                matches!(proj.as_slice(), [Proj::Field(f)] if f == "a"),
                "got {proj:?}"
            );
        }
        None => panic!("member read should be a LoadPlace, got {is:?}"),
    }
    assert!(
        !is.iter().any(|i| matches!(i, MirInstr::GetField { .. })),
        "a pure field read should not use GetField"
    );
}

#[test]
fn partial_move_lowers_to_a_move_place() {
    // `p.a^` (a pure field chain transfer) lowers to a `MovePlace` over that
    // field, distinct from a whole-variable `UseVar { Move }`.
    let is = instrs("var p: Foo = mk()\nvar x: Bar = p.a^\n");
    let place = is.iter().find_map(|i| match i {
        MirInstr::MovePlace { place, .. } => Some(place),
        _ => None,
    });
    match place {
        Some(MirPlace { proj, .. }) => {
            assert!(
                matches!(proj.as_slice(), [Proj::Field(f)] if f == "a"),
                "got {proj:?}"
            );
        }
        None => panic!("partial move should be a MovePlace, got {is:?}"),
    }
}

#[test]
fn break_crossing_try_lowers_to_escape_jump() {
    use mojito::mir::{MirInstr, MirTerm, lower_program};
    // `break` inside a `try` in a `for` loop lowers to a `MirTerm::EscapeJump` in
    // the try's body region — not a `MirInstr::Unsupported`.
    let src = "def main():\n    for i in range(3):\n        try:\n            break\n        finally:\n            print(i)\n";
    let prog = lower_program(&parse(src).expect("parse"));
    let prog = prog.expect("type error");
    let (_, main) = prog
        .functions
        .iter()
        .find(|(n, _)| n == "main")
        .expect("main");

    let mut found_escape = false;
    let mut found_unsupported = false;
    for b in &main.blocks {
        for instr in &b.instrs {
            match instr {
                MirInstr::Unsupported(_) => found_unsupported = true,
                MirInstr::Try { body, .. } => {
                    for rb in body {
                        if matches!(rb.term, MirTerm::EscapeJump { .. }) {
                            found_escape = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    assert!(
        found_escape,
        "break in the try body should lower to an EscapeJump"
    );
    assert!(
        !found_unsupported,
        "a function-level try escape must not be Unsupported"
    );
}

#[test]
fn pointer_construction_lowers_to_a_handle_and_owner_loan() {
    use mojito::check_program;
    let src = "def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    print(p[0])\n";
    let program = parse(src).expect("parse");
    let checked = check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let instrs: Vec<&MirInstr> = main
        .blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .collect();
    assert!(
        instrs
            .iter()
            .any(|instr| matches!(instr, MirInstr::MakeRef { .. })),
        "construction emits a frame/slot handle"
    );
    assert!(
        instrs.iter().any(|instr| matches!(instr,
            MirInstr::EstablishLoans { loans, .. }
                if loans.iter().any(|loan| loan.mutable && loan.interior.is_none())
        )),
        "the pointer binding carries a mutable owner loan"
    );
    assert!(
        instrs.iter().any(|instr| matches!(
            instr,
            MirInstr::LoadPlace { place, .. } if place.through.is_some()
        )),
        "a stable pointer deref substitutes the owner place through the loan"
    );
}

#[test]
fn list_element_references_lower_with_interior_generation_metadata() {
    use mojito::{OriginSeg, check_program};

    let src = "def main():\n    var values = [10, 20, 30]\n    ref first = values[0]\n    values.append(4)\n    print(first)\n";
    let program = parse(src).expect("parse");
    let checked = check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let instrs: Vec<&MirInstr> = main
        .blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .collect();

    let (establish_index, reference, owner) = instrs
        .iter()
        .enumerate()
        .find_map(|(index, instruction)| match instruction {
            MirInstr::EstablishLoans {
                reference, loans, ..
            } => loans.iter().find_map(|loan| {
                let origin = loan.interior.as_ref()?;
                origin
                    .path
                    .iter()
                    .any(
                        |segment| matches!(segment, OriginSeg::Interior(name) if name == "element"),
                    )
                    .then_some((index, *reference, origin.root))
            }),
            _ => None,
        })
        .expect("the list subscript establishes an element-interior generation");

    let invalidate_index = instrs
        .iter()
        .enumerate()
        .find_map(|(index, instruction)| match instruction {
            MirInstr::InvalidateInteriors { base, .. } if base.root == owner => Some(index),
            _ => None,
        })
        .expect("append invalidates the list's interior generations");

    let use_index = instrs
        .iter()
        .enumerate()
        .find_map(|(index, instruction)| match instruction {
            MirInstr::LoadPlace { place, .. } if place.through == Some(reference) => Some(index),
            _ => None,
        })
        .expect("the final print reads through the local reference");

    assert!(
        establish_index < invalidate_index && invalidate_index < use_index,
        "the element generation must be established, invalidated by append, then observed by the stale use: {instrs:#?}"
    );
}

#[test]
fn reference_iteration_binding_reestablishes_the_source_loans() {
    // A `BorrowReference` loop binding aliases the borrowed source through the
    // yielded handle, so the iterator's source loans re-establish on the
    // binding itself: an invalidation then names the user's variable rather
    // than only the compiler's iterator slot.
    let source = include_str!("../assets/ok/reference_yielding_iteration_named_source.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile reference-yielding iteration");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let nums = main
        .var_names
        .iter()
        .position(|name| name == "nums")
        .expect("nums slot") as u32;
    let binding = main
        .var_names
        .iter()
        .position(|name| name == "x")
        .expect("loop binding slot") as u32;
    let has_source_loan = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference, loans, .. }
                    if *reference == binding
                        && loans.iter().any(|loan| loan.place.root == nums)
            )
        });
    assert!(has_source_loan, "loop binding carries the source loan");
}

#[test]
fn nested_call_transfer_installs_loans_on_the_carrier() {
    // A nested `def` storing its owned parameter into a `mut` carrier lowers
    // its direct call through `CallIndirect`; the call site still installs
    // the transferred loan on the carrier's root after the call, exactly
    // like the direct free-call path.
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef main():\n    var sink: List[RefBox] = List[RefBox]()\n    var local = [9]\n    ref alias = local\n    def stash(mut s: List[RefBox], box: RefBox):\n        s.append(box^)\n    stash(sink, RefBox(alias))\n    print(sink[0].value[0])\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile nested transfer call");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let sink = main
        .var_names
        .iter()
        .position(|name| name == "sink")
        .expect("sink slot") as u32;
    let local = main
        .var_names
        .iter()
        .position(|name| name == "local")
        .expect("local slot") as u32;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    let call = instructions
        .iter()
        .position(|instruction| matches!(instruction, MirInstr::CallIndirect { .. }))
        .expect("nested call lowers to CallIndirect");
    let install = instructions
        .iter()
        .position(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference, loans, .. }
                    if *reference == sink && loans.iter().any(|loan| loan.place.root == local)
            )
        })
        .expect("nested call installs the transferred loan on the carrier");
    assert!(install > call, "the loan is installed after the call");
}

#[test]
fn captured_owner_transfer_installs_loans_in_the_owning_frame() {
    // A closure storing into a CAPTURED owner records a `Bound`-destination
    // effect; invoking it in the frame that owns the storage resolves the
    // owner through the lowering's owner-variable map and installs the
    // transferred loan there.
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Carrier:\n    var slot: RefBox\n\ndef main():\n    var keep = [1]\n    ref whole = keep\n    var sink = Carrier(RefBox(whole))\n    var local = [9]\n    def push() {mut sink, mut local}:\n        ref alias = local\n        sink.slot = RefBox(alias)\n    push()\n    print(len(keep))\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile captured-owner transfer");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let sink = main
        .var_names
        .iter()
        .position(|name| name == "sink")
        .expect("sink slot") as u32;
    let local = main
        .var_names
        .iter()
        .position(|name| name == "local")
        .expect("local slot") as u32;
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::EstablishLoans { reference, loans, .. }
                    if *reference == sink && loans.iter().any(|loan| loan.place.root == local)
            )),
        "the captured owner's frame installs the transferred loan"
    );
}

#[test]
fn interior_destination_transfers_carry_their_domain() {
    // A transfer whose destination projects below the actual's root records
    // the interior path; lowering installs the generation with that domain,
    // so rebinding the exact field later releases it (sibling domains and
    // the root generation stay).
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Two:\n    var a: List[RefBox]\n    var b: List[Int]\n\ndef main():\n    var a: List[RefBox] = List[RefBox]()\n    var t = Two(a^, [1])\n    var local = [9]\n    ref alias = local\n    t.a.append(RefBox(alias))\n    print(t.b[0])\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile interior-destination transfer");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let t = main
        .var_names
        .iter()
        .position(|name| name == "t")
        .expect("t slot") as u32;
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::EstablishLoans {
                    reference,
                    dest_interior: Some(domain),
                    ..
                } if *reference == t
                    && domain.root == t
                    && matches!(
                        domain.path.as_slice(),
                        [mojito::OriginSeg::Field(field)] if field == "a"
                    )
            )),
        "the transferred generation carries its interior destination domain"
    );
}

#[test]
fn nested_call_transfer_to_an_enclosing_parameter_defers_to_the_caller() {
    // A transfer destination rooted at the enclosing function's own parameter
    // is not installed locally — the derived transitive effect installs it at
    // the caller, where the storage actually lives.
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef outer(mut sink: List[RefBox], box: RefBox):\n    def stash(mut s: List[RefBox], b: RefBox):\n        s.append(b^)\n    stash(sink, box)\n\ndef main():\n    var sink: List[RefBox] = List[RefBox]()\n    var local = [9]\n    ref alias = local\n    outer(sink, RefBox(alias))\n    print(sink[0].value[0])\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile enclosing-parameter transfer");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, outer) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "outer")
        .expect("outer lowered");
    let outer_sink = outer
        .var_names
        .iter()
        .position(|name| name == "sink")
        .expect("outer sink slot") as u32;
    assert!(
        !outer
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::EstablishLoans { reference, .. } if *reference == outer_sink
            )),
        "a parameter-rooted destination installs nothing locally"
    );
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let sink = main
        .var_names
        .iter()
        .position(|name| name == "sink")
        .expect("sink slot") as u32;
    let local = main
        .var_names
        .iter()
        .position(|name| name == "local")
        .expect("local slot") as u32;
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::EstablishLoans { reference, loans, .. }
                    if *reference == sink && loans.iter().any(|loan| loan.place.root == local)
            )),
        "the derived transitive effect installs the loan at the caller"
    );
}

#[test]
fn borrowed_list_iteration_lowers_a_reference_bind_and_interior_loan() {
    use mojito::OriginSeg;

    let source = include_str!("../conformance/fixtures/borrowed_iteration_mutation.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile borrowed List iteration");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let values = main
        .var_names
        .iter()
        .position(|name| name == "values")
        .expect("values slot") as u32;
    let source_slot = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$iter") && !name.starts_with("$iterobj"))
        .expect("retained-source slot") as u32;
    let iterator = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$iterobj"))
        .expect("iterator-object slot") as u32;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();

    // The retained source is a genuine reference to the whole collection place
    // (never a value copy); interior granularity lives only in the loan.
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::MakeRef { place, .. } if place.root == values && place.proj.is_empty()
    )));
    assert!(matches!(main.var_tys.get(&source_slot), Some(Ty::Ref(_))));
    let interior_loan = |reference: u32| {
        instructions.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference: r, loans, .. }
                    if *r == reference
                        && loans.iter().any(|loan| {
                            loan.place.root == values
                                && loan.interior.as_ref().is_some_and(|origin| {
                                    origin.path.iter().any(|segment| matches!(
                                        segment,
                                        OriginSeg::Interior(name) if name == "element"
                                    ))
                                })
                        })
            )
        })
    };
    // The loan is established on the retained-source slot and re-established on
    // the long-lived iterator-object slot.
    assert!(interior_loan(source_slot));
    assert!(interior_loan(iterator));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::InvalidateInteriors { base, .. }
            if base.root == values && base.path == [OriginSeg::AnyIndex]
    )));
    assert!(!instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::InvalidateInteriors { base, .. }
            if base.root == values && base.path.is_empty()
    )));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn borrowed_named_user_source_lowers_a_reference_bind_and_whole_place_loan() {
    let source = include_str!("../assets/ok/reference_yielding_iteration_named_source.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile borrowed named user-source iteration");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let nums = main
        .var_names
        .iter()
        .position(|name| name == "nums")
        .expect("nums slot") as u32;
    let source_slot = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$iter") && !name.starts_with("$iterobj"))
        .expect("retained-source slot") as u32;
    let iterator = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$iterobj"))
        .expect("iterator-object slot") as u32;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();

    // Uniform shape with the collection case: `MakeRef` into a `Ty::Ref`-typed
    // retained-source slot; the only difference is the loan granularity — a
    // whole-place shared loan (`interior: None`) instead of an element
    // generation.
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::MakeRef { place, .. } if place.root == nums && place.proj.is_empty()
    )));
    assert!(matches!(main.var_tys.get(&source_slot), Some(Ty::Ref(_))));
    let whole_place_loan = |reference: u32| {
        instructions.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference: r, loans, .. }
                    if *r == reference
                        && loans.iter().any(|loan| {
                            loan.place.root == nums
                                && loan.place.proj.is_empty()
                                && loan.interior.is_none()
                        })
            )
        })
    };
    assert!(whole_place_loan(source_slot));
    assert!(whole_place_loan(iterator));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn borrowed_comprehension_sources_lower_like_statement_loops() {
    use mojito::OriginSeg;

    // Interior granularity: a comprehension over a named List binds the same
    // `MakeRef` retained-source slot and `element` interior loan as a `for`
    // statement, re-established on the iterator-object slot.
    let source = "def main():\n    var values = [1, 2, 3]\n    var doubled = [x * 2 for x in values]\n    print(len(doubled))\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile borrowed List comprehension");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let values = main
        .var_names
        .iter()
        .position(|name| name == "values")
        .expect("values slot") as u32;
    let source_slot = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$compiter") && !name.starts_with("$compiterobj"))
        .expect("retained-source slot") as u32;
    let iterator = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$compiterobj"))
        .expect("iterator-object slot") as u32;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::MakeRef { place, .. } if place.root == values && place.proj.is_empty()
    )));
    assert!(matches!(main.var_tys.get(&source_slot), Some(Ty::Ref(_))));
    let interior_loan = |reference: u32| {
        instructions.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference: r, loans, .. }
                    if *r == reference
                        && loans.iter().any(|loan| {
                            loan.place.root == values
                                && loan.interior.as_ref().is_some_and(|origin| {
                                    origin.path.iter().any(|segment| matches!(
                                        segment,
                                        OriginSeg::Interior(name) if name == "element"
                                    ))
                                })
                        })
            )
        })
    };
    assert!(interior_loan(source_slot));
    assert!(interior_loan(iterator));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );

    // Whole-place granularity: a comprehension over a named user iterable
    // borrows the whole source (`interior: None`) instead of copying it.
    let source = include_str!("../assets/ok/comprehension_borrowed_named_source.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile borrowed named-source comprehension");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let nums = main
        .var_names
        .iter()
        .position(|name| name == "nums")
        .expect("nums slot") as u32;
    let source_slot = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$compiter") && !name.starts_with("$compiterobj"))
        .expect("retained-source slot") as u32;
    let iterator = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$compiterobj"))
        .expect("iterator-object slot") as u32;
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::MakeRef { place, .. } if place.root == nums && place.proj.is_empty()
    )));
    assert!(matches!(main.var_tys.get(&source_slot), Some(Ty::Ref(_))));
    let whole_place_loan = |reference: u32| {
        instructions.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::EstablishLoans { reference: r, loans, .. }
                    if *r == reference
                        && loans.iter().any(|loan| {
                            loan.place.root == nums
                                && loan.place.proj.is_empty()
                                && loan.interior.is_none()
                        })
            )
        })
    };
    assert!(whole_place_loan(source_slot));
    assert!(whole_place_loan(iterator));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn reference_list_iteration_lowers_through_the_generic_protocol() {
    use mojito::OriginSeg;

    // `for ref value in values` over a bundled List runs the ordinary
    // reference-yielding protocol: a mutable `ref` binding fed by
    // `_ListIter.__next__`'s reference result, with the `element` interior
    // loan re-established on the binding — no synthesized `__len__`/`Index`
    // accessor pair remains.
    let source = include_str!("../conformance/fixtures/reference_iteration.mojo");
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(source, Path::new("mir_test.mojo"))
        .expect("compile List reference iteration");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let value = main
        .var_names
        .iter()
        .position(|name| name == "value")
        .expect("reference loop binding") as u32;
    match main.var_tys.get(&value) {
        Some(Ty::Ref(reference)) => {
            assert_eq!(reference.mutability, mojito::Mutability::Mutable);
        }
        other => panic!("loop binding is a mutable reference, got {other:?}"),
    }

    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::TryNext { call, .. }
            if call.target.contains("__next__")
                && call
                    .reference_result
                    .as_ref()
                    .is_some_and(|reference| reference.mutability == mojito::Mutability::Mutable)
    )));
    assert!(
        !main
            .var_names
            .iter()
            .any(|name| name.starts_with("$refindex")),
        "the bridge's synthesized index counter is gone"
    );
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::EstablishLoans { reference, loans, .. }
            if *reference == value
                && loans.iter().any(|loan| loan.interior.as_ref().is_some_and(|origin| {
                    origin.path.iter().any(|segment| matches!(
                        segment,
                        OriginSeg::Interior(name) if name == "element"
                    ))
                }))
    )));
}

#[test]
fn ref_returning_subscript_retains_a_typed_receiver_place() {
    let src = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef main():\n    var box = Box(40)\n    print(box[0])\n";
    let checked = mojito::check_program(&parse(src).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let (receiver, call) = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Index {
                base_place: Some(place),
                call: Some(call),
                ..
            } => Some((place, call)),
            _ => None,
        })
        .expect("nominal subscript retained its receiver place");
    assert!(receiver.is_typed(), "receiver place must cross typed MIR");
    assert_eq!(call.target, "Box.__getitem__");
    assert!(call.receiver_requires_place);
    assert!(call.reference_result.is_some());
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn chained_reference_subscript_materializes_a_retained_handle() {
    let src = "@fieldwise_init\nstruct Item(Copyable, Movable):\n    var value: Int\n    def bump(mut self):\n        self.value += 1\n\n@fieldwise_init\nstruct Shelf(Copyable, Movable):\n    var item: Item\n    def __getitem__(\n        ref self, index: Int\n    ) -> ref[origin_of(self.item)] Item:\n        return self.item\n\ndef main():\n    var shelf = Shelf(Item(41))\n    shelf[0].bump()\n    print(shelf.item.value)\n";
    let checked = mojito::check_program(&parse(src).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let reference_slot = main
        .var_names
        .iter()
        .position(|name| name.starts_with("$call_ref"))
        .expect("synthetic subscript reference slot") as u32;
    assert!(matches!(
        main.var_tys.get(&reference_slot),
        Some(mojito::Ty::Ref(_))
    ));
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::EstablishLoans { reference, .. } if *reference == reference_slot
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        MirInstr::MethodCall {
            method,
            recv_place: Some(place),
            ..
        } if method == "bump"
            && place.root == reference_slot
            && place.through == Some(reference_slot)
    )));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn reference_valued_subscript_receiver_materializes_the_peeled_handle_type() {
    let src = "@fieldwise_init\nstruct Item(Copyable, Movable):\n    var value: Int\n    def bump(mut self):\n        self.value += 2\n\n@fieldwise_init\nstruct RefList[origin: Origin[mut=True]]:\n    var values: List[ref[origin] Item]\n\ndef main():\n    var item = Item(40)\n    ref alias = item\n    var refs = RefList([alias])\n    refs.values[0].bump()\n";
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(src, Path::new("mir_test.mojo"))
        .expect("compile");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let receiver_place = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MethodCall {
                method,
                recv_place: Some(place),
                ..
            } if method == "bump" => Some(place),
            _ => None,
        })
        .expect("chained bump retains the accessor handle");
    assert_eq!(
        receiver_place.through,
        Some(receiver_place.root),
        "{:#?}",
        main.blocks
    );
    assert!(matches!(
        main.var_tys.get(&receiver_place.root),
        Some(Ty::Ref(reference))
            if matches!(reference.referent.as_ref(), Ty::Struct(name, _) if name == "Item")
    ));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn dynamic_reference_returning_call_actual_retains_one_hidden_place() {
    let src = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef index() -> Int:\n    return 0\n\ndef bump(mut value: Int):\n    value += 2\n\ndef main():\n    var box = Box(40)\n    bump(box[index()])\n";
    let checked = mojito::check_program(&parse(src).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| {
                matches!(instruction, MirInstr::Call { func, .. } if func.0 == "index")
            })
            .count(),
        1,
        "the dynamic index expression must be evaluated once"
    );
    let retained = instructions
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::Call {
                func, arg_places, ..
            } if func.0 == "bump" => arg_places.first().and_then(Option::as_ref),
            _ => None,
        })
        .expect("mut actual retains a caller place");
    assert_eq!(retained.through, Some(retained.root));
    assert!(matches!(main.var_tys.get(&retained.root), Some(Ty::Ref(_))));
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn nominal_setter_mir_orders_receiver_index_and_rhs_evaluation() {
    let src = "@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n    def __setitem__(mut self, index: Int, value: Int):\n        self.value = value + index\n\n@fieldwise_init\nstruct Outer(Copyable, Movable):\n    var box: Box\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.box)] Box:\n        return self.box\n\ndef rhs() -> Int:\n    return 40\n\ndef receiver_index() -> Int:\n    return 0\n\ndef index() -> Int:\n    return 2\n\ndef main():\n    var outer = Outer(Box(0))\n    outer[receiver_index()][index()] = rhs()\n";
    let checked = mojito::check_program(&parse(src).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");
    let instructions = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();
    let call_position = |name: &str| {
        instructions
            .iter()
            .position(
                |instruction| matches!(instruction, MirInstr::Call { func, .. } if func.0 == name),
            )
            .unwrap_or_else(|| panic!("call to {name} lowered"))
    };
    let rhs = call_position("rhs");
    let receiver = call_position("receiver_index");
    let index = call_position("index");
    let setter = instructions
        .iter()
        .position(|instruction| matches!(instruction, MirInstr::MultiSet { .. }))
        .expect("nominal setter lowered");
    assert!(receiver < index && index < rhs && rhs < setter);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
}

#[test]
fn union_interior_return_keeps_every_possible_generation() {
    use mojito::{OriginSeg, check_program};

    let src = "def choose(ref left: List[Int], ref right: List[Int], flag: Bool) -> ref[origin_of(left)._get_owned_interior[\"element\"], origin_of(right)._get_owned_interior[\"element\"]] Int:\n    if flag:\n        return left[0]\n    return right[0]\n\ndef main():\n    var left = [1]\n    var right = [2]\n    ref selected = choose(left, right, True)\n    print(len(left), len(right))\n    print(selected)\n";
    let program = parse(src).expect("parse");
    let checked = check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let (_, main) = mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered");

    let loans = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::EstablishLoans {
                reference, loans, ..
            } if main.var_names[*reference as usize] == "selected" => Some(loans),
            _ => None,
        })
        .expect("the union-valued reference establishes a grouped generation");
    let mut roots: Vec<_> = loans
        .iter()
        .filter_map(|loan| {
            let origin = loan.interior.as_ref()?;
            origin
                .path
                .iter()
                .any(|segment| matches!(segment, OriginSeg::Interior(name) if name == "element"))
                .then_some(origin.root)
        })
        .collect();
    roots.sort_unstable();
    roots.dedup();
    assert_eq!(roots.len(), 2, "both union members must survive lowering");
}

#[test]
fn checked_lowering_records_declaration_contracts() {
    use mojito::{Ty, check_program};
    let src = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def get(ref self) -> ref[self] Int:\n        return self.value\n\ndef plain() -> Int:\n    return 1\n\ndef failing() raises:\n    raise Error(\"boom\")\n\ndef main():\n    print(plain())\n    try:\n        failing()\n    except err:\n        print(err)\n";
    let program = parse(src).expect("parse");
    let checked = check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let function = |name: &str| {
        &mir.functions
            .iter()
            .find(|(candidate, _)| candidate == name)
            .unwrap_or_else(|| panic!("function '{name}' lowered"))
            .1
    };
    assert_eq!(function("main").ret_ty, Some(Ty::None));
    assert!(!function("main").raises);
    assert_eq!(function("plain").ret_ty, Some(Ty::Int));
    assert!(function("failing").raises);
    assert_eq!(function("__toplevel__").ret_ty, Some(Ty::None));
    // The ref-returning method records the checked fact without reading
    // source return syntax.
    assert!(function("Box.get").returns_reference);
    let declaration = |name: &str| {
        mir.declarations
            .functions
            .iter()
            .find(|declaration| declaration.lowered_name == name)
            .unwrap_or_else(|| panic!("declaration '{name}' recorded"))
    };
    assert_eq!(declaration("plain").ret_ty, Ty::Int);
    assert!(declaration("failing").raises);
    assert!(!declaration("plain").raises);
}

#[test]
fn recursively_lifted_declarations_preserve_checked_call_abis() {
    use mojito::Ty;

    let src = "def outer() raises -> Int:\n    var base = 40\n    def middle() raises {base} -> Int:\n        def inner(head: Int, /, tail: Int = 0, *, bump: Int = 0) raises {base} -> Int:\n            return base + head + tail + bump\n        return inner(1, bump=1)\n    return middle()\n\ndef main():\n    try:\n        print(outer())\n    except error:\n        print(error)\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let declaration = mir
        .declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == "outer$middle$inner")
        .expect("deep declaration");
    assert_eq!(declaration.param_names, ["base", "head", "tail", "bump"]);
    assert_eq!(declaration.param_types.len(), 4);
    assert_eq!(declaration.defaults.len(), 4);
    assert!(declaration.defaults[2].is_some());
    assert!(declaration.defaults[3].is_some());
    assert_eq!(declaration.required, [true, true, false, false]);
    assert_eq!(declaration.positional_only, Some(2));
    assert_eq!(declaration.keyword_only, Some(3));
    assert!(declaration.raises);
    assert_eq!(declaration.ret_ty, Ty::Int);
    assert_eq!(declaration.ref_params, [true, false, false, false]);

    let function = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "outer$middle$inner")
        .expect("deep function")
        .1;
    assert!(function.raises);
    assert_eq!(function.ret_ty, Some(Ty::Int));
}

#[test]
fn recursively_lifted_reference_return_keeps_its_checked_contract() {
    use mojito::Ty;

    let src = "def main():\n    var value = 40\n    def middle(mut middle_item: Int):\n        def inner(ref inner_item: Int) -> ref[inner_item] Int:\n            return inner_item\n        ref alias = inner(middle_item)\n        alias += 2\n    middle(value)\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let function = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main$middle$inner")
        .expect("deep reference-returning function")
        .1;
    assert!(function.returns_reference);
    assert_eq!(function.ret_ty, Some(Ty::Int));
    assert_eq!(function.ref_params, [true]);

    let declaration = mir
        .declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == "main$middle$inner")
        .expect("deep reference-returning declaration");
    assert_eq!(declaration.ret_ty, Ty::Int);
    assert_eq!(declaration.ref_params, [true]);
}

#[test]
fn nested_def_statement_materializes_explicit_capture_modes_once() {
    use mojito::mir::{MirCaptureMode, UseMode};

    let src = "def main():\n    var copied = 40\n    var moved = [40]\n    def snapshot() {var copied} -> Int:\n        return copied\n    def take() {var moved^} -> Int:\n        return moved[0]\n    print(snapshot(), take())\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let instructions: Vec<_> = main.blocks.iter().flat_map(|block| &block.instrs).collect();

    let closure = |function: &str| {
        instructions
            .iter()
            .enumerate()
            .find_map(|(index, instruction)| match instruction {
                MirInstr::MakeClosure {
                    function: target,
                    captures,
                    ..
                } if target == function => Some((index, captures)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("closure '{function}' materialized"))
    };
    let (copy_index, copy_captures) = closure("main$snapshot");
    let (move_index, move_captures) = closure("main$take");
    assert_eq!(copy_captures[0].mode, MirCaptureMode::Copy);
    assert_eq!(move_captures[0].mode, MirCaptureMode::Move);

    for (name, make_index) in [("snapshot", copy_index), ("take", move_index)] {
        let slot = main
            .var_names
            .iter()
            .position(|candidate| candidate == name)
            .expect("closure slot") as u32;
        let def_index = instructions
            .iter()
            .position(
                |instruction| matches!(instruction, MirInstr::DefVar { var, .. } if *var == slot),
            )
            .expect("declaration stores closure");
        assert!(make_index < def_index);
        assert!(instructions.iter().skip(def_index + 1).any(|instruction| {
            matches!(
                instruction,
                MirInstr::UseVar {
                    var,
                    mode: UseMode::BorrowShared,
                    ..
                } if *var == slot
            )
        }));
    }
    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| matches!(instruction, MirInstr::MakeClosure { .. }))
            .count(),
        2,
        "direct calls load declaration-created closures instead of recapturing"
    );
}

#[test]
fn sibling_capture_keeps_the_materialized_closure_slot_as_its_environment() {
    use mojito::mir::MirCaptureMode;

    let src = "def main():\n    var x = 1\n    def helper() {var x} -> Int:\n        return x\n    x = 2\n    def caller() {helper} -> Int:\n        return helper()\n    print(caller())\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let helper_slot = main
        .var_names
        .iter()
        .position(|name| name == "helper")
        .expect("helper closure slot") as u32;
    let x_slot = main
        .var_names
        .iter()
        .position(|name| name == "x")
        .expect("x slot") as u32;
    let caller_capture = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MakeClosure {
                function, captures, ..
            } if function == "main$caller" => captures.first(),
            _ => None,
        })
        .expect("caller closure capture");
    assert_eq!(caller_capture.place.root, helper_slot);
    assert_ne!(caller_capture.place.root, x_slot);
    assert_eq!(caller_capture.mode, MirCaptureMode::Reference);
}

#[test]
fn forwarded_sibling_capture_parameter_keeps_its_callable_type() {
    let src = "def outer() -> Int:\n    var base = 40\n    def helper() {base} -> Int:\n        return base + 2\n    def middle() {helper} -> Int:\n        def inner() {helper} -> Int:\n            return helper()\n        return inner()\n    return middle()\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    let middle = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "outer$middle")
        .expect("middle lifted")
        .1;
    let helper_slot = middle
        .var_names
        .iter()
        .position(|name| name == "helper")
        .expect("forwarded helper slot") as u32;
    assert!(matches!(
        middle.var_tys.get(&helper_slot),
        Some(mojito::Ty::Func { .. })
    ));
    let capture = middle
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::MakeClosure {
                function, captures, ..
            } if function == "outer$middle$inner" => captures.first(),
            _ => None,
        })
        .expect("inner captures forwarded helper");
    assert_eq!(capture.place.root, helper_slot);
    assert!(matches!(
        capture.place.root_ty.as_ref(),
        Some(mojito::Ty::Func { .. })
    ));
}

#[test]
fn same_named_block_defs_materialize_distinct_closure_slots() {
    let src = "def main():\n    if True:\n        def choose() -> Int:\n            return 1\n        print(choose())\n    if True:\n        def choose() -> Int:\n            return 42\n        print(choose())\n";
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let closure_values: std::collections::HashSet<_> = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::MakeClosure { dest, .. } => Some(dest.0),
            _ => None,
        })
        .collect();
    let closure_slots: std::collections::HashSet<_> = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            MirInstr::DefVar { var, src, .. } if closure_values.contains(&src.0) => Some(*var),
            _ => None,
        })
        .collect();
    assert_eq!(closure_slots.len(), 2, "{:?}", main.var_names);
}

#[test]
fn origin_specialized_function_values_use_indirect_call_mir() {
    let src = "def borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef main():\n    var value = 40\n    var function = borrow[origin_of(value)]\n    ref result = function(value)\n    result += 2\n";
    let program = parse(src).expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let mut instructions = main.blocks.iter().flat_map(|block| &block.instrs);
    assert!(instructions.clone().any(|instruction| matches!(
        instruction,
        MirInstr::Const {
            k: mojito::mir::Const::Function(name),
            ..
        } if name == "borrow"
    )));
    assert!(instructions.any(|instruction| matches!(
        instruction,
        MirInstr::CallIndirect { arg_places, .. }
            if matches!(arg_places.as_slice(), [Some(_)])
    )));
}

#[test]
fn explicit_origin_selection_erases_semantic_arguments_and_types_function_constants() {
    let direct = "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\ndef main():\n    var value = 40\n    ref selected = choose[origin_of(value)](value)\n    selected += 2\n";
    let checked = mojito::check_program(&parse(direct).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .any(|instruction| matches!(
                instruction,
                MirInstr::Call {
                    func,
                    param_arg_regs,
                    ..
                } if func.0.contains("choose") && param_arg_regs.is_empty()
            ))
    );
    assert!(!main.blocks.iter().flat_map(|block| &block.instrs).any(
        |instruction| matches!(instruction, MirInstr::Call { func, .. } if func.0 == "origin_of")
    ));

    let contextual = "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\ndef main():\n    var value = 40\n    var function: def(ref[origin_of(value)] Int) thin -> ref[origin_of(value)] Int = choose[origin_of(value)]\n    ref selected = function(value)\n";
    let checked = mojito::check_program(&parse(contextual).expect("parse")).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let function_register = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::Const {
                dest,
                k: mojito::mir::Const::Function(name),
            } if name.contains("choose") => Some(dest),
            _ => None,
        })
        .expect("selected function constant");
    assert!(matches!(
        main.reg_types.get(&function_register.0),
        Some(mojito::Ty::Func { .. })
    ));
}

#[test]
fn callable_type_bound_is_retained_and_verifies_indirect_calls() {
    let src = "def apply[F: def(Int) -> Int](callback: F, value: Int) -> Int:\n    return callback(value)\n\ndef increment(value: Int) -> Int:\n    return value + 1\n\ndef main():\n    print(apply(increment, 41))\n";
    let mut mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    assert!(mojito::mir::verify::verify(&mir).is_empty());
    let apply = &mut mir
        .functions
        .iter_mut()
        .find(|(name, _)| name == "apply")
        .expect("generic apply lowered")
        .1;
    let (dest, callee, argument, target) = apply
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::CallIndirect {
                dest,
                callee,
                args,
                resolved,
                ..
            } => Some((*dest, *callee, args[0], resolved.clone())),
            _ => None,
        })
        .expect("bounded callback lowers indirectly");
    let Some(mojito::Ty::Param {
        callable_bound: Some(contract),
        ..
    }) = apply.reg_types.get(&callee.0)
    else {
        panic!("callee register must retain its bounded type parameter")
    };
    assert!(matches!(
        contract.as_ref(),
        mojito::Ty::Func { params, ret, .. }
            if params == &[mojito::Ty::Int] && **ret == mojito::Ty::Int
    ));
    assert!(
        target
            .as_deref()
            .is_some_and(|target| target.contains("__trait_dispatch.__call__")),
        "bounded call retains an abstract checked dispatch target: {target:?}"
    );
    assert_eq!(apply.reg_types.get(&dest.0), Some(&mojito::Ty::Int));
    apply.reg_types.insert(argument.0, mojito::Ty::Bool);
    let errors = mojito::mir::verify::verify(&mir);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("argument 0 of indirect callable")),
        "the callable contract must independently verify argument types: {errors:?}"
    );
}

#[test]
fn nominal_indirect_call_mir_retains_and_verifies_the_selected_overload() {
    let src = "@fieldwise_init\nstruct Choose(def(Int) -> Int):\n    def __call__(self, value: Bool) -> Int:\n        return 0\n\n    def __call__(self, value: Int) -> Int:\n        return value + 1\n\ndef invoke(callback: def(Int) -> Int) -> Int:\n    return callback(41)\n\ndef main():\n    print(Choose()(41))\n    print(invoke(Choose()))\n";
    let mut mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let target = |program: &mojito::mir::MirProgram, function_name: &str| {
        program
            .functions
            .iter()
            .find(|(name, _)| name == function_name)
            .expect("function lowered")
            .1
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .find_map(|instruction| match instruction {
                MirInstr::CallIndirect { resolved, .. } => resolved.as_deref(),
                _ => None,
            })
            .expect("indirect call has a selected target")
            .to_string()
    };
    assert_eq!(target(&mir, "main"), "Choose.__call__$ov$Int");
    assert_eq!(target(&mir, "invoke"), "__trait_dispatch.__call__$ov$Int");

    fn indirect_target<'a>(
        program: &'a mut mojito::mir::MirProgram,
        function_name: &str,
    ) -> &'a mut Option<String> {
        program
            .functions
            .iter_mut()
            .find(|(name, _)| name == function_name)
            .expect("function lowered")
            .1
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instrs)
            .find_map(|instruction| match instruction {
                MirInstr::CallIndirect { resolved, .. } => Some(resolved),
                _ => None,
            })
            .expect("indirect call lowered")
    }
    *indirect_target(&mut mir, "invoke") = Some("__trait_dispatch.__call__$ov$Bool".to_string());
    let errors = mojito::mir::verify::verify(&mir);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("does not match callable contract")),
        "the verifier must reject a stale abstract callable target: {errors:?}"
    );
    *indirect_target(&mut mir, "invoke") = Some("__trait_dispatch.__call__$ov$Int".to_string());

    let call = mir
        .functions
        .iter_mut()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instrs)
        .find(|instruction| matches!(instruction, MirInstr::CallIndirect { .. }))
        .expect("nominal indirect call lowered");
    let MirInstr::CallIndirect { resolved, .. } = call else {
        unreachable!("filtered to indirect call")
    };
    *resolved = Some("Choose.__call__$ov$Bool".to_string());
    let errors = mojito::mir::verify::verify(&mir);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("argument 0 of") && error.contains("declared Bool")),
        "the verifier must reject a stale/wrong nominal overload target: {errors:?}"
    );
}

#[test]
fn nested_origin_specialization_loads_materialized_closure_with_bound_type() {
    use mojito::Origin;
    use mojito::origin::SigOrigin;

    let src = include_str!("../conformance/fixtures/nested_origin_specialized_function_value.mojo");
    let mir = lower_program(&parse(src).expect("parse")).expect("checked lowering");
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main lowered")
        .1;
    let borrow = main
        .var_names
        .iter()
        .position(|name| name == "borrow")
        .expect("materialized nested callable") as u32;
    let specialized = main
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            MirInstr::UseVar { dest, var, .. } if *var == borrow => main.reg_types.get(&dest.0),
            _ => None,
        })
        .expect("TypeApply loads the nested closure slot");
    assert!(matches!(
        specialized,
        mojito::Ty::Func {
            ref_return: Some(signature),
            ..
        } if matches!(
            signature.origin,
            SigOrigin::Bound(Origin::Place(_))
        )
    ));
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .all(|instruction| !matches!(
                instruction,
                MirInstr::Const {
                    k: mojito::mir::Const::Function(name),
                    ..
                } if name == "borrow"
            ))
    );
}

#[test]
fn specialized_runtime_pack_is_abi_only_and_binds_as_a_tuple() {
    use mojito::{Ty, check_program, elaborate};

    let src = "def count[*Types: Copyable](*args: *Types) -> Int:\n    return len(args)\n\ndef main():\n    print(count(1, \"two\"))\n";
    let program = parse(src).expect("parse");
    let program = elaborate(program).expect("specialize heterogeneous pack");
    let checked = check_program(&program).expect("check specialization");
    let mir = mojito::mir::lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );

    let declaration = mir
        .declarations
        .functions
        .iter()
        .find(|declaration| matches!(declaration.variadic, Some(Ty::RuntimePack(_))))
        .expect("specialized declaration retains the heterogeneous ABI marker");
    assert!(matches!(
        declaration.variadic,
        Some(Ty::RuntimePack(ref elements)) if elements == &[Ty::Int, Ty::StringLiteral]
    ));

    let function = mir
        .functions
        .iter()
        .find(|(name, _)| name == &declaration.lowered_name)
        .map(|(_, function)| function)
        .expect("specialized body lowered");
    assert_eq!(
        function.param_types,
        [Ty::Tuple(vec![Ty::Int, Ty::StringLiteral])],
        "the runtime frame exposes an ordinary Tuple collector to the body"
    );
    assert_eq!(
        function.var_tys.get(&0),
        Some(&Ty::Tuple(vec![Ty::Int, Ty::StringLiteral]))
    );
    assert!(
        mojito::mir::verify::verify(&mir).is_empty(),
        "verified body types must not leak the ABI-only RuntimePack marker"
    );
}

/// The authoritative compiler pipeline's MIR — includes the iterated
/// discover→elaborate→check fixpoint that monomorphizes inferred
/// bound-generic applications, which the raw `check_program` seam skips.
fn compiled_mir(src: &str) -> mojito::mir::MirProgram {
    let compiler = Compiler::default().with_snippet_module_scope();
    let program = compiler
        .compile_source(src, Path::new("mir_test.mojo"))
        .expect("compile");
    mojito::mir::lower_checked_program(program.checked())
}

fn function_names(mir: &mojito::mir::MirProgram) -> Vec<&str> {
    mir.functions
        .iter()
        .map(|(name, _)| name.as_str())
        .collect()
}

#[test]
fn inferred_bound_generic_call_monomorphizes_and_drops_the_template() {
    let mir = compiled_mir(
        "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    print(ident(2))\n",
    );
    let names = function_names(&mir);
    assert!(
        names.iter().any(|name| name.starts_with("ident$")),
        "{names:?}"
    );
    assert!(!names.contains(&"ident"), "{names:?}");
    let main = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "main")
        .expect("main MIR")
        .1;
    let rendered = format!("{main:?}");
    assert!(rendered.contains("ident$"), "{rendered}");
}

#[test]
fn inferred_iteration_clone_uses_no_erased_iterator_dispatch() {
    // The monomorphized clone of an inferred `first(xs, -1)` iterates
    // `List[Int]` through the ordinary concrete borrowed protocol; no
    // `__iterator_dispatch` shim remains at that call site. (The template body
    // must also be abstractly valid — round one checks the retained template —
    // so this uses the stdlib `first_or` shape.) This is the Stage-E
    // retirement baseline for inferred applications.
    let mir = compiled_mir(
        "from std.iterable import Iterable\n\ndef first[C: Iterable](items: C, default: C.Element) -> C.Element:\n    for item in items:\n        return item\n    return default\n\ndef main():\n    var xs = [3, 4, 5]\n    print(first(xs, -1))\n",
    );
    let names = function_names(&mir);
    let clone = mir
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("first$"))
        .unwrap_or_else(|| panic!("first clone exists: {names:?}"));
    let rendered = format!("{:?}", clone.1);
    assert!(
        !rendered.contains("__iterator_dispatch"),
        "clone still dispatches abstractly: {rendered}"
    );
    assert!(!names.contains(&"first"), "{names:?}");
}

#[test]
fn clone_interior_inferred_calls_reach_a_second_discovery_round() {
    // `outer`'s request is discovered in round one; `inner`'s instantiation is
    // recorded only inside the generated `outer$…` clone (a stamped source),
    // so its request is discovered in round two — pinning clone-span
    // stability across re-elaborations.
    let mir = compiled_mir(
        "def inner[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef outer[T: Copyable & Movable](x: T) -> T:\n    return inner(x)\n\ndef main():\n    print(outer(7))\n",
    );
    let names = function_names(&mir);
    assert!(
        names.iter().any(|name| name.starts_with("outer$")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name.starts_with("inner$")),
        "{names:?}"
    );
    assert!(!names.contains(&"outer"), "{names:?}");
    let outer_clone = &mir
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("outer$"))
        .expect("outer clone")
        .1;
    let rendered = format!("{outer_clone:?}");
    assert!(rendered.contains("inner$"), "{rendered}");
}

#[test]
fn conflicting_unrolled_occurrences_stay_on_the_abstract_path() {
    // `comptime for` unrolling duplicates one source occurrence with two
    // incompatible instantiations; the discovery loop drops the occurrence and
    // both calls keep the retained template's erased path.
    let mir = compiled_mir(
        "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    comptime for i in (1, \"s\"):\n        print(ident(i))\n",
    );
    let names = function_names(&mir);
    assert!(names.contains(&"ident"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("ident$")),
        "{names:?}"
    );
}

#[test]
fn conflict_retained_template_keeps_dispatch_and_adapter_under_the_compiler() {
    // The erased-dispatch residue witness for the authoritative pipeline: the
    // `comptime for` unrolling records conflicting closed instantiations at
    // one source occurrence, so discovery drops the occurrence and the
    // retained abstract template keeps the `__iterator_dispatch` protocol and
    // its `CopyIteratorReference` adapter. If the fixpoint ever
    // over-monomorphizes or abstract checking of retained templates breaks,
    // this pin notices.
    let mir = compiled_mir(
        "from std.iterable import Iterable\n\ndef first[C: Iterable](items: C, default: C.Element) -> C.Element:\n    for item in items:\n        return item\n    return default\n\ndef main():\n    comptime for i in (1, \"s\"):\n        print(first([i, i], i))\n",
    );
    let names = function_names(&mir);
    assert!(names.contains(&"first"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("first$")),
        "{names:?}"
    );
    let template = &mir
        .functions
        .iter()
        .find(|(name, _)| name == "first")
        .expect("retained template")
        .1;
    let rendered = format!("{template:?}");
    assert!(rendered.contains("__iterator_dispatch"), "{rendered}");
    assert!(rendered.contains("CopyIteratorReference"), "{rendered}");
}

#[test]
fn function_value_reference_retains_the_bound_generic_template() {
    // A function-value use has no application to monomorphize against, so it
    // pins the abstract template — the designed erased-dispatch fallback.
    let mir = compiled_mir(
        "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    var callback: def(Int) -> Int = ident\n    print(callback(41))\n",
    );
    let names = function_names(&mir);
    assert!(names.contains(&"ident"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("ident$")),
        "{names:?}"
    );
}
