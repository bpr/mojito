//! The pre-elaboration pipeline: passes over parametric MIR whose drops are
//! inserted, run before the elaborator instantiates it, as upstream runs its
//! pre-elaboration pass pipeline on parametric IR after destructor
//! insertion.
//!
//! It runs at two seams: over a program's own bodies
//! ([`pre_elaborate_program`]), where a [`ProgramPass`] runs once over the
//! whole program, a [`BodyPass`] over each body, and a [`CallGraphPass`]
//! over each body callees first, and over each imported package body as it
//! is finished ([`finish_package_body`]), where the body and call-graph
//! passes run. Never per instance.

use super::canonicalize::CANONICALIZE;
use super::cse::CSE;
use super::dead_symbols::ELIMINATE_DEAD_SYMBOLS;
use super::duplicate_functions::ELIMINATE_DUPLICATE_FUNCTIONS;
use super::elaborate_drops_function;
use super::inline_parametric::INLINE_PARAMETRIC;
use super::lower_lit::{LOWER_LIT, lower_lit};
use super::mem2reg::{LOWER_ARG_CONVENTIONS, MEM_2_REG};
use super::sccp::SCCP;
use super::unused_params::REMOVE_UNUSED_PARAMS;
use mojito_common::timing;
use mojito_mir::mir::text::package::PackageBodies;
use mojito_mir::mir::{
    Const, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirProgram,
    instruction_regions,
};
use std::collections::{HashMap, HashSet};

/// One pre-elaboration pass over a parametric body whose drops are
/// inserted.
pub struct BodyPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    /// Rewrite the body, and say whether it changed: `false` is MLIR's
    /// `markAllAnalysesPreserved`, so a caller need not verify the body
    /// again.
    pub run: fn(&str, &mut MirFunction, &MirDeclarations) -> bool,
}

/// One pre-elaboration pass over a whole program, which may remove its
/// functions: upstream's module passes, such as `EliminateDeadSymbols`.
pub struct ProgramPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    /// Rewrite the program, given the symbols it exports beside its entry
    /// roots.
    pub run: fn(&mut MirProgram, &[String]),
}

/// One pre-elaboration pass over a body that reads the finished bodies of
/// its callees, as MLIR's inliner walks the call graph.
///
/// Upstream's `InlineParametric` is one, and so is its `RemoveUnusedParams`,
/// which walks its own call graph callees first.
///
/// A seam runs it over each body after every callee the body calls through
/// a callee it `follows`, so the callee arrives already rewritten. A callee
/// on a cycle of such calls has no body to read ([`Callees::body`]).
pub struct CallGraphPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    /// Whether the pass reads the body of a callee so declared: the calls
    /// to one are the edges of the call graph it walks.
    pub follows: fn(&MirFunctionDeclaration, &MirDeclarations) -> bool,
    /// Rewrite the body, and say whether it changed, as [`BodyPass::run`].
    pub run: fn(&str, &mut MirFunction, &MirDeclarations, &Callees<'_>) -> bool,
}

/// The finished callee bodies a [`CallGraphPass`] reads: the program's own,
/// at the program seam, and the imported package's.
pub struct Callees<'a> {
    program: Option<ProgramCallees<'a>>,
    package: Option<&'a PackageBodies>,
    declarations: &'a MirDeclarations,
    follows: fn(&MirFunctionDeclaration, &MirDeclarations) -> bool,
}

impl Callees<'_> {
    /// The finished body of the callee `name`, when the pass follows it and
    /// it lies on no cycle of followed calls.
    ///
    /// A program callee comes rewritten, since the seam runs callees first.
    /// A package callee is finished on first use, which runs the pass over
    /// it in turn; a package body never calls a program body, so no cycle
    /// crosses the seam. Finishing a body runs every call-graph pass, so a
    /// package callee's cycle is one of the calls any of them follows
    /// ([`followed_by_a_call_graph_pass`]): a cycle that mixes the passes'
    /// edges would otherwise reach the body being finished, and the
    /// package's cycle memo, by row, would answer one pass for another. A
    /// callee whose finish is under way on this thread lies on a cycle too,
    /// one a drop elaboration closed ([`PackageBodies::finishing_body`]).
    pub fn body(&self, name: &str) -> Option<&MirFunction> {
        if let Some(program) = &self.program
            && let Some(&row) = program.rows.get(name)
        {
            return (program.followed.contains(&row) && !program.cyclic.contains(&row))
                .then(|| &program.functions[row].1);
        }
        let package = self.package?;
        if !package.contains(name)
            || !self
                .declarations
                .function(name)
                .is_some_and(|declaration| (self.follows)(declaration, self.declarations))
        {
            return None;
        }
        let declarations = self.declarations;
        if package.on_cycle(name, &|caller, function| {
            followed_callees(caller, function, declarations, |callee| {
                declarations.function(callee).is_some_and(|declaration| {
                    followed_by_a_call_graph_pass(declaration, declarations)
                })
            })
        }) {
            return None;
        }
        if package.finishing_body(name) {
            return None;
        }
        package.get(name)
    }

    /// The finished body of a callee compiled in the caller's own unit, as
    /// [`Self::body`] finds it: the program's at the program seam, never a
    /// package's, as upstream's `RemoveUnusedParams` skips an external
    /// generator; the package's at the package seam.
    pub fn unit_body(&self, name: &str) -> Option<&MirFunction> {
        match &self.program {
            Some(program) if !program.rows.contains_key(name) => None,
            _ => self.body(name),
        }
    }
}

/// A registered pass, by the unit it rewrites.
pub enum Pass {
    Program(ProgramPass),
    Body(BodyPass),
    CallGraph(CallGraphPass),
}

impl Pass {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Program(pass) => pass.name,
            Self::Body(pass) => pass.name,
            Self::CallGraph(pass) => pass.name,
        }
    }
}

/// The pre-elaboration pipeline, in order: upstream's pass rounds
/// (`docs/pre-elaboration-optimization-plan.md`) as Mojito adopts them.
///
/// Upstream runs four rounds, each opening with `RemoveUnusedParams` and
/// `EliminateDeadSymbols` (the second after an `InlineParametric`), then a
/// generator pipeline: `SROA`, `Mem2Reg`, and `Canonicalizer` in the first;
/// the same with `SCCP` before the `Canonicalizer` in the second; an
/// `InlineParametric`, then `SROA`, `Mem2Reg`, and `Canonicalizer` in the
/// third; and `CSEPass` in the fourth. Mojito has `Mem2Reg` (as `mem-2-reg`)
/// but neither `SROA` nor a CSE pass, so its fourth round has no generator
/// pipeline.
pub const PIPELINE: &[Pass] = &[
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(MEM_2_REG),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(INLINE_PARAMETRIC),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(MEM_2_REG),
    Pass::Body(SCCP),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::CallGraph(INLINE_PARAMETRIC),
    Pass::Body(MEM_2_REG),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
];

/// Drop elaboration as a named pass, so a pass test can run it over MIR
/// text whose drops are not yet inserted. The pipeline does not list it:
/// every pipeline body already had its drops inserted.
pub const ELABORATE_DROPS: BodyPass = BodyPass {
    name: "elaborate-drops",
    run: |name, function, declarations| {
        *function = elaborate_drops_function(name, std::mem::take(function), declarations);
        true
    },
};

/// Structured control flow lowered to blocks: upstream's `LowerControlFlow`.
///
/// Upstream lowers `hlcf` as it reaches LLVM. The ownership check runs
/// this pass on each body after its move scan, so the pipeline does not
/// list it: no body it reaches holds a region op.
pub const LOWER_CONTROL_FLOW: BodyPass = BodyPass {
    name: "lower-control-flow",
    run: |_, function, _| {
        mojito_mir::mir::lower_control_flow(function);
        true
    },
};

/// The pass registered under `name`: a [`PIPELINE`] pass, or one run only
/// on request (drop elaboration, `lower-lit`, control-flow lowering, and
/// the post-elaboration argument lowering).
pub fn pass_named(name: &str) -> Option<&'static Pass> {
    PIPELINE
        .iter()
        .chain(REGISTERED)
        .find(|pass| pass.name() == name)
}

/// Every registered pass's name once, in the order the pipeline first runs
/// it, then the passes run only on request.
pub fn pass_names() -> impl Iterator<Item = &'static str> {
    let mut seen = std::collections::HashSet::new();
    PIPELINE
        .iter()
        .chain(REGISTERED)
        .map(Pass::name)
        .filter(move |name| seen.insert(*name))
}

/// Run [`PIPELINE`]'s body and call-graph passes over the body `name` of
/// the package `bodies`.
///
/// In debug builds the body is verified after each pass, and each finding,
/// naming the pass, goes to `findings`.
///
/// A program pass has nothing to do here: a package body is finished only
/// when the elaborator first reaches it, so no finished body is dead.
pub fn run_pipeline(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    bodies: &PackageBodies,
    findings: &mut Vec<String>,
) {
    for pass in PIPELINE {
        match pass {
            Pass::Program(_) => {}
            Pass::Body(pass) => run_body_pass(pass, name, function, declarations, findings),
            Pass::CallGraph(pass) => {
                let callees = Callees {
                    program: None,
                    package: Some(bodies),
                    declarations,
                    follows: pass.follows,
                };
                if (pass.run)(name, function, declarations, &callees) {
                    verify_after(pass.name, name, function, declarations, findings);
                }
            }
        }
    }
}

/// The program seam: [`PIPELINE`] over `program`, its findings folded into
/// `invariant_errors`.
///
/// Each program pass runs once, given `exports`, the symbols the
/// compilation exports beside its entry roots; each body pass over every
/// function but `__toplevel__`; and each call-graph pass over the same
/// functions callees first, reading callees the program does not define
/// from `package`.
pub fn pre_elaborate_program(
    program: &mut MirProgram,
    package: Option<&PackageBodies>,
    exports: &[String],
) {
    for pass in PIPELINE {
        match pass {
            Pass::Program(pass) => {
                let _pass = timing::span(pass.name);
                (pass.run)(program, exports);
            }
            Pass::CallGraph(pass) => {
                let _pass = timing::span(pass.name);
                for row in run_call_graph_pass(pass, program, package) {
                    let MirProgram {
                        functions,
                        declarations,
                        invariant_errors,
                    } = &mut *program;
                    let (name, function) = &functions[row];
                    verify_after(pass.name, name, function, declarations, invariant_errors);
                }
            }
            Pass::Body(pass) => {
                let _pass = timing::span(pass.name);
                let MirProgram {
                    functions,
                    declarations,
                    invariant_errors,
                } = &mut *program;
                for (name, function) in functions.iter_mut() {
                    // Module-scope (`__toplevel__`) variables keep their final
                    // values for the CLI/`bindings()` global dump, so no pass
                    // rewrites them.
                    if name != "__toplevel__" {
                        run_body_pass(pass, name, function, declarations, invariant_errors);
                    }
                }
            }
        }
    }
}

/// The package seam's finishing hook: [`elaborate_drops_function`], then
/// [`lower_lit`], then [`run_pipeline`].
pub fn finish_package_body(
    name: &str,
    function: MirFunction,
    declarations: &MirDeclarations,
    bodies: &PackageBodies,
    findings: &mut Vec<String>,
) -> MirFunction {
    let mut function = elaborate_drops_function(name, function, declarations);
    lower_lit(name, &mut function, declarations);
    run_pipeline(name, &mut function, declarations, bodies, findings);
    function
}

/// [`finish_package_body`] with the pipeline switched off: drops and
/// [`lower_lit`] only.
pub fn finish_package_body_without_pipeline(
    name: &str,
    function: MirFunction,
    declarations: &MirDeclarations,
    _bodies: &PackageBodies,
    _findings: &mut Vec<String>,
) -> MirFunction {
    let mut function = elaborate_drops_function(name, function, declarations);
    lower_lit(name, &mut function, declarations);
    function
}

/// A verifier finding after the pass `pass` ran, naming the pass.
pub(super) fn pass_finding(pass: &str, finding: &str) -> String {
    format!("after pre-elaboration pass `{pass}`: {finding}")
}

/// Run the call-graph pass `pass` over every function of `program` but
/// `__toplevel__`, each after the callees it follows, and return the rows
/// of the functions it changed.
///
/// The order is the call graph's strongly connected components bottom-up,
/// as MLIR's inliner walks them; a function in a component of more than
/// one, or calling itself, is no callee's body ([`Callees::body`]).
pub(super) fn run_call_graph_pass(
    pass: &CallGraphPass,
    program: &mut MirProgram,
    package: Option<&PackageBodies>,
) -> Vec<usize> {
    let MirProgram {
        functions,
        declarations,
        ..
    } = program;
    let rows: HashMap<String, usize> = functions
        .iter()
        .enumerate()
        .map(|(row, (name, _))| (name.clone(), row))
        .collect();
    let followed: HashSet<usize> = declarations
        .functions
        .iter()
        .filter(|declaration| (pass.follows)(declaration, declarations))
        .filter_map(|declaration| rows.get(&declaration.lowered_name).copied())
        .collect();
    let (order, cyclic) = bottom_up_order(functions, declarations, &rows, &followed);
    let mut changed = Vec::new();
    for row in order {
        if functions[row].0 == "__toplevel__" {
            continue;
        }
        let mut function = std::mem::take(&mut functions[row].1);
        let callees = Callees {
            program: Some(ProgramCallees {
                functions,
                rows: &rows,
                followed: &followed,
                cyclic: &cyclic,
            }),
            package,
            declarations,
            follows: pass.follows,
        };
        if (pass.run)(&functions[row].0, &mut function, declarations, &callees) {
            changed.push(row);
        }
        functions[row].1 = function;
    }
    changed.sort_unstable();
    changed
}

/// The program's own bodies, as a [`CallGraphPass`] reads them.
struct ProgramCallees<'a> {
    functions: &'a [(String, MirFunction)],
    rows: &'a HashMap<String, usize>,
    /// The rows of the functions the pass follows.
    followed: &'a HashSet<usize>,
    /// The rows of the followed functions on a cycle of followed calls.
    cyclic: &'a HashSet<usize>,
}

/// Passes outside [`PIPELINE`] that [`pass_named`] still answers.
const REGISTERED: &[Pass] = &[
    Pass::Body(ELABORATE_DROPS),
    Pass::Body(LOWER_LIT),
    Pass::Body(LOWER_CONTROL_FLOW),
    Pass::Body(LOWER_ARG_CONVENTIONS),
    Pass::Body(CSE),
    Pass::Program(ELIMINATE_DUPLICATE_FUNCTIONS),
];

/// Run the body pass `pass` over the body `name`, verifying it after the
/// pass in debug builds when the pass changed it.
fn run_body_pass(
    pass: &BodyPass,
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) {
    if (pass.run)(name, function, declarations) {
        verify_after(pass.name, name, function, declarations, findings);
    }
}

/// In debug builds, verify the body `name` the pass `pass` changed, each
/// finding, naming the pass, going to `findings`.
fn verify_after(
    pass: &str,
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) {
    if cfg!(debug_assertions) {
        findings.extend(
            mojito_mir::mir::verify::verify_function_in(name, function, declarations)
                .into_iter()
                .map(|finding| pass_finding(pass, &finding)),
        );
    }
}

/// The callees `function`, the body of `name`, calls directly, by a call,
/// a resolved method call, a subscript call, a resolved operator, or an
/// iteration step, through a closure or a function value it makes, or as
/// the declared lifecycle member a drop, copy, or move runs
/// ([`mojito_mir::mir::lifecycle_callees`]), and the lifted bodies
/// it or its signature names
/// ([`mojito_mir::mir::named_lifted_bodies`]), that `followed` accepts,
/// each once, in its regions too.
fn followed_callees(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    followed: impl Fn(&str) -> bool,
) -> Vec<String> {
    fn walk(blocks: &[mojito_mir::mir::MirBlock], out: &mut Vec<String>) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::Call { func, .. } => out.push(func.0.clone()),
                    MirInstr::MethodCall {
                        resolved: Some(target),
                        ..
                    }
                    | MirInstr::BinOp {
                        resolved: Some(target),
                        ..
                    } => out.push(target.clone()),
                    MirInstr::GetIter { prepare, .. } => out.extend(prepare.iter().cloned()),
                    MirInstr::TryNext { call, .. } => out.push(call.target.clone()),
                    MirInstr::MakeClosure { function, .. }
                    | MirInstr::Const {
                        k: Const::Function(function),
                        ..
                    } => out.push(function.clone()),
                    _ => out.extend(
                        mojito_mir::mir::instruction_subscript_call(instruction)
                            .map(|call| call.target.clone()),
                    ),
                }
                for region in instruction_regions(instruction) {
                    walk(region, out);
                }
            }
        }
    }
    let mut callees = Vec::new();
    walk(&function.blocks, &mut callees);
    callees.extend(mojito_mir::mir::lifecycle_callees(function, declarations));
    callees.extend(mojito_mir::mir::named_lifted_bodies(
        name,
        function,
        declarations,
    ));
    let mut seen = HashSet::new();
    callees.retain(|callee| followed(callee) && seen.insert(callee.clone()));
    callees
}

/// Whether some [`PIPELINE`] call-graph pass follows a callee so declared.
fn followed_by_a_call_graph_pass(
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
) -> bool {
    PIPELINE.iter().any(|pass| match pass {
        Pass::CallGraph(pass) => (pass.follows)(declaration, declarations),
        _ => false,
    })
}

/// The rows of `functions` callees first along the calls to `followed`
/// rows (Tarjan's strongly connected components, which come out bottom-up),
/// and the followed rows on a cycle of such calls.
fn bottom_up_order(
    functions: &[(String, MirFunction)],
    declarations: &MirDeclarations,
    rows: &HashMap<String, usize>,
    followed: &HashSet<usize>,
) -> (Vec<usize>, HashSet<usize>) {
    struct Tarjan<'a> {
        edges: Vec<Vec<usize>>,
        index: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        order: Vec<usize>,
        cyclic: HashSet<usize>,
        followed: &'a HashSet<usize>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, row: usize) {
            self.index[row] = Some(self.next);
            self.lowlink[row] = self.next;
            self.next += 1;
            self.stack.push(row);
            self.on_stack[row] = true;
            for callee in self.edges[row].clone() {
                match self.index[callee] {
                    None => {
                        self.visit(callee);
                        self.lowlink[row] = self.lowlink[row].min(self.lowlink[callee]);
                    }
                    Some(index) if self.on_stack[callee] => {
                        self.lowlink[row] = self.lowlink[row].min(index);
                    }
                    Some(_) => {}
                }
            }
            if Some(self.lowlink[row]) == self.index[row] {
                let mut component = Vec::new();
                while let Some(member) = self.stack.pop() {
                    self.on_stack[member] = false;
                    component.push(member);
                    if member == row {
                        break;
                    }
                }
                if component.len() > 1 || self.edges[row].contains(&row) {
                    self.cyclic.extend(
                        component
                            .iter()
                            .filter(|member| self.followed.contains(member)),
                    );
                }
                self.order.extend(component);
            }
        }
    }
    let edges = functions
        .iter()
        .map(|(name, function)| {
            followed_callees(name, function, declarations, |callee| {
                rows.get(callee).is_some_and(|row| followed.contains(row))
            })
            .iter()
            .map(|callee| rows[callee])
            .collect()
        })
        .collect();
    let mut tarjan = Tarjan {
        edges,
        index: vec![None; functions.len()],
        lowlink: vec![0; functions.len()],
        on_stack: vec![false; functions.len()],
        stack: Vec::new(),
        next: 0,
        order: Vec::with_capacity(functions.len()),
        cyclic: HashSet::new(),
        followed,
    };
    for row in 0..functions.len() {
        if tarjan.index[row].is_none() {
            tarjan.visit(row);
        }
    }
    (tarjan.order, tarjan.cyclic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_pass_resolves_by_a_unique_name() {
        let names: Vec<_> = pass_names().collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for name in names {
            assert_eq!(pass_named(name).map(Pass::name), Some(name));
        }
        assert!(pass_named("elaborate-drops").is_some());
        assert!(pass_named("eliminate-dead-symbols").is_some());
        assert!(pass_named("canonicalize").is_some());
        assert!(pass_named("sccp").is_some());
        assert!(pass_named("remove-unused-params").is_some());
        assert!(pass_named("inline-parametric").is_some());
        assert!(pass_named("lower-control-flow").is_some());
        assert!(pass_named("mem-2-reg").is_some());
        assert!(pass_named("lower-arg-conventions").is_some());
        assert!(pass_named("cse").is_some());
        assert!(pass_named("eliminate-duplicate-functions").is_some());
        assert!(pass_named("no-such-pass").is_none());
    }
}
