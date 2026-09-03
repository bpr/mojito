//! Backend-private MIR monomorphization.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph. It never mutates the canonical MIR artifact.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirProgram, MirStructDeclaration, Reg,
};
use mojito_symbol::symbol::{CallableCandidate, InstanceArg};
use mojito_types::ct::{CtExpr, CtValue};
use mojito_types::types::{DependentType, ParamDecl, Ty, TyArg};

/// A fully concrete backend-private program and the concrete identity of every
/// requested public entry.
pub struct SpecializedProgram {
    pub program: MirProgram,
    pub entries: HashMap<String, String>,
}

/// A source-template-oriented specialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoError {
    pub function: Option<String>,
    pub construct: String,
}

/// Specialize the graph reachable from `entries` without modifying `program`.
pub fn specialize(
    program: &MirProgram,
    entries: &[String],
) -> Result<SpecializedProgram, MonoError> {
    Specializer::new(program).run(entries)
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
    types: HashMap<String, Ty>,
    values: HashMap<String, CtValue>,
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
}

mod equiv;
mod infer;
mod instances;
mod specializer;
mod substitute;
mod symbolic;
mod unify;

use equiv::*;
use substitute::*;
use symbolic::*;
use unify::*;

#[cfg(test)]
mod tests;
