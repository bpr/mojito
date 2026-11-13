//! The pre-elaboration pipeline: passes over parametric MIR whose drops are
//! inserted, run before the elaborator instantiates it, as upstream runs its
//! pre-elaboration pass pipeline on parametric IR after destructor
//! insertion.
//!
//! It runs at two seams: over a program's own bodies
//! ([`pre_elaborate_program`]), where a [`ProgramPass`] runs once over the
//! whole program, a [`BodyPass`] over each body, and a [`CallGraphPass`]
//! over each body callees first, and over each imported package body as it
//! is finished ([`finish_package_drops`], then [`finish_package_body`]),
//! where the body and call-graph passes run. Never per instance.

use super::automatic_inline::{ALWAYS_INLINE, AUTOMATIC_INLINE};
use super::canonicalize::CANONICALIZE;
use super::cse::CSE;
use super::dead_symbols::ELIMINATE_DEAD_SYMBOLS;
use super::duplicate_functions::ELIMINATE_DUPLICATE_FUNCTIONS;
use super::elaborate_drops_function;
use super::hoist_trivial_invariants::HOIST_TRIVIAL_INVARIANTS;
use super::inline_parametric::INLINE_PARAMETRIC;
use super::lower_lit::{LOWER_LIT, lower_lit};
use super::mem2reg::{IMM_ARGUMENTS, LOWER_ARG_CONVENTIONS, MEM_2_REG};
use super::sccp::SCCP;
use super::simplify_cf::SIMPLIFY_CF;
use super::sroa::SROA;
use super::stack_reuse::STACK_REUSE;
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
/// on a cycle of such calls has no body to read ([`Callees::body`]), or,
/// by its [`Cycles`], none from a caller on the same cycle, or none when it
/// only reaches such a cycle.
pub struct CallGraphPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    /// Whether the pass reads the body of a callee so declared: the calls
    /// to one are the edges of the call graph it walks.
    pub follows: fn(&MirFunctionDeclaration, &MirDeclarations) -> bool,
    /// Which program callees on a cycle of followed calls have no body.
    pub cycles: Cycles,
    /// Rewrite the body, and say whether it changed, as [`BodyPass::run`].
    pub run: fn(&str, &mut MirFunction, &MirDeclarations, &Callees<'_>) -> bool,
}

/// Which program callees on a cycle of followed calls a [`CallGraphPass`]
/// reads no body of.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cycles {
    /// Every one, as upstream's `RemoveUnusedParams` reads none.
    Refused,
    /// Every one, and every callee that reaches one along followed calls,
    /// as upstream's `InlineParametric` reads none: a callee whose body
    /// still calls into a cycle stays a call.
    Reached,
    /// Only one in the caller's own strongly connected component, as
    /// upstream's `AutomaticInline` (`canInlineCallee`) inlines a recursive
    /// callee into a caller outside its cycle.
    CallerComponent,
}

/// The finished callee bodies a [`CallGraphPass`] reads: the program's own,
/// at the program seam, and the imported package's.
pub struct Callees<'a> {
    program: Option<ProgramCallees<'a>>,
    package: Option<&'a PackageBodies>,
    declarations: &'a MirDeclarations,
    follows: fn(&MirFunctionDeclaration, &MirDeclarations) -> bool,
    /// The pass's name, which keys the package's memo of the bodies that
    /// reach a cycle along the pass's own calls.
    pass: &'static str,
    cycles: Cycles,
}

impl Callees<'_> {
    /// The finished body of the callee `name`, when the pass follows it and
    /// it lies on no cycle of followed calls.
    ///
    /// A program callee comes rewritten, since the seam runs callees first;
    /// one on a cycle has none, or, under [`Cycles::CallerComponent`], none
    /// when the cycle holds the caller, or, under [`Cycles::Reached`], none
    /// when it reaches a cycle either.
    /// A package callee is finished on first use, which runs the pass over
    /// it in turn; a package body never calls a program body, so no cycle
    /// crosses the seam. Finishing a body runs every call-graph pass, so a
    /// package callee's cycle is one of the calls any of them follows
    /// ([`followed_by_a_call_graph_pass`]): a cycle that mixes the passes'
    /// edges would otherwise reach the body being finished, and the
    /// package's cycle memo, by row, would answer one pass for another. A
    /// callee whose finish is under way on this thread lies on a cycle too,
    /// one a pass's rewrite closed ([`PackageBodies::finishing_body`]).
    pub fn body(&self, name: &str) -> Option<&MirFunction> {
        if let Some(program) = &self.program
            && let Some(&row) = program.rows.get(name)
        {
            let readable = match program.cycles {
                Cycles::Refused => !program.cyclic.contains(&row),
                Cycles::Reached => !program.reaching.contains(&row),
                Cycles::CallerComponent => {
                    program.components[row] != program.components[program.caller]
                }
            };
            return (program.followed.contains(&row) && readable)
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
        let successors = |caller: &str, function: &MirFunction, declarations: &MirDeclarations| {
            followed_callees(caller, function, declarations, |callee| {
                declarations.function(callee).is_some_and(|declaration| {
                    followed_by_a_call_graph_pass(declaration, declarations)
                })
            })
        };
        let refused = match self.cycles {
            Cycles::Reached => package.reaches_cycle(name, self.pass, &successors, &|callee| {
                self.declarations
                    .function(callee)
                    .is_some_and(|declaration| (self.follows)(declaration, self.declarations))
            }),
            Cycles::Refused | Cycles::CallerComponent => package.on_cycle(name, &successors),
        };
        if refused {
            return None;
        }
        if package.finishing_body(name) {
            return None;
        }
        package.get(name)
    }

    /// Whether the body [`Self::body`] answers for `name` is one of the
    /// program's, which the seam ran this pass's earlier rounds over
    /// beside the caller, rather than a package body finished whole.
    pub fn is_program_body(&self, name: &str) -> bool {
        self.program
            .as_ref()
            .is_some_and(|program| program.rows.contains_key(name))
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
/// third; and `CSEPass` in the fourth. Mojito runs each of them, as `sroa`,
/// `mem-2-reg`, `sccp`, `canonicalize`, and `cse`, after `imm-arguments`,
/// which gives each register-passable `read` argument the one value
/// upstream's parser passes it as.
pub const PIPELINE: &[Pass] = &[
    Pass::Body(IMM_ARGUMENTS),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(SROA),
    Pass::Body(MEM_2_REG),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(INLINE_PARAMETRIC),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(SROA),
    Pass::Body(MEM_2_REG),
    Pass::Body(SCCP),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::CallGraph(INLINE_PARAMETRIC),
    Pass::Body(SROA),
    Pass::Body(MEM_2_REG),
    Pass::Body(CANONICALIZE),
    Pass::CallGraph(REMOVE_UNUSED_PARAMS),
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(CSE),
];

/// Drop elaboration as a named pass, so a pass test can run it over MIR
/// text whose drops are not yet inserted.
///
/// The pipeline does not list it: every pipeline body already had its drops
/// inserted. Its output is structured, as every pipeline pass reads it.
pub const ELABORATE_DROPS: BodyPass = BodyPass {
    name: "elaborate-drops",
    run: |name, function, declarations| {
        *function = elaborate_drops_function(name, std::mem::take(function), declarations);
        true
    },
};

/// The pass registered under `name`: a [`PIPELINE`] pass, or one run only
/// on request (drop elaboration, `lower-lit`, and the post-elaboration
/// passes).
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
                    pass: pass.name,
                    cycles: pass.cycles,
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
                let changed = run_call_graph_pass(
                    pass,
                    &mut program.functions,
                    &program.declarations,
                    package,
                );
                for row in changed {
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

/// The package seam's finishing hook: [`run_pipeline`] over a body
/// [`finish_package_drops`] made.
///
/// The body keeps its structured control flow, which the elaborator
/// instantiates as it is.
pub fn finish_package_body(
    name: &str,
    mut function: MirFunction,
    declarations: &MirDeclarations,
    bodies: &PackageBodies,
    findings: &mut Vec<String>,
) -> MirFunction {
    run_pipeline(name, &mut function, declarations, bodies, findings);
    function
}

/// The package seam's drops hook: a lowered body's drops inserted and its
/// lifetime-only markers erased.
///
/// It runs [`elaborate_drops_function`], then [`lower_lit`]; the pipeline
/// and [`PackageBodies::on_cycle`]'s walk both read the body it makes.
pub fn finish_package_drops(
    name: &str,
    function: MirFunction,
    declarations: &MirDeclarations,
) -> MirFunction {
    let mut function = elaborate_drops_function(name, function, declarations);
    lower_lit(name, &mut function, declarations);
    function
}

/// A verifier finding after the pass `pass` ran, naming the pass.
pub(super) fn pass_finding(pass: &str, finding: &str) -> String {
    format!("after pre-elaboration pass `{pass}`: {finding}")
}

/// Run the call-graph pass `pass` over every function of `functions`, a
/// program declared by `declarations`, but `__toplevel__`, each after the
/// callees it follows, and return the rows of the functions it changed.
///
/// The order is the call graph's strongly connected components bottom-up,
/// as MLIR's inliner walks them; a function in a component of more than
/// one, or calling itself, is no callee's body, or no body to a caller in
/// its component, and under [`Cycles::Reached`] neither is one whose
/// followed calls reach such a function ([`Callees::body`]).
pub(super) fn run_call_graph_pass(
    pass: &CallGraphPass,
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    package: Option<&PackageBodies>,
) -> Vec<usize> {
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
    let BottomUp {
        order,
        cyclic,
        reaching,
        components,
    } = bottom_up_order(functions, declarations, &rows, &followed);
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
                reaching: &reaching,
                components: &components,
                caller: row,
                cycles: pass.cycles,
            }),
            package,
            declarations,
            follows: pass.follows,
            pass: pass.name,
            cycles: pass.cycles,
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
    /// The rows of the followed functions whose followed calls reach a
    /// cycle of them, the cyclic rows included.
    reaching: &'a HashSet<usize>,
    /// The strongly connected component of each row.
    components: &'a [usize],
    /// The row of the function the pass is rewriting.
    caller: usize,
    cycles: Cycles,
}

/// Passes outside [`PIPELINE`] that [`pass_named`] still answers.
const REGISTERED: &[Pass] = &[
    Pass::Body(ELABORATE_DROPS),
    Pass::Body(LOWER_LIT),
    Pass::Body(LOWER_ARG_CONVENTIONS),
    Pass::Body(SIMPLIFY_CF),
    Pass::Body(HOIST_TRIVIAL_INVARIANTS),
    Pass::Body(STACK_REUSE),
    Pass::CallGraph(AUTOMATIC_INLINE),
    Pass::CallGraph(ALWAYS_INLINE),
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
    fn walk(block: &mojito_mir::mir::MirBlock, out: &mut Vec<String>) {
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
                MirInstr::MakeClosure { function, .. }
                | MirInstr::Const {
                    k: Const::Function(function) | Const::FunctionInstance { function, .. },
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
    let mut callees = Vec::new();
    walk(&function.body, &mut callees);
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

/// The rows of a program callees first along the calls to followed rows.
struct BottomUp {
    order: Vec<usize>,
    /// The followed rows on a cycle of followed calls.
    cyclic: HashSet<usize>,
    /// The followed rows whose followed calls reach such a cycle, the
    /// cyclic rows included.
    reaching: HashSet<usize>,
    /// The strongly connected component of each row.
    components: Vec<usize>,
}

/// The rows of `functions` callees first along the calls to `followed`
/// rows (Tarjan's strongly connected components, which come out bottom-up),
/// the followed rows on a cycle of such calls, the followed rows reaching
/// one, and each row's component.
fn bottom_up_order(
    functions: &[(String, MirFunction)],
    declarations: &MirDeclarations,
    rows: &HashMap<String, usize>,
    followed: &HashSet<usize>,
) -> BottomUp {
    struct Tarjan<'a> {
        edges: Vec<Vec<usize>>,
        index: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        order: Vec<usize>,
        cyclic: HashSet<usize>,
        reaching: HashSet<usize>,
        components: Vec<usize>,
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
                    self.components[member] = row;
                    component.push(member);
                    if member == row {
                        break;
                    }
                }
                let on_cycle = component.len() > 1 || self.edges[row].contains(&row);
                let followed: Vec<usize> = component
                    .iter()
                    .copied()
                    .filter(|member| self.followed.contains(member))
                    .collect();
                if on_cycle {
                    self.cyclic.extend(followed.iter().copied());
                }
                // Every callee outside the component popped first, so its
                // answer is final.
                if on_cycle
                    || component.iter().any(|member| {
                        self.edges[*member]
                            .iter()
                            .any(|callee| self.reaching.contains(callee))
                    })
                {
                    self.reaching.extend(followed);
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
        reaching: HashSet::new(),
        components: vec![0; functions.len()],
        followed,
    };
    for row in 0..functions.len() {
        if tarjan.index[row].is_none() {
            tarjan.visit(row);
        }
    }
    BottomUp {
        order: tarjan.order,
        cyclic: tarjan.cyclic,
        reaching: tarjan.reaching,
        components: tarjan.components,
    }
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
        assert!(pass_named("sroa").is_some());
        assert!(pass_named("mem-2-reg").is_some());
        assert!(pass_named("lower-arg-conventions").is_some());
        assert!(pass_named("cse").is_some());
        assert!(pass_named("eliminate-duplicate-functions").is_some());
        assert!(pass_named("automatic-inline").is_some());
        assert!(pass_named("no-such-pass").is_none());
    }

    #[test]
    fn a_package_cycle_walk_follows_the_destructor_a_drop_runs() {
        let input = include_bytes!(
            "../../../../tests/mir_passes/elaborate-drops/destructor_closes_cycle.mir"
        );
        let program = mojito_mir::mir::text::load_artifact(input, "destructor_closes_cycle.mir")
            .expect("the case loads")
            .program;
        let declarations = &program.declarations;
        let lowered = |name: &str| {
            program
                .functions
                .iter()
                .find(|(function, _)| function == name)
                .map(|(_, function)| function.clone())
                .expect("the case holds the function")
        };
        let callees = |name: &str, function: &MirFunction| {
            followed_callees(name, function, declarations, |_| true)
        };
        // `touch`'s lowered body names no destructor: its drop of `s` does.
        assert!(!callees("touch", &lowered("touch")).contains(&"S.__deinit__".to_string()));
        let dropped = finish_package_drops("touch", lowered("touch"), declarations);
        assert!(callees("touch", &dropped).contains(&"S.__deinit__".to_string()));
        let destructor =
            finish_package_drops("S.__deinit__", lowered("S.__deinit__"), declarations);
        assert!(callees("S.__deinit__", &destructor).contains(&"touch".to_string()));
    }
}
