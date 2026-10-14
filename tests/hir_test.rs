//! Phase 1 (AST → HIR) tests. They assert the **shape** of the region tree —
//! region ops, their regions' blocks, and terminators — for each control-flow
//! construct, since Phase 1 is about structure, not instruction contents.

use mojito::Ty;
use mojito::hir::{Cfg, HirCond, HirExcept, HirInstr, HirRegion, Terminator};
use mojito::{check_program, parse};

/// Check a module-scope snippet and lower its statements to a CFG.
fn checked_cfg(src: &str) -> Cfg {
    let program = parse(src).expect("parse error");
    let checked = check_program(&program).expect("type error");
    Cfg::build_checked_fn(&checked, &[], checked.tables().roots())
}

/// Lower one named function body through the checked HIR handoff.
fn checked_function_cfg(src: &str, function: &str) -> Cfg {
    let program = parse(src).expect("parse error");
    let checked = check_program(&program).expect("type error");
    let (params, body) = function_body(&checked, function);
    Cfg::build_checked_fn(&checked, &params, &body)
}

/// [`checked_function_cfg`] with the bundled library linked, which a
/// `comptime for` over a `range` steps through.
fn linked_function_cfg(src: &str, function: &str) -> Cfg {
    let linked = mojito::link_source(src, std::path::Path::new("hir_test.mojo")).expect("link");
    let program = mojito::elaborate(linked).expect("elaborate");
    let checked = check_program(&program).expect("type error");
    let (params, body) = function_body(&checked, function);
    Cfg::build_checked_fn(&checked, &params, &body)
}

/// The parameter names and body statement nodes of the checked function
/// named `function`.
fn function_body(
    checked: &mojito::checked::CheckedProgram,
    function: &str,
) -> (Vec<String>, Vec<mojito::checked::CheckedStmtId>) {
    let tables = checked.tables();
    tables
        .roots()
        .iter()
        .find_map(|root| match &tables.statement(*root)?.op {
            mojito::checked::CheckedStmtOp::Def(def) if def.name == function => Some((
                def.params.iter().map(|param| param.name.clone()).collect(),
                def.body.clone(),
            )),
            _ => None,
        })
        .expect("function body")
}

/// The body statement nodes of the checked `def` at root `index`.
fn root_def_body(
    checked: &mojito::checked::CheckedProgram,
    index: usize,
) -> Vec<mojito::checked::CheckedStmtId> {
    let tables = checked.tables();
    match &tables.statement(tables.roots()[index]).expect("root").op {
        mojito::checked::CheckedStmtOp::Def(def) => def.body.clone(),
        _ => panic!("expected function"),
    }
}

/// Checked function `main` of `src`, and the label of its first loop.
fn try_in_loop(src: &str) -> (Cfg, u32) {
    let cfg = checked_function_cfg(src, "main");
    let (label, _) = the_loop(&cfg, &cfg.root);
    (cfg, label)
}

/// The parts of the one `Try` op of `region`'s entry block.
fn the_try<'c>(cfg: &'c Cfg, region: &HirRegion) -> (&'c HirRegion, Option<&'c HirExcept>) {
    entry_instrs(cfg, region)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::Try { body, except, .. } => Some((body, except.as_ref())),
            _ => None,
        })
        .expect("a try op")
}

/// The body of the `try` that the body of the first loop of `cfg` holds.
fn loop_try(cfg: &Cfg) -> &HirRegion {
    let (_, body) = the_loop(cfg, &cfg.root);
    the_try(cfg, body).0
}

/// Every block must be sealed with exactly one terminator.
fn assert_all_sealed(cfg: &Cfg) {
    for b in cfg.g.node_indices() {
        assert!(cfg.term(b).is_some(), "block {b:?} has no terminator");
    }
}

/// The instructions of `region`'s entry block.
fn entry_instrs<'c>(cfg: &'c Cfg, region: &HirRegion) -> &'c [HirInstr] {
    &cfg.block(region.blocks[0]).instrs
}

/// The terminator of `region`'s last block.
fn last_term<'c>(cfg: &'c Cfg, region: &HirRegion) -> &'c Terminator {
    cfg.term(*region.blocks.last().expect("a region has a block"))
        .expect("sealed")
}

/// The one `Loop` op of `region`'s entry block: its label and body.
fn the_loop<'c>(cfg: &'c Cfg, region: &HirRegion) -> (u32, &'c HirRegion) {
    entry_instrs(cfg, region)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::Loop { label, body } => Some((*label, body)),
            _ => None,
        })
        .expect("a loop op")
}

/// The `If` a loop body opens with: its operand and its else arm.
fn loop_test<'c>(cfg: &'c Cfg, body: &HirRegion) -> (&'c HirCond, &'c HirRegion) {
    entry_instrs(cfg, body)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::If {
                cond, then, orelse, ..
            } => {
                assert!(matches!(last_term(cfg, then), Terminator::Yield));
                Some((cond, orelse))
            }
            _ => None,
        })
        .expect("a loop body opens with its test")
}

#[test]
fn straight_line_is_one_block_returning() {
    // No control flow ⇒ a single block, implicitly `return None`.
    let c = checked_cfg("var x: Int = 1\nvar y: Int = 2\n");
    assert_eq!(c.node_count(), 1);
    assert_eq!(c.root.blocks, [c.entry]);
    assert_eq!(c.edge_count(), 0);
    assert!(matches!(c.term(c.entry), Some(Terminator::Return(None))));
    assert_eq!(c.block(c.entry).instrs.len(), 2); // two Binds
    assert_all_sealed(&c);
}

#[test]
fn if_without_else_is_one_if_op_with_yielding_arms() {
    let c = checked_function_cfg("def main(a: Bool):\n    if a:\n        pass\n", "main");
    assert_eq!(
        c.root.blocks.len(),
        1,
        "the `if` adds no block to its region"
    );
    assert_eq!(c.node_count(), 3);
    assert_eq!(c.edge_count(), 0);
    assert_all_sealed(&c);
    match entry_instrs(&c, &c.root) {
        [
            HirInstr::If {
                cond: HirCond::Expr(_),
                then,
                elifs,
                orelse,
            },
        ] => {
            assert!(elifs.is_empty());
            assert!(matches!(last_term(&c, then), Terminator::Yield));
            assert!(matches!(last_term(&c, orelse), Terminator::Yield));
        }
        other => panic!("expected one `if` op, got {other:?}"),
    }
}

#[test]
fn elif_is_a_condition_region_not_a_nested_if() {
    let c = checked_function_cfg(
        "def main(a: Bool, b: Bool):\n    if a:\n        pass\n    elif b:\n        pass\n    else:\n        pass\n",
        "main",
    );
    assert_all_sealed(&c);
    let [HirInstr::If { elifs, orelse, .. }] = entry_instrs(&c, &c.root) else {
        panic!("expected one `if` op");
    };
    let [elif] = elifs.as_slice() else {
        panic!("expected one elif");
    };
    assert!(matches!(last_term(&c, &elif.cond), Terminator::ElifCond(_)));
    assert!(matches!(last_term(&c, &elif.then), Terminator::Yield));
    assert!(
        !entry_instrs(&c, orelse)
            .iter()
            .any(|instruction| matches!(instruction, HirInstr::If { .. })),
        "the else arm holds no nested `if`"
    );
}

#[test]
fn while_loop_is_a_labelled_loop_that_tests_first() {
    let c = checked_function_cfg("def main(a: Bool):\n    while a:\n        pass\n", "main");
    assert_all_sealed(&c);
    assert_eq!(c.root.blocks.len(), 1);
    let (label, body) = the_loop(&c, &c.root);
    let (cond, exit) = loop_test(&c, body);
    assert!(matches!(cond, HirCond::Expr(_)));
    assert!(matches!(last_term(&c, exit), Terminator::Break(l) if *l == label));
    assert!(matches!(last_term(&c, body), Terminator::Continue(l) if *l == label));
    assert_eq!(c.labels, label + 1);
}

#[test]
fn break_leaves_the_loop_without_continuing() {
    let c = checked_function_cfg("def main(a: Bool):\n    while a:\n        break\n", "main");
    let (label, body) = the_loop(&c, &c.root);
    assert!(matches!(last_term(&c, body), Terminator::Break(l) if *l == label));
    assert!(
        !body
            .blocks
            .iter()
            .any(|b| matches!(c.term(*b), Some(Terminator::Continue(_)))),
        "a body that always breaks never continues"
    );
    assert_all_sealed(&c);
}

#[test]
fn continue_starts_the_next_iteration() {
    let c = checked_function_cfg(
        "def main(a: Bool):\n    while a:\n        continue\n",
        "main",
    );
    let (label, body) = the_loop(&c, &c.root);
    assert!(matches!(last_term(&c, body), Terminator::Continue(l) if *l == label));
    assert_all_sealed(&c);
}

#[test]
fn nested_loops_break_names_the_innermost_label() {
    let c = checked_function_cfg(
        "def main(a: Bool, b: Bool):\n    while a:\n        while b:\n            break\n",
        "main",
    );
    let (outer, outer_body) = the_loop(&c, &c.root);
    let (inner, inner_body) = the_loop(&c, outer_body);
    assert_ne!(outer, inner, "every loop of a function has its own label");
    assert!(matches!(last_term(&c, inner_body), Terminator::Break(l) if *l == inner));
    assert!(matches!(last_term(&c, outer_body), Terminator::Continue(l) if *l == outer));
    assert_all_sealed(&c);
}

#[test]
fn a_break_in_a_loop_else_names_the_enclosing_loop() {
    let c = checked_function_cfg(
        "def main(a: Bool, b: Bool):\n    while a:\n        while b:\n            pass\n        else:\n            break\n",
        "main",
    );
    let (outer, outer_body) = the_loop(&c, &c.root);
    let (inner, inner_body) = the_loop(&c, outer_body);
    let (_, inner_else) = loop_test(&c, inner_body);
    assert!(
        matches!(last_term(&c, inner_else), Terminator::Break(l) if *l == outer),
        "the else body's own break leaves the outer loop"
    );
    assert!(
        !inner_else
            .blocks
            .iter()
            .any(|b| matches!(c.term(*b), Some(Terminator::Break(l)) if *l == inner))
    );
}

#[test]
fn return_seals_the_block_with_a_value() {
    let c = checked_function_cfg("def main() -> Int:\n    return 1\n", "main");
    assert_eq!(c.node_count(), 1);
    assert!(matches!(c.term(c.entry), Some(Terminator::Return(Some(_)))));
    assert_all_sealed(&c);
}

#[test]
fn a_returning_arm_does_not_yield() {
    let c = checked_function_cfg(
        "def main(a: Bool) -> Int:\n    if a:\n        return 1\n    else:\n        pass\n    return 2\n",
        "main",
    );
    let [HirInstr::If { then, orelse, .. }] = entry_instrs(&c, &c.root) else {
        panic!("expected one `if` op");
    };
    assert!(matches!(last_term(&c, then), Terminator::Return(Some(_))));
    assert!(matches!(last_term(&c, orelse), Terminator::Yield));
    assert!(matches!(c.term(c.entry), Some(Terminator::Return(Some(_)))));
    assert_all_sealed(&c);
}

#[test]
fn a_comptime_if_is_one_comptime_if_op() {
    let c = checked_function_cfg(
        "def f[n: Int]() -> Int:\n    var r = 0\n    comptime if n > 0:\n        r = 1\n    elif n < 0:\n        r = 2\n    else:\n        r = 3\n    return r\n",
        "f",
    );
    assert_eq!(c.root.blocks.len(), 1);
    let ops: Vec<_> = entry_instrs(&c, &c.root)
        .iter()
        .filter_map(|instruction| match instruction {
            HirInstr::ComptimeIf {
                conds,
                arms,
                orelse,
            } => Some((conds, arms, orelse)),
            _ => None,
        })
        .collect();
    let [(conds, arms, orelse)] = ops.as_slice() else {
        panic!("expected one `comptime if` op");
    };
    assert_eq!(conds.len(), 2);
    assert_eq!(arms.len(), 2);
    for region in arms.iter().chain([*orelse]) {
        assert_eq!(region.blocks.len(), 1);
        assert!(matches!(last_term(&c, region), Terminator::ComptimeYield));
    }
    assert_eq!(c.edge_count(), 0);
    assert_all_sealed(&c);
}

#[test]
fn a_returning_comptime_arm_does_not_yield() {
    let c = checked_function_cfg(
        "def f[n: Int]() -> Int:\n    comptime if n > 0:\n        return 1\n    return 2\n",
        "f",
    );
    let [HirInstr::ComptimeIf { arms, orelse, .. }] = entry_instrs(&c, &c.root) else {
        panic!("expected one `comptime if` op");
    };
    assert!(matches!(
        last_term(&c, &arms[0]),
        Terminator::Return(Some(_))
    ));
    assert!(matches!(last_term(&c, orelse), Terminator::ComptimeYield));
    assert_all_sealed(&c);
}

#[test]
fn a_comptime_if_arm_break_is_a_comptime_break() {
    let c = linked_function_cfg(
        "def main():\n    comptime for i in range(3):\n        comptime if i == 1:\n            break\n",
        "main",
    );
    let body = entry_instrs(&c, &c.root)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::ComptimeFor { body, .. } => Some(body),
            _ => None,
        })
        .expect("a comptime for op");
    let [HirInstr::ComptimeIf { arms, .. }] = entry_instrs(&c, body) else {
        panic!("expected the body's `comptime if`");
    };
    assert!(matches!(last_term(&c, &arms[0]), Terminator::ComptimeBreak));
    assert_all_sealed(&c);
}

#[test]
fn for_loop_tests_its_has_next_slot_before_binding() {
    let c = checked_cfg("for i in range(3):\n    pass\n");
    let (label, body) = the_loop(&c, &c.root);
    let (cond, exit) = loop_test(&c, body);
    assert!(matches!(cond, HirCond::Slot(_)));
    assert!(matches!(last_term(&c, exit), Terminator::Break(l) if *l == label));
    assert!(matches!(
        entry_instrs(&c, body),
        [
            HirInstr::HasNext { .. },
            HirInstr::If { .. },
            HirInstr::Next { .. },
            HirInstr::BindIteration { .. },
            HirInstr::Stmt(_), // `pass`
        ]
    ));
    assert!(matches!(last_term(&c, body), Terminator::Continue(l) if *l == label));
    assert_all_sealed(&c);
}

#[test]
fn a_comptime_for_stop_arm_is_a_comptime_break() {
    let c = linked_function_cfg(
        "def main():\n    comptime for i in range(3):\n        print(i)\n",
        "main",
    );
    let orelse = entry_instrs(&c, &c.root)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::ComptimeFor { orelse, .. } => Some(orelse),
            _ => None,
        })
        .expect("a comptime for op");
    assert!(entry_instrs(&c, orelse).is_empty());
    assert!(matches!(last_term(&c, orelse), Terminator::ComptimeBreak));
    assert_all_sealed(&c);
}

#[test]
fn a_comptime_for_break_inside_an_if_is_a_comptime_break() {
    let c = linked_function_cfg(
        "def main(a: Bool):\n    comptime for i in range(3):\n        if a:\n            break\n",
        "main",
    );
    let body = entry_instrs(&c, &c.root)
        .iter()
        .find_map(|instruction| match instruction {
            HirInstr::ComptimeFor { body, .. } => Some(body),
            _ => None,
        })
        .expect("a comptime for op");
    assert!(matches!(last_term(&c, body), Terminator::ComptimeContinue));
    let [HirInstr::If { then, .. }] = entry_instrs(&c, body) else {
        panic!("expected the body's `if`");
    };
    assert!(matches!(last_term(&c, then), Terminator::ComptimeBreak));
    assert_all_sealed(&c);
}

#[test]
fn try_part_break_continue_name_the_enclosing_loop() {
    let (cfg, label) = try_in_loop(
        "def main():\n    while True:\n        try:\n            break\n        finally:\n            pass\n",
    );
    let body = loop_try(&cfg);
    assert_eq!(body.blocks.len(), 1);
    assert!(matches!(last_term(&cfg, body), Terminator::Break(l) if *l == label));
    let (cfg, label) = try_in_loop(
        "def main():\n    while True:\n        try:\n            continue\n        finally:\n            pass\n",
    );
    let body = loop_try(&cfg);
    assert!(matches!(last_term(&cfg, body), Terminator::Continue(l) if *l == label));
}

#[test]
fn nested_loop_in_try_part_takes_a_fresh_label() {
    let (cfg, outer) = try_in_loop(
        "def main():\n    while True:\n        try:\n            while True:\n                break\n        finally:\n            pass\n",
    );
    let body = loop_try(&cfg);
    assert!(matches!(last_term(&cfg, body), Terminator::TryYield));
    let (label, inner) = the_loop(&cfg, body);
    assert_ne!(label, outer, "the function numbers every loop once");
    assert!(matches!(last_term(&cfg, inner), Terminator::Break(l) if *l == label));
    assert_eq!(cfg.labels, 2);
}

#[test]
fn dead_region_op_is_erased() {
    let cfg = checked_function_cfg(
        "def f(x: Int) -> Int:\n    return 1\n    if x > 0:\n        var y = x\n        print(y)\n    return 3\n",
        "f",
    );
    assert_eq!(cfg.root.blocks.len(), 1);
    assert_eq!(cfg.g.node_count(), 1, "the dead arm lowers no block");
    assert!(!cfg.vars.iter().any(|name| name == "y"));
    assert!(
        !entry_instrs(&cfg, &cfg.root)
            .iter()
            .any(|instruction| matches!(instruction, HirInstr::If { .. }))
    );
    assert!(matches!(
        last_term(&cfg, &cfg.root),
        Terminator::Return(Some(_))
    ));
}

#[test]
fn raise_erases_rest_of_block() {
    let cfg = checked_function_cfg(
        "def f(x: Int) raises -> Int:\n    raise Error(\"e\")\n    var dead = x\n    return dead\n",
        "f",
    );
    assert!(!cfg.vars.iter().any(|name| name == "dead"));
    let statements = entry_instrs(&cfg, &cfg.root)
        .iter()
        .filter(|instruction| matches!(instruction, HirInstr::Stmt(_)))
        .count();
    assert_eq!(statements, 1, "only the `raise` lowers");
}

#[test]
fn checked_hir_retains_semantic_identity_type_and_adjustments() {
    let body = mojito::parse(
        "def f(x: Int) -> Int:\n    return x\n\ndef main():\n    var y: Int = f(1 + 2)\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&body).expect("check");
    let main_body = root_def_body(&checked, 1);
    let cfg = Cfg::build_checked_fn(&checked, &[], &main_body);
    let expression = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instr| match instr {
            mojito::hir::HirInstr::Bind { expr, .. } => Some(expr),
            _ => None,
        })
        .expect("checked bind");
    let node = |id| cfg.checked.expression(id).expect("checked node");
    assert!(node(expression.node).ty.is_some());
    assert_eq!(
        expression.children.len(),
        1,
        "the call retains its argument"
    );
    assert_eq!(
        expression.children[0].children.len(),
        2,
        "the argument retains both operator operands"
    );
    assert!(
        expression
            .children
            .iter()
            .flat_map(|child| std::iter::once(child).chain(&child.children))
            .all(|child| node(child.node).ty.is_some()),
        "every recursively retained child is a typed checked node"
    );
}

#[test]
fn checked_hir_places_use_owner_identity_and_typed_projections() {
    let program = mojito::parse(
        "@fieldwise_init\nstruct Cell:\n    var value: Int\n\ndef read(cell: Cell) -> Int:\n    return cell.value\n",
    ).expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let body = root_def_body(&checked, 1);
    let cfg = Cfg::build_checked_fn(&checked, &["cell".to_string()], &body);
    let expression = cfg
        .g
        .node_weights()
        .find_map(|block| match &block.term {
            Some(Terminator::Return(Some(expression))) => Some(expression),
            _ => None,
        })
        .expect("return expression");
    let place = expression.place.as_ref().expect("checked place");
    assert!(matches!(place.root_ty.kind(), mojito::TyKind::Struct(name, _) if name == "Cell"));
    assert_eq!(place.projections.len(), 1);
    assert_eq!(place.ty, Ty::int());
}

#[test]
fn checked_hir_retains_distinct_comprehension_binding_owners() {
    let program = mojito::parse(
        "def main():\n    var result = [x for x in range(2) for x in range(x + 1)]\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let body = root_def_body(&checked, 0);
    let cfg = Cfg::build_checked_fn(&checked, &[], &body);
    let expression = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::Bind { expr, .. } => cfg
                .checked
                .expression(expr.node)
                .filter(|node| matches!(node.op, mojito::checked::CheckedOp::Comprehension { .. })),
            _ => None,
        })
        .expect("comprehension binding");
    assert_eq!(expression.comprehension_bindings.len(), 2);
    assert_ne!(
        expression.comprehension_bindings[0].owner,
        expression.comprehension_bindings[1].owner
    );
}

#[test]
fn checked_hir_carries_nested_declaration_and_call_binding_identity() {
    let program = mojito::parse(
        "def main():\n    if True:\n        def choose() -> Int:\n            return 1\n        choose()\n    if True:\n        def choose() -> Int:\n            return 42\n        choose()\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let body = root_def_body(&checked, 0);
    let cfg = Cfg::build_checked_fn(&checked, &[], &body);

    let declarations: Vec<_> = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            mojito::hir::HirInstr::Stmt(statement) => cfg
                .checked
                .statement(statement.node)
                .filter(|node| {
                    matches!(
                        &node.op,
                        mojito::checked::CheckedStmtOp::Def(def) if def.name == "choose"
                    )
                })
                .and_then(|node| node.binding),
            _ => None,
        })
        .collect();
    let calls: Vec<_> = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            mojito::hir::HirInstr::Eval(expression) => cfg
                .checked
                .expression(expression.node)
                .filter(|node| {
                    matches!(
                        &node.op,
                        mojito::checked::CheckedOp::Call { name, .. } if name == "choose"
                    )
                })
                .and_then(|node| node.binding),
            _ => None,
        })
        .collect();
    assert_eq!(declarations.len(), 2);
    assert_eq!(calls.len(), 2);
    assert_ne!(declarations[0], declarations[1]);
    assert_eq!(calls, declarations);
}

#[test]
fn checked_hir_next_retains_a_shadowed_loop_binder_identity() {
    let program = mojito::parse(
        "def main():\n    var item = 1\n    var items: List[Int] = [40]\n    for item in items:\n        def read() {item} -> Int:\n            return item\n        print(read())\n",
    )
    .expect("parse");
    let checked = mojito::check_program(&program).expect("check");
    let body = root_def_body(&checked, 0);
    let cfg = Cfg::build_checked_fn(&checked, &[], &body);
    let outer = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::Bind { binding, .. } => *binding,
            _ => None,
        })
        .expect("outer item binding");
    let loop_item = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::BindIteration { binding, .. } => Some(*binding),
            _ => None,
        })
        .expect("loop item binding");
    assert_ne!(outer, loop_item);
}

#[test]
fn checked_hir_raising_iterator_has_explicit_exhaustion_result() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct I:\n    var current: Int\n    var end: Int\n    def __next__(mut self) raises StopIteration -> Int:\n        if self.current >= self.end:\n            raise StopIteration(0)\n        var result = self.current\n        self.current += 1\n        return result\n\n@fieldwise_init\nstruct C:\n    var end: Int\n    def __iter__(self) -> I:\n        return I(0, self.end)\n\ndef main():\n    for value in C(2):\n        print(value)\n";
    let cfg = checked_function_cfg(source, "main");
    let operations: Vec<_> = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .collect();
    let (element_ty, call) = operations
        .iter()
        .copied()
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::TryNext {
                exhaustion,
                element_ty,
                call,
                ..
            } if matches!(
                exhaustion.kind(),
                mojito::TyKind::Struct(name, arguments)
                    if name == "StopIteration" && arguments.is_empty()
            ) =>
            {
                Some((element_ty, call))
            }
            _ => None,
        })
        .expect("typed TryNext");
    assert_eq!(element_ty, &Ty::int());
    assert_eq!(&call.result_ty, &Ty::int());
    assert!(call.reference_result.is_none());
    assert!(!operations.iter().any(|instruction| matches!(
        instruction,
        mojito::hir::HirInstr::HasNext { .. } | mojito::hir::HirInstr::Next { .. }
    )));
}

#[test]
fn checked_hir_reference_yielding_try_next_retains_a_reference_result() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct RefIter[o: Origin[mut=False]]:\n    var source: ref[o] Int\n    var done: Bool\n    def __next__(mut self) raises StopIteration -> ref[Self.o] Int:\n        if self.done:\n            raise StopIteration(0)\n        self.done = True\n        return self.source\n\n@fieldwise_init\nstruct RefSource:\n    var value: Int\n    def __iter__(ref self) -> RefIter[origin_of(self.value)]:\n        ref value = self.value\n        return RefIter(value, False)\n\ndef main():\n    var source = RefSource(42)\n    for item in source:\n        print(item)\n";
    let cfg = checked_function_cfg(source, "main");
    let (element_ty, call) = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::TryNext {
                element_ty, call, ..
            } => Some((element_ty, call)),
            _ => None,
        })
        .expect("reference-yielding TryNext");
    assert!(matches!(
        element_ty.kind(),
        mojito::TyKind::Ref(reference) if reference.referent == mojito::TyKind::Int
    ));
    assert!(matches!(
        call.result_ty.kind(),
        mojito::TyKind::Ref(reference) if reference.referent == mojito::TyKind::Int
    ));
    assert_eq!(
        call.reference_result.as_ref(),
        match call.result_ty.kind() {
            mojito::TyKind::Ref(reference) => Some(reference),
            _ => None,
        }
    );
}

// Pins the erased machinery via the raw check seam; under the authoritative
// Compiler this program's closed applications monomorphize.
#[test]
fn checked_hir_retains_the_abstract_iterator_copy_adapter() {
    let source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    let cfg = checked_function_cfg(source, "first");
    let adapter = cfg
        .g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .find_map(|instruction| match instruction {
            mojito::hir::HirInstr::TryNext { call, .. } => Some(call.result_adapter),
            _ => None,
        })
        .expect("abstract iterator next");
    assert_eq!(
        adapter,
        Some(mojito::checked::CheckedResultAdapter::CopyIteratorReference)
    );
}

/// How many expressions carrying a lifted application the checked walk
/// reaches from `body`, given what it does at a nested `def`.
fn lifted_applications_reached(
    tables: &mojito::checked::CheckedTables,
    body: &[mojito::checked::CheckedStmtId],
    at_def: mojito::checked::WalkResult,
) -> usize {
    use mojito::checked::{CheckedStmtOp, SemanticAdjustment, WalkResult, Walked};
    let mut reached = 0;
    tables.walk(body, &mut |node| match node {
        Walked::Statement(statement) if matches!(statement.op, CheckedStmtOp::Def(_)) => at_def,
        Walked::Statement(_) => WalkResult::Advance,
        Walked::Expression(expression) => {
            reached += usize::from(expression.adjustments.iter().any(|adjustment| {
                matches!(adjustment, SemanticAdjustment::ComptimeApplication { .. })
            }));
            WalkResult::Advance
        }
    });
    reached
}

// The application spelled only in a local's annotation is reached through
// the declaration's op, though no operand names it; a nested `def`'s is
// reached only when the walk enters the def's body.
#[test]
fn checked_walk_reaches_annotation_applications_and_skips_nested_bodies() {
    use mojito::checked::WalkResult;
    let source = "\
def h(x: Int) -> Int:
    return x * 2

def f[n: Int]():
    comptime vals = [n, n * 2]
    var v: SIMD[DType.int32, h(vals[0]) - vals[0]] = 5
    print(v)

def outer[n: Int]():
    def inner():
        var v: SIMD[DType.int32, h(n) - n] = 5
        print(v)
    inner()
";
    let compiled = mojito::compiler::Compiler::default()
        .compile_unlinked(source)
        .expect("compile");
    let checked = compiled.checked();
    let cfg = |function| {
        let (params, body) = function_body(checked, function);
        Cfg::build_checked_fn(checked, &params, &body)
    };
    let f = cfg("f");
    assert!(lifted_applications_reached(&f.checked, &f.body, WalkResult::Advance) > 0);
    let outer = cfg("outer");
    assert!(lifted_applications_reached(&outer.checked, &outer.body, WalkResult::Advance) > 0);
    assert_eq!(
        lifted_applications_reached(&outer.checked, &outer.body, WalkResult::SkipBodies),
        0
    );
}

/// The `Bind` destinations of every block, in graph order.
fn bind_dests(cfg: &Cfg) -> Vec<mojito::hir::VarId> {
    cfg.g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            mojito::hir::HirInstr::Bind { dest, .. } => Some(*dest),
            _ => None,
        })
        .collect()
}

#[test]
fn discard_assignments_bind_distinct_slots() {
    let cfg = checked_function_cfg("def f():\n    _ = 1\n    _ = \"s\"\n", "f");
    let dests = bind_dests(&cfg);
    assert_eq!(dests.len(), 2);
    assert_ne!(dests[0], dests[1]);
    assert!(!cfg.vars.iter().any(|name| name == "_"));
}

#[test]
fn named_local_rebinds_one_slot() {
    let cfg = checked_function_cfg("def f():\n    var x = 1\n    x = 2\n", "f");
    let dests = bind_dests(&cfg);
    assert_eq!(dests.len(), 2);
    assert_eq!(dests[0], dests[1]);
}

/// The `try` ops HIR keeps in `cfg`, in graph order: each one's body and
/// `except` part.
fn try_ops(cfg: &Cfg) -> Vec<(&HirRegion, Option<&HirExcept>)> {
    cfg.g
        .node_weights()
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            HirInstr::Try { body, except, .. } => Some((body, except.as_ref())),
            _ => None,
        })
        .collect()
}

/// The slots the `Bind`s of `region`'s block write.
fn region_bind_dests(cfg: &Cfg, region: &HirRegion) -> Vec<u32> {
    entry_instrs(cfg, region)
        .iter()
        .filter_map(|instruction| match instruction {
            HirInstr::Bind { dest, .. } => Some(*dest),
            _ => None,
        })
        .collect()
}

#[test]
fn try_region_resolves_a_name_by_its_binding() {
    let src = "def main():\n    var h = 1\n    if True:\n        var h = 2\n        try:\n            h = 5\n            print(h)\n        except e:\n            print(e)\n";
    let cfg = checked_function_cfg(src, "main");
    let (body, _) = try_ops(&cfg)[0];
    let dests = region_bind_dests(&cfg, body);
    assert_eq!(dests.len(), 1);
    let outer = cfg
        .vars
        .iter()
        .position(|name| name == "h")
        .expect("outer h") as u32;
    assert_ne!(dests[0], outer, "the assignment writes the inner `h`");
    assert!(cfg.bindings.values().any(|slot| *slot == dests[0]));
}

#[test]
fn sequential_except_names_get_distinct_slots() {
    let src = "def main():\n    try:\n        raise Error(\"a\")\n    except e:\n        print(e)\n    try:\n        raise Error(\"b\")\n    except e:\n        print(e)\n";
    let cfg = checked_function_cfg(src, "main");
    let handlers: Vec<u32> = try_ops(&cfg)
        .into_iter()
        .map(|(_, except)| {
            let except = except.expect("a handler");
            let slot = except.slot.expect("a named handler's slot");
            assert_eq!(cfg.bindings[&except.binding.expect("its binding")], slot);
            slot
        })
        .collect();
    assert_eq!(handlers.len(), 2);
    assert_ne!(handlers[0], handlers[1]);
    for handler in &handlers {
        assert!(cfg.slots.values().any(|read| read == handler));
    }
}
