//! The elaborator: MIR monomorphization for every backend.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph, which the VM and the native backend both
//! consume. It never mutates the canonical MIR artifact.

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use equiv::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use promote::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use spread::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use substitute::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use symbolic::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use unify::*;

use availability::Availability;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirClosureCapture, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirProgram, MirStructDeclaration, MirTerm, Reg,
};
use mojito_native_core::target::NativeTarget;
use mojito_symbol::symbol::{CallableCandidate, InstanceArg};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{
    ParamBindings, ParamContext, ParamExpr, ParamKind, ParamRef, SIZE_OF_FUNCTION,
};
use mojito_types::types::{
    ConstraintOperand, GenericConstraint, ParamDecl, SimdDtype, SimdWidth, Ty, TyArg,
};
use mojito_vm::backend::VmBackend;

/// A concrete program and the concrete identity of every requested public
/// entry.
#[derive(Debug, Clone)]
pub struct SpecializedProgram {
    pub program: ConcreteMir,
    pub entries: HashMap<String, String>,
    pub parametric: ParametricInstances,
}

/// What the specialization instantiated from parametric bodies: the source
/// functions whose types name a parameter, which reach MIR once and run
/// erased on the VM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParametricInstances {
    /// Parametric bodies in the source program, reachable or not.
    pub bodies: usize,
    /// The parametric body each concrete function was substituted from, one
    /// entry per instance: the distinct names are the bodies the entries
    /// reach.
    pub instance_templates: Vec<String>,
}

/// A source-template-oriented specialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoError {
    pub function: Option<String>,
    pub construct: String,
}

impl std::fmt::Display for MonoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.function {
            Some(function) => write!(f, "in `{function}`: unsupported {}", self.construct),
            None => write!(f, "unsupported {}", self.construct),
        }
    }
}

impl std::error::Error for MonoError {}

/// Specialize the graph reachable from `entries` without modifying `program`.
///
/// `target` is the native target the elaboration answers layout queries
/// for; a program that asks none elaborates without one.
pub fn specialize(
    program: &MirProgram,
    entries: &[String],
    target: Option<&NativeTarget>,
) -> Result<SpecializedProgram, MonoError> {
    // Expanding polymorphic recursion nests each instance's types one level
    // deeper than the last, and the type walks recurse on that nesting, so
    // the elaborator runs on a stack deep enough to reach the instance
    // budget rather than overflow on the way.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("elaborator".to_string())
            .stack_size(ELABORATOR_STACK_BYTES)
            .spawn_scoped(scope, || Specializer::new(program, target).run(entries))
            .map_or_else(
                |_| Specializer::new(program, target).run(entries),
                |elaborator| {
                    elaborator
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                },
            )
    })
}

/// The entry roots of a whole program.
///
/// They are `main` and the module initializer `__toplevel__`, each when the
/// program defines it. Source execution, artifact execution, and a native
/// executable all elaborate from these; any other root is an entry a native
/// caller requests by name.
pub fn entry_roots(program: &MirProgram) -> Vec<String> {
    ["main", "__toplevel__"]
        .into_iter()
        .filter(|entry| program.functions.iter().any(|(name, _)| name == entry))
        .map(str::to_string)
        .collect()
}

/// The names of the parametric bodies of `program`: the functions whose
/// types name a parameter.
pub fn parametric_bodies(program: &MirProgram) -> HashSet<&str> {
    program
        .functions
        .iter()
        .filter(|(_, function)| function_types(function).any(is_symbolic))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The most instances one elaboration demands. It is the elaborator's only
/// bound — there is no instantiation-depth limit, as upstream — and it stops
/// expanding polymorphic recursion (`f[W[T]]` calling `f[W[W[T]]]`), which
/// would otherwise never terminate.
const INSTANCE_BUDGET: usize = 4096;

/// Where an instance stands on the worklist.
///
/// A reference edge (a call in a body) enqueues and never waits, so
/// recursion through references is valid; a demand edge (a compile-time
/// application the elaborator must evaluate now) materializes the instance
/// at once, and a demand on an `Active` instance is upstream's "function
/// instantiation in parameter domain that recursively requires itself".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceState {
    Pending,
    Active,
    Completed,
    Failed,
}

/// The per-function scratch state `materialize` overwrites, saved around a
/// nested materialization a compile-time demand performs.
#[derive(Default)]
struct FunctionFrame {
    constant_values: HashMap<u32, CtValue>,
    callable_targets: HashMap<u32, (String, bool)>,
    closure_captures: HashMap<String, Vec<MirClosureCapture>>,
    enclosing: Bindings,
    folded_slots: HashSet<u32>,
}

/// The elaborator thread's stack: enough for the type walks at the nesting
/// depth the instance budget admits. Untouched pages are never committed.
const ELABORATOR_STACK_BYTES: usize = 256 << 20;

#[derive(Clone, PartialEq, Eq, Hash)]
struct InstanceKey {
    template: String,
    arguments: Vec<InstanceArg>,
    /// The concrete owner instance of a generic struct's method (for example
    /// `List$mono$TInt` for `List.grow`). Methods carry their owner's identity
    /// here rather than in `arguments`, which hold only the method's own
    /// generic parameters.
    owner: Option<String>,
}

#[derive(Clone, Default)]
struct Bindings {
    /// Each type binder's solution, keyed by the binder's identity: two
    /// binders sharing a spelling are two entries.
    types: HashMap<ParamRef, Ty>,
    /// Each value binder's solution, keyed by identity as `types` is.
    values: HashMap<ParamRef, CtValue>,
    /// The callee a retained callable runtime parameter's argument names,
    /// keyed by the parameter: a runtime parameter is a local of the
    /// signature, not a binder.
    callables: HashMap<String, String>,
    associated: HashMap<String, Ty>,
    /// When materializing a generic struct's method: the owner's template name
    /// and its concrete instance type. Substitution rewrites the bare in-body
    /// `self` spelling (`Struct(template, [])`) to the concrete instance so
    /// nested method calls can bind the owner's parameters from the receiver.
    self_instance: Option<(String, Ty)>,
    /// The names of every generic struct template in the source program.
    /// Substitution renames a concrete application of one of these to its
    /// instance symbol; checker-specialized structs with empty `param_decls`
    /// (the `Tuple$tN` family) keep their names.
    generic_templates: Rc<HashSet<String>>,
    /// Each source struct's parameters and unparameterized associated types.
    /// Substitution solves `C.Element` from them once `C` is bound to an
    /// instance, where no signature spelled the member for unification.
    associated_types: Rc<HashMap<String, AssociatedTypes>>,
    /// The call-site arity of an unspecialized variadic callee: substitution
    /// rewrites `VariadicPack(T)` into the concrete `RuntimePack([T'; n])`.
    variadic_arity: Option<usize>,
    /// Callable parameters whose bound argument is a closure with captures.
    /// The environment survives no name, so the instance takes the closure as
    /// a runtime parameter and its body keeps the indirect call, instead of
    /// folding the callable's name into a direct one.
    runtime_callables: Vec<ParamRef>,
    /// The enclosing value parameters a lifted body's instance folds in
    /// place of its leading captures, by name, in capture order.
    folded_captures: Vec<(String, CtValue)>,
    /// What answers a layout application (`size_of[T]()`) a type of the
    /// instance carries: the target, and the struct declarations known when
    /// the instance was materialized. `None` where no layout is answered.
    layout: Option<Rc<LayoutOracle>>,
}

/// The layout answers available to one instance's substitution.
struct LayoutOracle {
    target: NativeTarget,
    structs: mojito_native_core::layout::StructFieldIndex,
}

impl LayoutOracle {
    /// The size of the concrete `ty`, or the layout error naming why not.
    fn size_of(&self, ty: &Ty) -> Result<i64, MonoError> {
        let layout = mojito_native_core::layout::LayoutCx {
            target: &self.target,
            structs: &self.structs,
        };
        layout
            .layout_of(ty)
            .map(|layout| layout.size as i64)
            .map_err(|error| MonoError {
                function: None,
                construct: format!("size_of of `{ty}`: {error}"),
            })
    }
}

/// A struct's own parameters and its associated types over them.
struct AssociatedTypes {
    param_decls: Vec<ParamDecl>,
    members: Vec<(String, Ty)>,
}

struct Specializer<'a> {
    source: &'a MirProgram,
    /// The target every layout query is answered for.
    target: Option<&'a NativeTarget>,
    functions: HashMap<&'a str, &'a MirFunction>,
    declarations: HashMap<&'a str, &'a MirFunctionDeclaration>,
    structs: HashMap<&'a str, &'a MirStructDeclaration>,
    generic_templates: Rc<HashSet<String>>,
    associated_types: Rc<HashMap<String, AssociatedTypes>>,
    queue: VecDeque<(InstanceKey, Bindings)>,
    instances: Vec<(InstanceKey, String)>,
    /// Each demanded key's position in `instances`.
    instance_index: HashMap<InstanceKey, usize>,
    /// The state of each instance, by its position in `instances`.
    states: Vec<InstanceState>,
    /// The instances a compile-time evaluation is waiting on, outermost
    /// first: a demand on one of them is a cycle in the parameter domain.
    demand_stack: Vec<String>,
    /// Every compile-time application evaluated so far, by instance name:
    /// the same application under the same bindings runs once.
    evaluations: HashMap<String, CtValue>,
    /// The compile-time execution steps left to this compilation.
    fuel: usize,
    /// The executor of compile-time applications, as upstream's elaborator
    /// owns its interpreter.
    vm: VmBackend,
    /// The struct types [`Specializer::discover_structs`] has walked: a type
    /// met again in a later body has nothing left to discover.
    discovered_types: HashSet<Ty>,
    output_functions: Vec<(String, MirFunction)>,
    output_function_decls: Vec<MirFunctionDeclaration>,
    output_structs: Vec<MirStructDeclaration>,
    constant_values: HashMap<u32, CtValue>,
    callable_targets: HashMap<u32, (String, bool)>,
    /// The environment of each lifted body closed over in the function being
    /// specialized (see [`equiv::function_closure_captures`]).
    closure_captures: HashMap<String, Vec<MirClosureCapture>>,
    /// The bindings of the instance being specialized: an erased body
    /// forwarding its own binder as a callee's type argument
    /// (`hash[Self.H](key)`), or building a value argument from its own
    /// value binders (`successor[n, 1 + n]()`), resolves them here.
    enclosing: Bindings,
    /// The slots of the function being specialized that hold a folded value
    /// parameter. A snapshot of one never differs from the slot, so a
    /// direct call to a generic nested `def` may pass the slot itself.
    folded_slots: HashSet<u32>,
}

mod availability;
mod equiv;
mod infer;
mod instances;
mod promote;
mod specializer;
mod spread;
mod substitute;
mod symbolic;
mod unify;
mod unroll;

#[cfg(test)]
mod tests;
