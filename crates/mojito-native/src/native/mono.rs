//! The elaborator: MIR monomorphization for every backend.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph, which the VM and the native backend both
//! consume. It never mutates the canonical MIR artifact.

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use equiv::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use promote::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use substitute::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use symbolic::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use unify::*;

use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirClosureCapture, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirProgram, MirStructDeclaration, Reg,
};
use mojito_symbol::symbol::{CallableCandidate, InstanceArg};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{ParamBindings, ParamContext, ParamExpr, ParamRef};
use mojito_types::types::{ParamDecl, Ty, TyArg};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParametricInstances {
    /// Parametric bodies in the source program, reachable or not.
    pub bodies: usize,
    /// Those the entries reach.
    pub reached: usize,
    /// The concrete functions substituted from the reached ones.
    pub instances: usize,
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
pub fn specialize(
    program: &MirProgram,
    entries: &[String],
) -> Result<SpecializedProgram, MonoError> {
    Specializer::new(program).run(entries)
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

#[derive(Clone, PartialEq, Eq)]
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
}

struct Specializer<'a> {
    source: &'a MirProgram,
    functions: HashMap<&'a str, &'a MirFunction>,
    declarations: HashMap<&'a str, &'a MirFunctionDeclaration>,
    structs: HashMap<&'a str, &'a MirStructDeclaration>,
    generic_templates: Rc<HashSet<String>>,
    queue: VecDeque<(InstanceKey, Bindings)>,
    instances: Vec<(InstanceKey, String)>,
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
    /// Instance names enqueued only by struct discovery's eager `__init__`
    /// walk, never by a call site. A conditional constructor (`__init__(out
    /// self) where conforms_to(Self.T, Defaultable)`) has no MIR-visible
    /// clause, so discovery over-approximates; an instance that cannot
    /// materialize is dropped instead of rejecting the program, exactly
    /// because the checker admitted no call to it.
    speculative: HashSet<String>,
}

mod equiv;
mod infer;
mod instances;
mod promote;
mod specializer;
mod substitute;
mod symbolic;
mod unify;

#[cfg(test)]
mod tests;
