//! The elaborator: MIR monomorphization for every backend.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph, which the VM and the native backend both
//! consume. It never mutates the canonical MIR artifact.

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use equiv::*;
use mojito_mir::mir::{
    addressed_slots, destroyed_structs, relocated_structs, renumber_slots, retire_slots,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use promote::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use spread::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use substitute::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use symbolic::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use unify::*;

use availability::Availability;
use catalog::{Catalog, StructCatalog};
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_mir::mir::text::package::PackageBodies;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirClosureCapture, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirProgram, MirStructDeclaration, MirTerm, Reg,
};
use mojito_native_core::layout::StructFieldIndex;
use mojito_native_core::target::NativeTarget;
use mojito_symbol::symbol::{CallableLookup, InstanceArg};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{
    PackQuery, ParamBindings, ParamContext, ParamExpr, ParamKind, ParamRef, SIZE_OF_FUNCTION,
};
use mojito_types::types::{
    ConstraintOperand, GenericConstraint, ParamDecl, SimdDtype, SimdWidth, Ty, TyArg, TyKind,
};
use mojito_vm::backend::{ConcreteProgram, VmBackend};

/// A concrete program and the concrete identity of every requested public
/// entry.
#[derive(Debug, Clone)]
pub struct SpecializedProgram {
    pub program: ConcreteMir,
    pub entries: HashMap<String, String>,
    pub counts: ElaborationCounts,
}

impl SpecializedProgram {
    /// Run `rewrite` over the concrete program's bodies, re-admit its
    /// result as concrete, and recount it: the seam a post-elaboration
    /// pipeline runs at, as upstream nests its per-function pipeline after
    /// `ElaborateGenerators`. `rewrite` returns the index of each body it
    /// changed, and only those are verified again
    /// ([`ConcreteMir::rewrite_bodies`]).
    pub fn rewritten(
        self,
        rewrite: impl FnOnce(&mut [(String, MirFunction)], &MirDeclarations) -> Vec<usize>,
    ) -> Result<Self, MonoError> {
        let Self {
            program,
            entries,
            mut counts,
        } = self;
        let program = program
            .rewrite_bodies(rewrite)
            .map_err(|findings| MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!(
                    "concrete MIR that does not verify after the post-elaboration pipeline: {}",
                    findings.join("; ")
                ),
            })?;
        counts.recount(&program);
        Ok(Self {
            program,
            entries,
            counts,
        })
    }

    /// Run `eliminate`, a symbol pass that removes functions and
    /// declarations, those its roots do not reach or those identical to a
    /// function that stays, over the concrete program, rooted at `exports`
    /// and every requested entry's instance, and recount it: the seam
    /// upstream's `EliminateDuplicateFunctions` and `EliminateDeadSymbols`
    /// run at straight out of the elaborator, and `EliminateDeadSymbols`
    /// again after its post-elaboration pipeline ([`ConcreteMir::pruned`]).
    pub fn pruned(
        self,
        exports: &[String],
        eliminate: impl FnOnce(&mut MirProgram, &[String]),
    ) -> Result<Self, MonoError> {
        let roots: Vec<String> = exports
            .iter()
            .chain(self.entries.values())
            .cloned()
            .collect();
        let Self {
            program,
            entries,
            mut counts,
        } = self;
        let program = program
            .pruned(|program| eliminate(program, &roots))
            .map_err(|findings| MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!(
                    "concrete MIR that does not verify after symbol elimination: {}",
                    findings.join("; ")
                ),
            })?;
        counts.recount(&program);
        Ok(Self {
            program,
            entries,
            counts,
        })
    }
}

/// What the elaboration multiplied: each generator it reached and the
/// instances it made of it, and the size of the concrete output. These are
/// the elaborator's pass statistics; the driver reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElaborationCounts {
    /// One row per reached generator, sorted by template name.
    pub generators: Vec<GeneratorCount>,
    /// Instructions in the whole concrete program.
    pub concrete_instructions: usize,
    /// Blocks in the whole concrete program.
    pub concrete_blocks: usize,
}

impl ElaborationCounts {
    /// The generator instances the elaboration made, over every generator.
    pub fn generator_instances(&self) -> usize {
        self.generators.iter().map(|row| row.instances).sum()
    }

    /// The instructions of the reached generators' template bodies, each
    /// counted once.
    pub fn generator_instructions(&self) -> usize {
        self.generators
            .iter()
            .map(|row| row.template_instructions)
            .sum()
    }

    /// The instructions of every generator instance in the concrete output.
    pub fn generator_instance_instructions(&self) -> usize {
        self.generators
            .iter()
            .map(|row| row.instance_instructions)
            .sum()
    }

    /// The blocks of the reached generators' template bodies, each counted
    /// once.
    pub fn generator_blocks(&self) -> usize {
        self.generators.iter().map(|row| row.template_blocks).sum()
    }

    /// The blocks of every generator instance in the concrete output.
    pub fn generator_instance_blocks(&self) -> usize {
        self.generators.iter().map(|row| row.instance_blocks).sum()
    }

    /// Recount the concrete instructions and blocks, every generator's
    /// instances' and the whole program's, over `program`, a rewrite of the
    /// elaboration's output. An instance the rewrite removed leaves its
    /// generator's row, and a generator left with none leaves the counts.
    /// Template sizes stay as the elaborator read them.
    pub fn recount(&mut self, program: &MirProgram) {
        let output: HashMap<&str, &MirFunction> = program
            .functions
            .iter()
            .map(|(name, function)| (name.as_str(), function))
            .collect();
        for row in &mut self.generators {
            row.instance_names
                .retain(|name| output.contains_key(name.as_str()));
            row.instances = row.instance_names.len();
        }
        self.generators.retain(|row| row.instances > 0);
        for row in &mut self.generators {
            let instances = || {
                row.instance_names
                    .iter()
                    .filter_map(|name| output.get(name.as_str()))
            };
            row.instance_instructions = instances().map(|f| f.instruction_count()).sum();
            row.instance_blocks = instances().map(|f| f.block_count()).sum();
        }
        let functions = || program.functions.iter().map(|(_, function)| function);
        self.concrete_instructions = functions().map(MirFunction::instruction_count).sum();
        self.concrete_blocks = functions().map(MirFunction::block_count).sum();
    }
}

/// One generator: a template the elaboration instantiated under a name of
/// its own (`twice` as `twice$mono$TInt`). A body instantiated under its
/// template's name is a plain function, emitted once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratorCount {
    pub template: String,
    pub instances: usize,
    /// Instructions of the template body, as the elaborator read it.
    pub template_instructions: usize,
    /// Instructions of its instances in the concrete output, summed.
    pub instance_instructions: usize,
    /// Blocks of the template body, as the elaborator read it.
    pub template_blocks: usize,
    /// Blocks of its instances in the concrete output, summed.
    pub instance_blocks: usize,
    /// Its instances' names in the concrete output.
    pub instance_names: Vec<String>,
}

/// A source-template-oriented specialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoError {
    pub kind: MonoErrorKind,
    pub function: Option<String>,
    /// The construct the elaborator cannot instantiate, or the reason the
    /// instance fails.
    pub construct: String,
}

/// Whose failure a [`MonoError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonoErrorKind {
    /// A construct the elaborator does not instantiate yet.
    Unsupported,
    /// An instance the program demands fails, as the pin's function
    /// instantiation does (`rebind input type 'String' does not match
    /// result type 'Int'`).
    Instantiation,
    /// A package body the elaboration materializes failed verification
    /// when its importer finished it.
    Verify,
    /// A package declaration the elaboration reached does not decode.
    Decode,
}

impl std::fmt::Display for MonoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let construct = &self.construct;
        match (&self.function, self.kind) {
            (Some(function), MonoErrorKind::Unsupported) => {
                write!(f, "in `{function}`: unsupported {construct}")
            }
            (None, MonoErrorKind::Unsupported) => write!(f, "unsupported {construct}"),
            (Some(function), MonoErrorKind::Instantiation) => write!(
                f,
                "function instantiation of `{function}` failed: {construct}"
            ),
            (None, MonoErrorKind::Instantiation) => {
                write!(f, "function instantiation failed: {construct}")
            }
            (Some(function), MonoErrorKind::Verify) => {
                write!(
                    f,
                    "package body `{function}` fails verification: {construct}"
                )
            }
            (None, MonoErrorKind::Verify) => {
                write!(f, "a package body fails verification: {construct}")
            }
            (Some(function), MonoErrorKind::Decode) => {
                write!(f, "in `{function}`: {construct}")
            }
            (None, MonoErrorKind::Decode) => write!(f, "{construct}"),
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
    specialize_importing(program, None, entries, target)
}

/// [`specialize`] for a program that imports a compiled package: a callee
/// the program does not define is the package's, whose body is decoded the
/// first time the elaboration reaches it.
pub fn specialize_importing(
    program: &MirProgram,
    package: Option<&PackageBodies>,
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
            .spawn_scoped(scope, || {
                Specializer::new(program, target)
                    .importing(package)
                    .run(entries)
            })
            .map_or_else(
                |_| {
                    Specializer::new(program, target)
                        .importing(package)
                        .run(entries)
                },
                |elaborator| {
                    elaborator
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                },
            )
    })
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
    /// Whether the instance is of the generator a direct call names, with
    /// its template's unused arguments ([`MirFunction::removed_args`])
    /// removed: upstream's `_REMOVED_ARG` clone. Every other use of such a
    /// template instantiates the full generator, keyed by every parameter.
    reduced: bool,
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
    /// Each source struct's parameters, unparameterized associated types,
    /// and fields. Substitution renames a concrete application of a generic
    /// template to its instance symbol, and solves `C.Element` from them
    /// once `C` is bound to an instance, where no signature spelled the
    /// member for unification, and a reflection query reads the fields.
    structs: Rc<StructCatalog>,
    /// The call-site arity of an unspecialized variadic callee: substitution
    /// rewrites `VariadicPack(T)` into the concrete `RuntimePack([T'; n])`.
    variadic_arity: Option<usize>,
    /// Callable parameters whose bound argument is a closure with captures.
    /// The environment survives no name, so the instance takes the closure as
    /// a runtime parameter and its body keeps the indirect call, instead of
    /// folding the callable's name into a direct one.
    runtime_callables: Vec<ParamRef>,
    /// Callable parameters bound to a generic closure with captures, each
    /// beside the closure's lifted body and environment. The instance folds
    /// the body's name, as for a callable that captures nothing, and takes
    /// the environment alone: one trailing reference parameter per captured
    /// place, typed here, which a call through the parameter passes to the
    /// body's specialization.
    callable_environments: Vec<(ParamRef, String, Vec<Ty>)>,
    /// What answers a layout application (`size_of[T]()`) a type of the
    /// instance carries: the target, and the struct declarations known when
    /// the layout is asked. `None` where no layout is answered.
    layout: Option<Rc<LayoutOracle>>,
    /// What answers the application of a lowered function a type or a
    /// parameter argument of the instance carries (`SIMD[dt, L[0]]` over a
    /// local display binding). `None` where no application is answered.
    applications: Option<Rc<Applications>>,
    /// The parameters the instance's identity does not hold: those its
    /// generator removed, which stay bound for the forwarded arguments that
    /// still evaluate them, and those an erased struct instance's
    /// `_REMOVED_ARG` arguments leave unbound. A struct argument that
    /// mentions one is erased ([`CtValue::RemovedArg`]), as upstream's
    /// generator spells it, so one instance takes every struct agreeing on
    /// the kept arguments.
    removed: Vec<ParamRef>,
    /// Whether a direct call demands the instance, which takes the reduced
    /// generator when its template removed an argument
    /// ([`InstanceKey::reduced`]).
    reduced: bool,
}

impl Bindings {
    /// Erase every origin the bound types carry, as upstream binds a type
    /// parameter to a type whose origins read `origin={ }`. The instance key
    /// names these erased types, so a binding keeps none of the origins of
    /// whichever caller first demanded the instance: those name the caller's
    /// own binders and places, which mean nothing in the callee. The
    /// instance's own origin binders are not bindings and stay symbolic, as
    /// upstream keeps an instance's origin parameters unspecialized.
    fn erase_origins(&mut self) {
        fn erase_value(value: &mut CtValue) {
            match value {
                CtValue::Type(ty) => *ty = origin_erased(ty),
                CtValue::Tuple(elements) => elements.iter_mut().for_each(erase_value),
                _ => {}
            }
        }
        self.types
            .values_mut()
            .chain(self.associated.values_mut())
            .chain(self.self_instance.iter_mut().map(|(_, ty)| ty))
            .for_each(|ty| *ty = origin_erased(ty));
        self.values.values_mut().for_each(erase_value);
    }
}

/// What a call passes for the callable parameters it binds to closures that
/// capture ([`Specializer::bind_capturing_callables`]).
#[derive(Default)]
struct CapturingArguments {
    /// Each closure passed whole, in the order its instance promotes them.
    closures: Vec<Reg>,
    /// Each captured place a generic closure's instance takes by reference,
    /// after every closure, beside the register its load fills.
    environment: Vec<(Reg, MirPlace)>,
}

/// The compile-time applications of lowered functions the elaboration's
/// substitutions have met at constant arguments, each keyed by its function
/// and arguments: the values demanded so far, and the applications a
/// substitution met with no value yet, which the specializer demands before
/// it substitutes again ([`Specializer::answer_pending`]).
#[derive(Default)]
struct Applications {
    /// The program's own lowered functions.
    declared: HashSet<String>,
    /// The package whose functions are lowered too.
    imported: Option<mojito_mir::mir::link::ImportedDeclarations>,
    answered: RefCell<HashMap<(String, Vec<ParamExpr>), CtValue>>,
    pending: RefCell<Vec<(String, Vec<ParamExpr>)>>,
}

impl Applications {
    /// The value of `function` applied to `args`, recording the application
    /// as pending when it is a lowered function's at constant arguments
    /// that nothing has run yet.
    fn answer(&self, function: &str, args: &[ParamExpr]) -> Option<CtValue> {
        let declared = self.declared.contains(function)
            || self
                .imported
                .as_ref()
                .is_some_and(|imported| imported.package().contains_function(function));
        if !declared || args.iter().any(|arg| arg.as_constant().is_none()) {
            return None;
        }
        let key = (function.to_string(), args.to_vec());
        let answer = self.answered.borrow().get(&key).cloned();
        if answer.is_none() {
            let mut pending = self.pending.borrow_mut();
            if !pending.contains(&key) {
                pending.push(key);
            }
        }
        answer
    }
}

/// The layout answers available to one instance's substitution.
struct LayoutOracle {
    target: NativeTarget,
    structs: Rc<RefCell<StructFieldIndex>>,
    /// The source structs a layout reaches, joined to `structs` when one
    /// is first asked.
    catalog: Rc<StructCatalog>,
}

impl LayoutOracle {
    /// The size of the concrete `ty`, or the layout error naming why not.
    fn size_of(&self, ty: &Ty) -> Result<i64, MonoError> {
        self.catalog.fill_layout(&self.structs, ty);
        let structs = self.structs.borrow();
        let layout = mojito_native_core::layout::LayoutCx {
            target: &self.target,
            structs: &structs,
        };
        layout
            .layout_of(ty)
            .map(|layout| layout.size as i64)
            .map_err(|error| MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!("size_of of `{ty}`: {error}"),
            })
    }
}

/// A struct's own parameters, and its associated types, compile-time value
/// members, and fields over them.
struct StructShape {
    param_decls: Vec<ParamDecl>,
    members: Vec<(String, Ty)>,
    parameterized: Vec<mojito_checked::declaration::ParameterizedAssociatedType>,
    values: Vec<(String, ParamExpr)>,
    fields: Vec<(String, Ty)>,
}

struct Specializer<'a> {
    source: &'a MirProgram,
    /// The target every layout query is answered for.
    target: Option<&'a NativeTarget>,
    functions: bodies::Bodies<'a>,
    declarations: Catalog<'a, MirFunctionDeclaration>,
    structs: Catalog<'a, MirStructDeclaration>,
    struct_catalog: Rc<StructCatalog>,
    queue: VecDeque<(InstanceKey, Bindings)>,
    instances: Vec<(InstanceKey, String)>,
    /// Each demanded key's position in `instances`.
    instance_index: HashMap<InstanceKey, usize>,
    /// Each instance's position in `instances`, by its name.
    instance_names: HashMap<String, usize>,
    /// The instances each instance's materialization demanded, by its
    /// position in `instances`: the edges a compile-time demand follows to
    /// the fragment it runs.
    references: Vec<Vec<String>>,
    /// The references of each materialization under way, innermost last.
    reference_frames: Vec<Vec<String>>,
    /// The positions in `instances` of each struct instance's lifecycle
    /// members, by the struct's name: what a value of the struct runs
    /// without a call naming it.
    lifecycle_instances: HashMap<String, Vec<usize>>,
    /// The state of each instance, by its position in `instances`.
    states: Vec<InstanceState>,
    /// The instances a compile-time evaluation is waiting on, outermost
    /// first: a demand on one of them is a cycle in the parameter domain.
    demand_stack: Vec<String>,
    /// Every compile-time application evaluated so far, by instance name:
    /// the same application under the same bindings runs once.
    evaluations: HashMap<String, CtValue>,
    /// The prepared fragment each demanded instance runs in, by its name.
    fragments: HashMap<String, Rc<ConcreteProgram>>,
    /// The applications the instances' types and parameter arguments carry
    /// ([`Bindings::applications`]).
    applications: Rc<Applications>,
    /// The compile-time execution steps left to this compilation.
    fuel: usize,
    /// The executor of compile-time applications, as upstream's elaborator
    /// owns its interpreter.
    vm: VmBackend,
    /// The struct types [`Specializer::discover_structs`] has walked: a type
    /// met again in a later body has nothing left to discover.
    discovered_types: HashSet<Ty>,
    output_functions: Vec<(String, MirFunction)>,
    /// Each output function's position in `output_functions`, by name.
    output_positions: HashMap<String, usize>,
    output_function_decls: Vec<MirFunctionDeclaration>,
    output_structs: Vec<MirStructDeclaration>,
    /// Each output struct's position in `output_structs`, by name.
    output_struct_positions: HashMap<String, usize>,
    /// The lifecycle member instances demanded of a struct not yet
    /// declared, by its name: [`Specializer::declare_output_struct`] puts
    /// them into its member table.
    pending_struct_members: HashMap<String, Vec<(&'static str, String)>>,
    /// The field types a layout query reads: each output struct as it is
    /// declared, and each non-generic source struct a query reaches, when
    /// one first does. Every [`LayoutOracle`] shares it, so building one
    /// copies nothing.
    layout_structs: Rc<RefCell<StructFieldIndex>>,
    /// Each concrete type's conformance to a trait, as decided with no
    /// conformance under way: the answer depends on nothing else, so a
    /// nested type's conformance reads its level below's.
    conformance_answers: RefCell<HashMap<(Ty, String), Option<bool>>>,
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
    /// The parameter constants an unrolled iteration of the function being
    /// specialized could not compute, by destination register: each fails
    /// the instantiation once the instance is found to reach it.
    unreached_failures: HashMap<u32, MonoError>,
    /// Each generic nested body declared in an unrolled region copy, under
    /// the name the copy's closure value takes: the template and the copy's
    /// bindings, which a call binding the body's own parameters extends.
    region_generators: HashMap<String, (String, Bindings)>,
}

/// `ty` with every struct origin argument unbound and every pointer origin
/// static: the one origin erasure an instance's key and its bindings share.
/// Origins erase from the runtime ABI and from the instance symbol.
fn origin_erased(ty: &Ty) -> Ty {
    let recur = origin_erased;
    match ty.kind() {
        TyKind::Struct(..) if ty.is_closed() => ty.clone(),
        TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(_) => TyArg::Origin(mojito_types::origin::Origin::Unbound),
                })
                .collect(),
        )),
        TyKind::Pointer { element, .. } => TyKind::Pointer {
            element: recur(element),
            origin: mojito_types::origin::PointerOrigin::Static,
        }
        .into(),
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(recur).collect()).into()
        }
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(recur).collect()).into()
        }
        TyKind::ComptimeList(element) => TyKind::ComptimeList(recur(element)).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(recur(element)).into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recur(&reference.referent);
            TyKind::Ref(reference).into()
        }
        _ => ty.clone(),
    }
}

mod availability;
mod bodies;
mod catalog;
mod equiv;
mod failure;
mod gather;
mod infer;
mod instances;
mod promote;
mod rebind;
mod specializer;
mod spread;
mod substitute;
mod symbolic;
mod unify;
mod unroll;

#[cfg(test)]
mod tests;
