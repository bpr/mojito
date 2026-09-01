//! Backend-private MIR monomorphization.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph. It never mutates the canonical MIR artifact.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::ct::{CtExpr, CtValue};
use crate::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirProgram, MirStructDeclaration, Reg,
};
use crate::symbol::{CallableCandidate, InstanceArg};
use crate::types::{DependentType, ParamDecl, Ty, TyArg};

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

impl<'a> Specializer<'a> {
    fn new(source: &'a MirProgram) -> Self {
        Self {
            source,
            functions: source
                .functions
                .iter()
                .map(|(n, f)| (n.as_str(), f))
                .collect(),
            declarations: source
                .declarations
                .functions
                .iter()
                .map(|d| (d.lowered_name.as_str(), d))
                .collect(),
            structs: source
                .declarations
                .structs
                .iter()
                .map(|d| (d.name.as_str(), d))
                .collect(),
            generic_templates: Rc::new(
                source
                    .declarations
                    .structs
                    .iter()
                    .filter(|d| !d.param_decls.is_empty())
                    .map(|d| d.name.clone())
                    .collect(),
            ),
            queue: VecDeque::new(),
            instances: Vec::new(),
            output_functions: Vec::new(),
            output_function_decls: Vec::new(),
            output_structs: Vec::new(),
            constant_values: HashMap::new(),
            callable_targets: HashMap::new(),
        }
    }

    fn run(mut self, entries: &[String]) -> Result<SpecializedProgram, MonoError> {
        let mut entry_map = HashMap::new();
        for entry in entries {
            let decl = self.declarations.get(entry.as_str()).copied();
            self.functions.get(entry.as_str()).copied().ok_or_else(|| {
                self.error(
                    None,
                    format!("entry function `{entry}` (not found in the MIR program)"),
                )
            })?;
            if decl.is_some_and(|decl| !decl.param_decls.is_empty()) {
                return Err(self.error(
                    Some(entry),
                    format!("generic entry `{entry}` has unresolved parameters"),
                ));
            }
            let name = self.enqueue(entry, self.base_bindings(), Vec::new())?;
            entry_map.insert(entry.clone(), name);
        }
        while let Some((key, bindings)) = self.queue.pop_front() {
            if self
                .output_functions
                .iter()
                .any(|(name, _)| name == self.instance_name(&key))
            {
                continue;
            }
            if self.output_functions.len() >= 4096 {
                return Err(self.error(
                    Some(&key.template),
                    "polymorphic recursion exceeded the 4096-instance budget",
                ));
            }
            self.materialize(key, bindings)?;
        }
        let function_order = self
            .source
            .functions
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.as_str(), index))
            .collect::<HashMap<_, _>>();
        let instance_templates = self
            .instances
            .iter()
            .map(|(key, name)| (name.as_str(), key.template.as_str()))
            .collect::<HashMap<_, _>>();
        self.output_functions.sort_by_key(|(name, _)| {
            let template = instance_templates
                .get(name.as_str())
                .copied()
                .unwrap_or(name);
            function_order.get(template).copied().unwrap_or(usize::MAX)
        });
        self.output_function_decls.sort_by_key(|decl| {
            let template = instance_templates
                .get(decl.lowered_name.as_str())
                .copied()
                .unwrap_or(decl.lowered_name.as_str());
            function_order.get(template).copied().unwrap_or(usize::MAX)
        });
        Ok(SpecializedProgram {
            program: MirProgram {
                functions: self.output_functions,
                declarations: MirDeclarations {
                    structs: self.output_structs,
                    functions: self.output_function_decls,
                },
                invariant_errors: self.source.invariant_errors.clone(),
            },
            entries: entry_map,
        })
    }

    fn base_bindings(&self) -> Bindings {
        Bindings {
            generic_templates: Rc::clone(&self.generic_templates),
            ..Bindings::default()
        }
    }

    fn enqueue(
        &mut self,
        template: &str,
        bindings: Bindings,
        arguments: Vec<InstanceArg>,
    ) -> Result<String, MonoError> {
        let owner = bindings.self_instance.as_ref().and_then(|(_, ty)| {
            if let Ty::Struct(name, _) = ty {
                Some(name.clone())
            } else {
                None
            }
        });

        let key = InstanceKey {
            template: template.to_string(),
            arguments,
            owner,
        };
        if let Some((_, name)) = self.instances.iter().find(|(known, _)| known == &key) {
            return Ok(name.clone());
        }
        // A generic struct's method takes its concrete owner's spelling
        // (`List$mono$TInt.grow`), so lowering's name-composed lifecycle and
        // overload lookups against the instance struct name keep working.
        let name = if let Some(owner) = &key.owner {
            let base = crate::symbol::retarget_method_symbol(template, owner).ok_or_else(|| {
                self.error(
                    Some(template),
                    format!("owner-bound instance `{template}` is not a method symbol"),
                )
            })?;
            if key.arguments.is_empty() {
                base
            } else {
                crate::symbol::instance_symbol(&base, &key.arguments)
            }
        } else if key.arguments.is_empty() {
            template.to_string()
        } else {
            crate::symbol::instance_symbol(template, &key.arguments)
        };
        if (name != template && self.functions.contains_key(name.as_str()))
            || self.instances.iter().any(|(_, n)| n == &name)
        {
            return Err(self.error(
                Some(template),
                format!("concrete instance symbol `{name}` collides with an existing declaration"),
            ));
        }
        self.instances.push((key.clone(), name.clone()));
        self.queue.push_back((key, bindings));
        Ok(name)
    }

    fn instance_name(&self, key: &InstanceKey) -> &str {
        self.instances
            .iter()
            .find(|(known, _)| known == key)
            .expect("queued instance has identity")
            .1
            .as_str()
    }

    fn materialize(&mut self, key: InstanceKey, bindings: Bindings) -> Result<(), MonoError> {
        let name = self.instance_name(&key).to_string();
        let mut function = self
            .functions
            .get(key.template.as_str())
            .copied()
            .ok_or_else(|| {
                self.error(
                    Some(&key.template),
                    format!("callee `{}` has no MIR body", key.template),
                )
            })?
            .clone();
        substitute_function(&mut function, &bindings).map_err(|mut e| {
            e.function.get_or_insert_with(|| key.template.clone());
            e
        })?;
        self.constant_values = function_constant_values(&function);
        self.callable_targets = function_callable_targets(&function);
        self.constant_values.extend(
            self.callable_targets
                .iter()
                .map(|(reg, (target, _))| (*reg, CtValue::Str(target.clone()))),
        );
        // Take the blocks out so call rewriting can read the function's
        // substituted register-type table without aliasing its body.
        let mut blocks = std::mem::take(&mut function.blocks);
        // Iterator normalization first: block order does not put every
        // `GetIter` before the `HasNext`/`Next`/`TryNext` that reads its
        // destination (comprehension loops interleave), and the advance
        // rewrites need the iterator slot types this pass records.
        self.rewrite_iterator_inits(&key.template, &mut function, &mut blocks)?;
        self.rewrite_blocks(&key.template, &mut function, &mut blocks)?;
        function.blocks = blocks;
        repair_storage_result_types(&mut function);
        erase_specialized_generic_callable_storage(&mut function);
        ensure_concrete_function(&key.template, &function)?;

        if let Some(declaration) = self.declarations.get(key.template.as_str()).copied() {
            let mut declaration = declaration.clone();
            substitute_declaration(&mut declaration, &bindings)?;
            declaration.lowered_name = name.clone();
            declaration.param_decls.clear();
            self.output_function_decls.push(declaration);
        }
        self.discover_structs(&key.template, &function)?;
        self.output_functions.push((name, function));
        Ok(())
    }

    fn rewrite_blocks(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for block in blocks {
            for instruction in &mut block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    self.rewrite_blocks(owner, function, body)?;
                    if let Some((_, blocks)) = handler {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    if let Some(blocks) = orelse {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    if let Some(blocks) = finalbody {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    continue;
                }
                // `len`/`abs`/`round` on a nominal receiver are checker-typed
                // dunder dispatches the VM performs by name (`call_dunder`);
                // rewrite them into ordinary method calls so the shared
                // resolver and the `MethodCall` arm below monomorphize the
                // dunder instance like any other method. Non-struct operands
                // pass through for the backend's scalar interception.
                if let MirInstr::Call {
                    dest,
                    func,
                    args,
                    kwargs,
                    ..
                } = instruction
                {
                    let dunder = match func.0.as_str() {
                        "len" => Some("__len__"),
                        "abs" => Some("__abs__"),
                        "round" => Some("__round__"),
                        // Conversion builtins over a nominal receiver are the
                        // same VM dunder dispatch (`builtin_convert`'s struct
                        // arm).
                        "Int" => Some("__int__"),
                        "Float64" => Some("__float__"),
                        "Bool" => Some("__bool__"),
                        _ => None,
                    };
                    if let Some(method) = dunder
                        && !self.functions.contains_key(func.0.as_str())
                        && kwargs.is_empty()
                        && args.len() == 1
                        && matches!(
                            function.reg_types.get(&args[0].0).map(peel_refs),
                            Some(Ty::Struct(..))
                        )
                    {
                        *instruction = dunder_method_call(*dest, args[0], method, None, Vec::new());
                    }
                }
                // A binary operator on a nominal left operand is the same VM
                // dunder dispatch (`apply_binop` → `call_dunder`): rewrite to
                // the operator method so the shared resolver monomorphizes
                // the compiled instance (`String.__add__`, user `__eq__`, …).
                // `in`/`not in` dispatch on the right operand and stay
                // untouched (they keep their contextual rejection).
                if let MirInstr::BinOp {
                    op,
                    dest,
                    a,
                    b,
                    resolved,
                } = instruction
                    && let Some(method) = op.dunder()
                    && !matches!(op, crate::ast::InfixOp::In | crate::ast::InfixOp::NotIn)
                    && matches!(
                        function.reg_types.get(&a.0).map(peel_refs),
                        Some(Ty::Struct(..))
                    )
                {
                    *instruction = dunder_method_call(*dest, *a, method, resolved.take(), vec![*b]);
                }
                match instruction {
                    MirInstr::Call {
                        dest,
                        func,
                        args,
                        kwargs,
                        param_arg_regs,
                        ..
                    } => {
                        if !self.functions.contains_key(func.0.as_str()) {
                            // `print` of a nominal struct displays through
                            // `write_to` over the builtin-string writer (the
                            // VM's `format_value` dispatch); enqueue the
                            // instances the lowered expansion calls.
                            if matches!(func.0.as_str(), "print" | "String") {
                                for arg in args.clone() {
                                    self.enqueue_display_instance(owner, function, arg)?;
                                }
                            }
                            if self.structs.contains_key(func.0.as_str())
                                && !crate::symbol::is_stdlib_string_struct(&func.0)
                            {
                                // `Type(copy=value)` runs `__copyinit__` (the
                                // VM's `construct_via_copy`), which struct
                                // discovery enqueues per instance — never an
                                // `__init__` contract.
                                let copy_form =
                                    args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy";
                                if copy_form {
                                    if let Some(Ty::Struct(concrete, _)) =
                                        function.reg_types.get(&dest.0)
                                        && concrete != &func.0
                                        && nominal_template(concrete) == func.0.as_str()
                                    {
                                        func.0 = concrete.clone();
                                    }
                                    continue;
                                }
                                let init_base = format!("{}.__init__", func.0);
                                let init = crate::symbol::resolve_callable_symbol(
                                    self.functions.iter().map(|(name, f)| CallableCandidate {
                                        name,
                                        n_params: f.n_params,
                                    }),
                                    &init_base,
                                    args.len() + kwargs.len(),
                                );
                                if self.functions.contains_key(init.as_str()) {
                                    let (target, bindings, arguments) = self.infer_call(
                                        owner,
                                        function,
                                        &init,
                                        Some(*dest),
                                        *dest,
                                        args,
                                        kwargs,
                                        param_arg_regs,
                                    )?;
                                    if let Some((_, concrete)) = &bindings.self_instance {
                                        function.reg_types.insert(dest.0, concrete.clone());
                                    }
                                    self.enqueue(&target, bindings, arguments)?;
                                    // The instance identity now carries every
                                    // compile-time solution; the call-site
                                    // value registers are redundant (a body
                                    // that still needs one fails its own
                                    // contextual check).
                                    for param_arg in param_arg_regs.iter_mut() {
                                        param_arg.value = None;
                                    }
                                }
                                // A generic struct's output declaration is
                                // instance-named; respell the constructor
                                // call so lowering's struct lookup matches.
                                if let Some(Ty::Struct(concrete, _)) =
                                    function.reg_types.get(&dest.0)
                                    && concrete != &func.0
                                    && nominal_template(concrete) == func.0.as_str()
                                {
                                    func.0 = concrete.clone();
                                }
                                for param_arg in param_arg_regs.iter_mut() {
                                    param_arg.value = None;
                                }
                            }
                            continue;
                        }
                        // A direct constructor call's destination is its
                        // `out self`, not the declared `None` return — bind
                        // it as the receiver.
                        let receiver = (func.0.contains(".__init__")
                            && function.reg_types.contains_key(&dest.0))
                        .then_some(*dest);
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &func.0,
                            receiver,
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        func.0 = self.enqueue(&target, bindings, arguments)?;
                        for param_arg in param_arg_regs.iter_mut() {
                            param_arg.value = None;
                        }
                    }
                    MirInstr::MakeSimd { elems, .. } => {
                        for elem in elems.clone() {
                            self.enqueue_intable_instance(owner, function, elem)?;
                        }
                    }
                    MirInstr::MethodCall {
                        dest,
                        recv,
                        method,
                        resolved,
                        args,
                        kwargs,
                        param_arg_regs,
                        ..
                    } => {
                        let receiver = function.reg_types.get(&recv.0).ok_or_else(|| {
                            self.error(Some(owner), "method receiver lacks a MIR type")
                        })?;
                        // A scalar/literal Hashable leaf contributes to the
                        // hasher through the hasher's compiled
                        // `_update_with_simd` (a literal through the nominal
                        // String's `__hash__`); enqueue those instances for
                        // the lowered leaf dispatch.
                        if method == "__hash__"
                            && args.len() == 1
                            && kwargs.is_empty()
                            && !matches!(peel_refs(receiver), Ty::Struct(..))
                        {
                            let receiver = peel_refs(receiver).clone();
                            self.enqueue_hash_leaf_instances(owner, function, args[0], &receiver)?;
                        }
                        if resolved
                            .as_deref()
                            .is_some_and(|target| target.starts_with("__trait_dispatch."))
                            && !matches!(peel_refs(receiver), Ty::Struct(..))
                        {
                            *resolved = None;
                            continue;
                        }
                        // `write` on the builtin-string accumulator (the
                        // `Value::Str` writer inside a `write_to` expansion)
                        // formats nominal arguments through their own
                        // `write_to` conformance — enqueue those instances
                        // for the lowered recursion.
                        if method == "write" && matches!(peel_refs(receiver), Ty::StringLiteral) {
                            for arg in args.clone() {
                                self.enqueue_display_instance(owner, function, arg)?;
                            }
                            continue;
                        }
                        // A borrowed receiver dispatches on its referent, as
                        // the VM dereferences `Value::Ref` receivers.
                        let Ty::Struct(receiver_name, _) = peel_refs(receiver) else {
                            continue;
                        };
                        // Source methods are declared under the template name;
                        // an instance-named receiver (`List$mono$TInt`) still
                        // resolves against `List.*` and gets its instance
                        // identity from `infer_call`'s receiver binding.
                        let target = crate::symbol::resolve_method_symbol(
                            self.functions.iter().map(|(name, f)| CallableCandidate {
                                name,
                                n_params: f.n_params,
                            }),
                            nominal_template(receiver_name),
                            method,
                            resolved.as_deref(),
                            args.len() + kwargs.len(),
                        );
                        if !self.functions.contains_key(target.as_str()) {
                            // The VM-synthesized `Writer.write` dispatch calls
                            // the receiver's `write_string`; enqueue its
                            // instance for the lowered expansion.
                            if method == "write" {
                                let write_string = crate::symbol::resolve_callable_symbol(
                                    self.functions.iter().map(|(name, f)| CallableCandidate {
                                        name,
                                        n_params: f.n_params,
                                    }),
                                    &format!("{}.write_string", nominal_template(receiver_name)),
                                    1,
                                );
                                if self.functions.contains_key(write_string.as_str()) {
                                    let receiver_ty = peel_refs(receiver).clone();
                                    let (bindings, arguments, _) = self.infer_receiver_call(
                                        owner,
                                        &write_string,
                                        &receiver_ty,
                                        None,
                                    )?;
                                    self.enqueue(&write_string, bindings, arguments)?;
                                }
                            }
                            continue;
                        }
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &target,
                            Some(*recv),
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *resolved = Some(concrete);
                        for param_arg in param_arg_regs.iter_mut() {
                            param_arg.value = None;
                        }
                    }
                    // An indirect call whose callee is a nominal callable
                    // struct devirtualizes into a direct `__call__` method
                    // call — the VM's `runtime_method_name` dispatch, made
                    // static — so the ordinary method lowering (mut-receiver
                    // write-back, outcome, sret) serves it. Func-typed
                    // callees keep the instruction and lower through their
                    // two-word `{invoke, env}` value.
                    MirInstr::CallIndirect {
                        dest,
                        callee,
                        raises,
                        args,
                        kwargs,
                        callee_place,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        resolved,
                        ..
                    } => {
                        let dependent_callable =
                            function.reg_types.get(&callee.0).is_some_and(|ty| {
                                matches!(
                                    peel_refs(ty),
                                    Ty::GenericFunc { .. }
                                        | Ty::Param {
                                            callable_bound: Some(_),
                                            ..
                                        }
                                )
                            });
                        if let Some((target, captures_are_empty)) =
                            self.callable_targets.get(&callee.0).cloned()
                            && (captures_are_empty || dependent_callable)
                        {
                            if !captures_are_empty {
                                return Err(self.error(
                                    Some(owner),
                                    format!("generic retained callable `{target}` has captures"),
                                ));
                            }
                            let (target, bindings, arguments) = self.infer_call(
                                owner,
                                function,
                                &target,
                                None,
                                *dest,
                                args,
                                kwargs,
                                param_arg_regs,
                            )?;
                            let concrete = self.enqueue(&target, bindings, arguments)?;
                            *instruction = MirInstr::Call {
                                dest: *dest,
                                func: crate::mir::FuncRef(concrete),
                                raises: raises.clone(),
                                args: std::mem::take(args),
                                kwargs: std::mem::take(kwargs),
                                arg_places: std::mem::take(arg_places),
                                kwarg_places: std::mem::take(kwarg_places),
                                capture_accesses: std::mem::take(capture_accesses),
                                param_arg_regs: Vec::new(),
                            };
                            continue;
                        }
                        let Some(receiver) = function.reg_types.get(&callee.0) else {
                            continue;
                        };
                        let Ty::Struct(receiver_name, _) = peel_refs(receiver) else {
                            continue;
                        };
                        let target = crate::symbol::resolve_method_symbol(
                            self.functions.iter().map(|(name, f)| CallableCandidate {
                                name,
                                n_params: f.n_params,
                            }),
                            nominal_template(receiver_name),
                            "__call__",
                            resolved.as_deref(),
                            args.len() + kwargs.len(),
                        );
                        if !self.functions.contains_key(target.as_str()) {
                            continue;
                        }
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &target,
                            Some(*callee),
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *instruction = MirInstr::MethodCall {
                            dest: *dest,
                            recv: *callee,
                            method: "__call__".to_string(),
                            resolved: Some(concrete),
                            raises: raises.clone(),
                            reference_result: None,
                            result_adapter: None,
                            args: std::mem::take(args),
                            kwargs: std::mem::take(kwargs),
                            recv_place: callee_place.take(),
                            recv_writes: true,
                            arg_places: std::mem::take(arg_places),
                            kwarg_places: std::mem::take(kwarg_places),
                            capture_accesses: Vec::new(),
                            param_arg_regs: std::mem::take(param_arg_regs),
                            param_decls: Vec::new(),
                        };
                    }
                    // A retained callable names its lifted body on the
                    // instruction; enqueue it so the reachable graph carries
                    // the compiled target the thunk will call. Lifted bodies
                    // are monomorphic in the supported subset — one whose
                    // signature still spells generic parameters (a lambda
                    // inside an unspecialized generic) rejects contextually.
                    MirInstr::MakeClosure {
                        function: target, ..
                    }
                    | MirInstr::Const {
                        k: Const::Function(target),
                        ..
                    } => {
                        let Some(body) = self.functions.get(target.as_str()).copied() else {
                            continue;
                        };
                        if function_types(body).any(is_symbolic) {
                            continue;
                        }
                        *target = self.enqueue(target, self.base_bindings(), Vec::new())?;
                    }
                    // A value-parameter read (`Self.length`) resolves to
                    // the bound constant carried by the receiver instance's
                    // type arguments — the VM's `get_field` value-parameter
                    // fallback over reified `value_params`.
                    MirInstr::GetField { dest, base, field } => {
                        let Some(receiver) = function.reg_types.get(&base.0) else {
                            continue;
                        };
                        let Some(constant) = self.value_param_constant(receiver, field) else {
                            continue;
                        };
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: constant,
                        };
                    }
                    MirInstr::LoadPlace { dest, place }
                        if place.proj.len() == 1
                            && matches!(&place.proj[0], crate::mir::Proj::Field(_)) =>
                    {
                        let crate::mir::Proj::Field(field) = &place.proj[0] else {
                            continue;
                        };
                        let Some(receiver) = function.var_tys.get(&place.root) else {
                            continue;
                        };
                        let Some(constant) = self.value_param_constant(receiver, field) else {
                            continue;
                        };
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: constant,
                        };
                    }
                    // Checker-selected subscript invocations retarget to
                    // their concrete instances exactly like method calls;
                    // intrinsic storage subscripts carry no nominal call and
                    // pass through.
                    MirInstr::Index {
                        dest,
                        base,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, base) = (*dest, *base);
                        self.rewrite_subscript_call(owner, function, base, Some(dest), call)?;
                    }
                    MirInstr::Slice {
                        dest,
                        object,
                        call: Some(call),
                        ..
                    }
                    | MirInstr::MultiIndex {
                        dest,
                        object,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, object) = (*dest, *object);
                        self.rewrite_subscript_call(owner, function, object, Some(dest), call)?;
                    }
                    MirInstr::MultiSet { receiver, call, .. } => {
                        let receiver = *receiver;
                        self.rewrite_subscript_call(owner, function, receiver, None, call)?;
                    }
                    // An untyped iterator slot passes through: it belongs to
                    // a compiler-private pack loop the backend rejects at its
                    // own boundary.
                    MirInstr::HasNext {
                        iter,
                        method: Some(method),
                        ..
                    } => {
                        if let Some(receiver) = function.var_tys.get(iter).cloned() {
                            let (target, _) = self.resolve_iterator_step(
                                owner,
                                &receiver,
                                "__len__",
                                Some(method),
                                None,
                            )?;
                            *method = target;
                        }
                    }
                    MirInstr::Next {
                        iter,
                        call: Some(call),
                        ..
                    }
                    | MirInstr::TryNext { iter, call, .. } => {
                        if let Some(receiver) = function.var_tys.get(iter).cloned() {
                            let (target, _) = self.resolve_iterator_step(
                                owner,
                                &receiver,
                                "__next__",
                                Some(&call.target),
                                Some(&call.result_ty),
                            )?;
                            call.target = target;
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The constant a value-parameter member read (`Self.length`) resolves
    /// to, when `field` names a value parameter (not a declared field) of
    /// the receiver's template and the instance type carries its solution.
    fn value_param_constant(&self, receiver: &Ty, field: &str) -> Option<Const> {
        let Ty::Struct(name, type_args) = peel_refs(receiver) else {
            return None;
        };
        let struct_decl = self.structs.get(nominal_template(name)).copied()?;
        if struct_decl
            .fields
            .iter()
            .any(|(field_name, _)| field_name == field)
        {
            return None;
        }
        let position = struct_decl
            .param_decls
            .iter()
            .position(|decl| matches!(decl, ParamDecl::Value { name, .. } if name == field))?;
        let TyArg::Val(value) = type_args.get(position)? else {
            return None;
        };
        match value {
            CtValue::Int(v) => Some(Const::Int(*v)),
            CtValue::UInt(v) => Some(Const::Int(*v as i64)),
            CtValue::Bool(v) => Some(Const::Bool(*v)),
            _ => None,
        }
    }

    /// Enqueue the instances a lowered scalar `__hash__(hasher)` leaf calls:
    /// the hasher's `_update_with_simd`, and for a string-literal receiver
    /// the nominal String's `__hash__` bound to that hasher (the VM
    /// materializes the literal and dispatches the same way).
    fn enqueue_hash_leaf_instances(
        &mut self,
        owner: &str,
        function: &MirFunction,
        hasher: Reg,
        receiver: &Ty,
    ) -> Result<(), MonoError> {
        let Some(hasher_ty) = function.reg_types.get(&hasher.0) else {
            return Ok(());
        };
        let hasher_ty = peel_refs(hasher_ty).clone();
        if !matches!(hasher_ty, Ty::Struct(..)) {
            return Ok(());
        }
        self.enqueue_nominal_method_instance(owner, &hasher_ty, "_update_with_simd", 1, &[])?;
        if matches!(receiver, Ty::StringLiteral) {
            let string = Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), Vec::new());
            self.enqueue_nominal_method_instance(
                owner,
                &string,
                "__hash__",
                1,
                &[("H", hasher_ty.clone())],
            )?;
        }
        Ok(())
    }

    /// Enqueue one nominal method instance reached by a lowered intrinsic
    /// rather than an explicit MIR call: the receiver binds the owner's
    /// parameters, `method_bindings` bind the method's own type parameters,
    /// and any remaining type parameter (a `Some[..]` sugar spelling)
    /// instantiates at the builtin string, as `enqueue_display_instance`.
    fn enqueue_nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        method_bindings: &[(&str, Ty)],
    ) -> Result<(), MonoError> {
        let Ty::Struct(name, arguments) = receiver else {
            return Ok(());
        };
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            method,
            None,
            argc,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        let Some(declaration) = self.declarations.get(target.as_str()).copied() else {
            return Ok(());
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)).copied() {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(name) != name.as_str() {
                bindings.self_instance =
                    Some((nominal_template(name).to_string(), receiver.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        for (parameter, ty) in method_bindings {
            bindings.types.insert((*parameter).to_string(), ty.clone());
        }
        for decl in &declaration.param_decls {
            if let ParamDecl::Type { name, .. } = decl
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        for ty in &declaration.param_types {
            if let Ty::Param { name, .. } = ty
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, &target)?;
        arguments.drain(..owner_covered);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Enqueue the `write_to` instance a lowered `print` of a nominal struct
    /// calls — the VM's `format_value` dispatch. The receiver binds the
    /// owner's parameters; the writer parameter instantiates at the builtin
    /// string (the VM's `Value::Str` accumulator).
    fn enqueue_display_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        let Some(ty) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let ty = peel_refs(ty).clone();
        let Ty::Struct(name, arguments) = &ty else {
            return Ok(());
        };
        if crate::symbol::is_stdlib_string_struct(name) {
            return Ok(());
        }
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "write_to",
            None,
            1,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        let Some(declaration) = self.declarations.get(target.as_str()).copied() else {
            return Ok(());
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)).copied() {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(name) != name.as_str() {
                bindings.self_instance = Some((nominal_template(name).to_string(), ty.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        for decl in &declaration.param_decls {
            if let ParamDecl::Type { name, .. } = decl
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        // The `Some[Writer]` sugar parameter is infer-only (absent from
        // `param_decls`); bind its spelling from the declared type.
        for ty in &declaration.param_types {
            if let Ty::Param { name, .. } = ty
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, &target)?;
        arguments.drain(..owner_covered);
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Enqueue a nominal `__int__` reached implicitly by scalar/SIMD
    /// construction. The checker records the concrete operand type on the
    /// element register, while MIR deliberately keeps construction as
    /// `MakeSimd` rather than synthesizing a method call.
    fn enqueue_intable_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        let Some(ty @ Ty::Struct(name, _)) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "__int__",
            None,
            0,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        let (bindings, arguments, _) = self.infer_receiver_call(owner, &target, ty, None)?;
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Retarget one checker-selected subscript invocation (the
    /// `__getitem__`/`__setitem__` family) to its concrete instance. The
    /// receiver binds the owner's parameters and the destination's checked
    /// type anchors the result, mirroring the nullary iterator-step
    /// inference; subscript actuals are `Int` indexes or slice descriptors
    /// and never carry generic solutions of their own.
    fn rewrite_subscript_call(
        &mut self,
        owner: &str,
        function: &MirFunction,
        receiver: Reg,
        dest: Option<Reg>,
        call: &mut crate::mir::MirSubscriptCall,
    ) -> Result<(), MonoError> {
        let Some(receiver_ty) = function.reg_types.get(&receiver.0) else {
            return Ok(());
        };
        let receiver_ty = peel_refs(receiver_ty).clone();
        let Ty::Struct(receiver_name, _) = &receiver_ty else {
            return Ok(());
        };
        let method = if dest.is_some() {
            "__getitem__"
        } else {
            "__setitem__"
        };
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(receiver_name),
            method,
            Some(&call.target),
            call.arguments.len(),
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        // The checker-selected result fact is the authoritative anchor even
        // for a reference result. `unify_result` peels the handle layer, so a
        // bare implicit-view receiver can recover its owner element type from
        // `ref Int` without confusing it with a reference-valued element.
        let result = Some(&call.result_ty);
        let (mut bindings, mut arguments, _) =
            self.infer_receiver_call(owner, &target, &receiver_ty, result)?;
        // A comptime-specialized accessor (`Tuple$tN.__getitem__[i: Int]`)
        // varies by its value parameter: the constant index joins the
        // instance identity — sharing on the receiver alone would collapse
        // same-element-type indexes onto one body — and binds for the
        // instance body's value-parameter reads.
        for (decl, param_arg) in call.param_decls.iter().zip(&call.param_arg_regs) {
            let ParamDecl::Value { name, .. } = decl else {
                continue;
            };
            if bindings.values.contains_key(name.as_str()) {
                continue;
            }
            let value = param_arg
                .value
                .and_then(|reg| const_reg_value(function, reg));
            let Some(value) = value else {
                return Ok(());
            };
            bindings.values.insert(name.clone(), value.clone());
            arguments.push(InstanceArg::Value(value));
        }
        call.target = self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_call(
        &self,
        owner: &str,
        caller: &MirFunction,
        target: &str,
        receiver: Option<Reg>,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        param_args: &[crate::mir::MirParamArg],
    ) -> Result<(String, Bindings, Vec<InstanceArg>), MonoError> {
        let declaration = self.declarations.get(target).copied().ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = self.base_bindings();
        let receiver_pattern_for_instance = receiver.and_then(|_| {
            self.functions
                .get(target)
                .and_then(|function| function.param_types.first())
                .cloned()
        });
        let mut owner_covered = 0;
        if let Some(receiver) = receiver {
            let actual_receiver = peel_refs(reg_ty(caller, receiver, owner)?);
            if let Ty::Struct(receiver_name, arguments) = actual_receiver
                && let Some(struct_decl) =
                    self.structs.get(nominal_template(receiver_name)).copied()
            {
                bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                    self.error(
                        Some(owner),
                        format!("monomorphizing receiver for `{target}`: {e}"),
                    )
                })?;
                // An instance-named receiver carries the owner's concrete
                // identity: record it so the method instance is named under
                // the owner and its body's bare `self` spelling resolves.
                if nominal_template(receiver_name) != receiver_name {
                    bindings.self_instance = Some((
                        nominal_template(receiver_name).to_string(),
                        actual_receiver.clone(),
                    ));
                    owner_covered =
                        owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
                }
            }
            let receiver_pattern = self
                .functions
                .get(target)
                .and_then(|function| function.param_types.first())
                .ok_or_else(|| {
                    self.error(
                        Some(owner),
                        format!("method `{target}` lacks a receiver type"),
                    )
                })?;
            unify(receiver_pattern, actual_receiver, &mut bindings)
                .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        }
        bind_explicit_value_arguments(
            &declaration.param_decls,
            param_args,
            &self.constant_values,
            &mut bindings,
            target,
        )?;
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        let names = &declaration.param_names;
        let required = &declaration.required;
        let slots = match_call_slots(
            names,
            required,
            declaration.positional_only,
            declaration.keyword_only,
            args.len(),
            &kwargs
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            CallVariadics {
                positional: declaration.variadic.is_some(),
                keyword: declaration.kw_variadic.is_some(),
            },
        )
        .map_err(|e| {
            self.error(
                Some(owner),
                format!("binding call to `{target}` during monomorphization: {e:?}"),
            )
        })?;
        let mut callable_arguments = Vec::new();
        for (index, slot) in slots.slots.iter().enumerate() {
            let actual_reg = match slot {
                ArgSlot::Positional(i) => Some(args[*i]),
                ArgSlot::Keyword(i) => Some(kwargs[*i].1),
                ArgSlot::Default => None,
            };
            if let Some(actual_reg) = actual_reg {
                let actual = reg_ty(caller, actual_reg, owner)?;
                // Explicit value parameters may resolve a dependent pattern;
                // an ordinary unresolved type parameter must remain available
                // for structural inference from this runtime argument.
                let pattern = substitute_ty(&declaration.param_types[index], &bindings)
                    .unwrap_or_else(|_| declaration.param_types[index].clone());
                if is_symbolic(&pattern) {
                    unify(&pattern, actual, &mut bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}`: {e}"))
                    })?;
                }
                // Ordinary `Func` parameters carry their closure environment
                // at runtime. Only retained generic-callable parameters are
                // compile-time inputs to instance selection; treating every
                // statically traceable closure as such would discard captures.
                if matches!(
                    peel_refs(&declaration.param_types[index]),
                    Ty::GenericFunc { .. }
                        | Ty::Param {
                            callable_bound: Some(_),
                            ..
                        }
                ) && let Some((callable, captures_are_empty)) =
                    self.callable_targets.get(&actual_reg.0)
                {
                    if !captures_are_empty {
                        return Err(self.error(
                            Some(owner),
                            format!("generic retained callable `{callable}` has captures"),
                        ));
                    }
                    bindings.values.insert(
                        declaration.param_names[index].clone(),
                        CtValue::Str(callable.clone()),
                    );
                    bindings
                        .callables
                        .insert(declaration.param_names[index].clone(), callable.clone());
                    callable_arguments.push(InstanceArg::Value(CtValue::Str(callable.clone())));
                }
            }
        }
        // An unspecialized variadic callee instantiates at its call-site
        // arity: each overflow positional unifies against the pack element
        // and the arity joins the instance identity. Checker-specialized
        // packs (`Tuple$tN`'s concrete `RuntimePack`) keep their identity.
        let variadic_arity = match &declaration.variadic {
            // The declaration records the pack ELEMENT type; a concrete
            // `RuntimePack`/`Tuple` spelling means the checker already
            // specialized the pack (`Tuple$tN`).
            Some(element) if !matches!(element, Ty::RuntimePack(_) | Ty::Tuple(_)) => {
                for index in &slots.positional_overflow {
                    let actual = reg_ty(caller, args[*index], owner)?;
                    unify(element, actual, &mut bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}` pack: {e}"))
                    })?;
                }
                bindings.variadic_arity = Some(slots.positional_overflow.len());
                Some(slots.positional_overflow.len())
            }
            _ => None,
        };
        if receiver != Some(dest)
            && let Some(actual) = caller.reg_types.get(&dest.0)
        {
            unify_result(&declaration.ret_ty, actual, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing `{target}` return: {e}"),
                )
            })?;
        }
        if bindings.self_instance.is_none()
            && let Some(receiver_pattern) = receiver_pattern_for_instance.as_ref()
            && let Ty::Struct(template, arguments) = peel_refs(receiver_pattern)
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)).copied() {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let Some(receiver) = receiver
            && let Ty::Struct(receiver_name, _) = peel_refs(reg_ty(caller, receiver, owner)?)
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
            && !struct_decl.param_decls.is_empty()
        {
            let owner_arguments = ordered_arguments(
                &struct_decl.param_decls,
                &bindings,
                nominal_template(receiver_name),
            )?;
            let ty_arguments = owner_arguments
                .iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => TyArg::Ty(ty.clone()),
                    InstanceArg::Value(value) => TyArg::Val(value.clone()),
                })
                .collect::<Vec<_>>();
            let owner =
                crate::symbol::instance_symbol(nominal_template(receiver_name), &owner_arguments);
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                Ty::Struct(owner, ty_arguments),
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
        // The owner-restating prefix (`__init__` prepends the struct's
        // `param_decls`) is already carried by the instance's `owner`
        // identity; keep only the method's own parameters.
        arguments.drain(..owner_covered);
        if let Some(arity) = variadic_arity {
            arguments.push(InstanceArg::Value(CtValue::Int(arity as i64)));
        }
        arguments.extend(callable_arguments);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        Ok((target.to_string(), bindings, arguments))
    }

    /// Walk `blocks` (recursing into `try` regions) folding every `GetIter`
    /// before the main call rewrite reads iterator slot types.
    fn rewrite_iterator_inits(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for block in blocks {
            for instruction in &mut block.instrs {
                match instruction {
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        self.rewrite_iterator_inits(owner, function, body)?;
                        if let Some((_, blocks)) = handler {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                        if let Some(blocks) = orelse {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                        if let Some(blocks) = finalbody {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                    }
                    MirInstr::GetIter {
                        source,
                        dest,
                        mode: _,
                        prepare,
                    } => {
                        let (source, dest) = (*source, *dest);
                        self.rewrite_get_iter(owner, function, source, dest, prepare)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Fold a `GetIter` normalization chain: retarget every `prepare` step to
    /// its concrete instance, statically unroll dynamic `__trait_dispatch.`
    /// normalization (the VM repeats that step at runtime until the value has a
    /// `__next__`; the receiver is concrete here), and record the chain's final
    /// return type as the iterator variable's type — HIR leaves the split
    /// `$iterobj` slot untyped.
    fn rewrite_get_iter(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        source: crate::hir::VarId,
        dest: crate::hir::VarId,
        prepare: &mut Vec<String>,
    ) -> Result<(), MonoError> {
        let Some(mut current) = function.var_tys.get(&source).cloned() else {
            // An untyped source belongs to a compiler-private pack loop the
            // backend rejects at its own boundary.
            return Ok(());
        };
        // A pack-typed source is the compiler-private pack fallback (the
        // VM's `remove(0)` loop): no nominal protocol resolves. The split
        // slot keeps the pack layout; lowering tracks the advance position
        // in a backend-side shadow slot.
        if matches!(&current, Ty::RuntimePack(_) | Ty::Tuple(_)) {
            function.var_tys.insert(dest, current);
            return Ok(());
        }
        // A borrowed named source binds the slot to a reference; follow it to
        // the underlying iterable type, as the VM does for name resolution.
        if let Ty::Ref(reference) = &current {
            current = (*reference.referent).clone();
        }
        let dispatch = prepare
            .iter()
            .find(|symbol| symbol.starts_with("__trait_dispatch."))
            .cloned();
        for selected in prepare.iter_mut() {
            let (target, result) =
                self.resolve_iterator_step(owner, &current, "__iter__", Some(selected), None)?;
            *selected = target;
            current = result;
        }
        if let Some(selected) = dispatch {
            let mut budget = 8u32;
            while !self.has_iterator_next(&current) {
                if budget == 0 {
                    return Err(self.error(
                        Some(owner),
                        "iterator normalization did not converge within the dispatch budget",
                    ));
                }
                budget -= 1;
                let (target, result) =
                    self.resolve_iterator_step(owner, &current, "__iter__", Some(&selected), None)?;
                prepare.push(target);
                current = result;
            }
        }
        function.var_tys.insert(dest, current);
        Ok(())
    }

    /// Resolve one nullary iterator-protocol operation against a concrete
    /// receiver type, enqueue the target instance, and return its concrete
    /// name plus its substituted result type.
    fn resolve_iterator_step(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        selected: Option<&str>,
        result: Option<&Ty>,
    ) -> Result<(String, Ty), MonoError> {
        let Ty::Struct(receiver_name, _) = receiver else {
            return Err(self.error(
                Some(owner),
                format!("iterator `{method}` operation applied to non-struct type `{receiver}`"),
            ));
        };
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(receiver_name),
            method,
            selected,
            0,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Err(self.error(
                Some(owner),
                format!("iterator method `{target}` is missing from the MIR program"),
            ));
        }
        let (bindings, arguments, result) =
            self.infer_receiver_call(owner, &target, receiver, result)?;
        let concrete = self.enqueue(&target, bindings, arguments)?;
        Ok((concrete, result))
    }

    /// The receiver-typed sibling of [`Self::infer_call`] for nullary method
    /// calls carried by iterator instructions, which name their receiver as a
    /// variable slot rather than a register.
    fn infer_receiver_call(
        &self,
        owner: &str,
        target: &str,
        receiver: &Ty,
        result: Option<&Ty>,
    ) -> Result<(Bindings, Vec<InstanceArg>, Ty), MonoError> {
        let declaration = self.declarations.get(target).copied().ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Ty::Struct(receiver_name, arguments) = receiver
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
        {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(receiver_name) != receiver_name {
                bindings.self_instance = Some((
                    nominal_template(receiver_name).to_string(),
                    receiver.clone(),
                ));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        let receiver_pattern = self
            .functions
            .get(target)
            .and_then(|function| function.param_types.first())
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!("method `{target}` lacks a receiver type"),
                )
            })?;
        unify(receiver_pattern, receiver, &mut bindings)
            .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        if let Some(result) = result {
            unify_result(&declaration.ret_ty, result, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing `{target}` return: {e}"),
                )
            })?;
        }
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        if bindings.self_instance.is_none()
            && let Ty::Struct(template, arguments) = peel_refs(receiver_pattern)
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)).copied() {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let Ty::Struct(receiver_name, _) = receiver
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
            && !struct_decl.param_decls.is_empty()
        {
            let owner_arguments = ordered_arguments(
                &struct_decl.param_decls,
                &bindings,
                nominal_template(receiver_name),
            )?;
            let ty_arguments = owner_arguments
                .iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => TyArg::Ty(ty.clone()),
                    InstanceArg::Value(value) => TyArg::Val(value.clone()),
                })
                .collect::<Vec<_>>();
            let owner =
                crate::symbol::instance_symbol(nominal_template(receiver_name), &owner_arguments);
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                Ty::Struct(owner, ty_arguments),
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
        arguments.drain(..owner_covered);
        let result = substitute_ty(&declaration.ret_ty, &bindings).map_err(|e| {
            self.error(
                Some(owner),
                format!("monomorphizing `{target}` result: {}", e.construct),
            )
        })?;
        Ok((bindings, arguments, result))
    }

    /// Whether the concrete receiver type resolves a nullary `__next__` — the
    /// VM's runtime convergence test for dynamic iterator normalization.
    fn has_iterator_next(&self, receiver: &Ty) -> bool {
        let Ty::Struct(name, _) = receiver else {
            return false;
        };
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "__next__",
            None,
            0,
        );
        self.functions.contains_key(target.as_str())
    }

    fn discover_structs(&mut self, owner: &str, function: &MirFunction) -> Result<(), MonoError> {
        let mut types = function_types(function).cloned().collect::<Vec<_>>();
        // Storage take/destroy intrinsics name their element type directly on
        // the instruction; seed it so the element's lifecycle methods
        // (notably `__deinit__` for the destroy forms) always join the walk
        // even when no register or variable carries the bare element type.
        push_instruction_types(&function.blocks, &mut types);
        while let Some(ty) = types.pop() {
            collect_nested_types(&ty, &mut types);
            let Ty::Struct(name, arguments) = ty else {
                continue;
            };
            // The checker-virtual slice descriptors have no source template;
            // give them the backend's raw layout (three i64 bounds plus a
            // presence bitmask — the VM's `Value::Slice` `Option<i64>` fields)
            // so descriptor-typed parameters and locals lay out.
            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") {
                if !self.output_structs.iter().any(|decl| decl.name == name) {
                    self.output_structs.push(MirStructDeclaration {
                        name,
                        fields: vec![
                            ("start".to_string(), Ty::Int),
                            ("end".to_string(), Ty::Int),
                            ("step".to_string(), Ty::Int),
                            ("flags".to_string(), Ty::Int),
                        ],
                        mut_self_methods: Default::default(),
                        fieldwise_init: false,
                        param_decls: Vec::new(),
                        explicit_destroy_message: None,
                        explicit_destructors: Default::default(),
                    });
                }
                continue;
            }
            let template_name = name.split("$mono").next().unwrap_or(&name).to_string();
            let Some(template) = self.structs.get(template_name.as_str()).copied() else {
                continue;
            };
            if arguments.len() < template.param_decls.len() {
                continue;
            }
            let mut bindings = self.base_bindings();
            bind_ty_args(&template.param_decls, &arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing struct `{template_name}`: {e}"),
                )
            })?;
            if name != template_name {
                bindings.self_instance = Some((
                    template_name.clone(),
                    Ty::Struct(name.clone(), arguments.clone()),
                ));
            }
            let mut declaration = template.clone();
            for (_, field) in &mut declaration.fields {
                *field = substitute_ty(field, &bindings)?;
            }
            declaration.name = name;
            declaration.param_decls.clear();
            // Overload-qualified `mut self` entries name the template
            // (for example, a signature-qualified `List.pop`); respell them
            // under the instance so
            // lowering's receiver write-back check matches the retargeted
            // method symbols. Bare method-name entries stay as they are.
            declaration.mut_self_methods = declaration
                .mut_self_methods
                .iter()
                .map(|entry| {
                    if entry.contains('.') {
                        crate::symbol::retarget_method_symbol(entry, &declaration.name)
                            .unwrap_or_else(|| entry.clone())
                    } else {
                        entry.clone()
                    }
                })
                .collect();
            if let Some(existing) = self
                .output_structs
                .iter()
                .find(|decl| decl.name == declaration.name)
            {
                // Output declarations dedupe by name, but a checker-concrete
                // generic application keeps its template name — two distinct
                // instantiations would silently share whichever declaration
                // was discovered first. Sharing is benign only when the field
                // substitutions are equivalent modulo pointer element types
                // (every pointer is one opaque target word and drops inertly
                // — the `_RawAlloc`/`List` shape); anything else rejects
                // contextually instead of laying out against the wrong
                // instance. Renaming concrete applications to instance
                // symbols is the Collections slice's canonicalization
                // prerequisite.
                if !fields_equivalent(&existing.fields, &declaration.fields) {
                    return Err(self.error(
                        Some(owner),
                        format!(
                            "struct instance `{}` has conflicting field \
                             substitutions (instance identity collision): {:?} versus {:?}",
                            declaration.name, existing.fields, declaration.fields
                        ),
                    ));
                }
                continue;
            }
            types.extend(declaration.fields.iter().map(|(_, ty)| ty.clone()));
            self.output_structs.push(declaration);
            for method in ["__init__", "__copyinit__", "__moveinit__", "__deinit__"] {
                if method == "__copyinit__"
                    && crate::symbol::is_stdlib_string_struct(&template_name)
                {
                    continue;
                }
                let base = format!("{template_name}.{method}");
                let candidates = self
                    .functions
                    .iter()
                    .filter(|(candidate, _)| {
                        **candidate == base || crate::symbol::is_overload_of(candidate, &base)
                    })
                    .map(|(candidate, _)| (*candidate).to_string())
                    .collect::<Vec<_>>();
                for candidate in candidates {
                    let Some(function_decl) = self.declarations.get(candidate.as_str()).copied()
                    else {
                        continue;
                    };
                    // An unspecialized variadic overload cannot materialize
                    // without a call-site arity; those sites enqueue it.
                    if matches!(&function_decl.variadic, Some(element)
                        if !matches!(element, Ty::RuntimePack(_) | Ty::Tuple(_)))
                    {
                        continue;
                    }
                    let Ok(mut method_arguments) =
                        ordered_arguments(&function_decl.param_decls, &bindings, &candidate)
                    else {
                        continue;
                    };
                    if bindings.self_instance.is_some() {
                        let covered =
                            owner_covered_prefix(&template.param_decls, &function_decl.param_decls);
                        method_arguments.drain(..covered);
                    }
                    self.enqueue(&candidate, bindings.clone(), method_arguments)?;
                }
            }
        }
        Ok(())
    }

    fn error(&self, function: Option<&str>, construct: impl Into<String>) -> MonoError {
        MonoError {
            function: function.map(str::to_string),
            construct: construct.into(),
        }
    }
}

/// Field-list equivalence for name-colliding struct instances: strict
/// structural equality except that pointer types collapse (one opaque
/// target word, drop-inert), recursing through nested aggregate shapes.
fn fields_equivalent(a: &[(String, Ty)], b: &[(String, Ty)]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((a_name, a_ty), (b_name, b_ty))| a_name == b_name && ty_equivalent(a_ty, b_ty))
}

fn ty_equivalent(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Pointer { .. }, Ty::Pointer { .. }) => true,
        (Ty::Struct(a_name, a_args), Ty::Struct(b_name, b_args)) => {
            a_name == b_name
                && a_args.len() == b_args.len()
                && a_args.iter().zip(b_args).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => ty_equivalent(a, b),
                    _ => a == b,
                })
        }
        (Ty::Tuple(a), Ty::Tuple(b)) | (Ty::RuntimePack(a), Ty::RuntimePack(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| ty_equivalent(a, b))
        }
        _ => a == b,
    }
}

/// An unresolved method-call shell for a VM by-name dunder dispatch
/// (`call_dunder`): the `MethodCall` rewrite arm resolves and retargets it
/// through the shared resolver like any source-level method call.
/// The compile-time value of a register defined by a `Const` in `function`,
/// when that constant has a `CtValue` form — the resolver for value-parameter
/// arguments spelled as materialized literal registers.
fn const_reg_value(function: &MirFunction, reg: Reg) -> Option<CtValue> {
    const_reg_value_inner(function, reg, &mut HashSet::new())
}

fn function_constant_values(function: &MirFunction) -> HashMap<u32, CtValue> {
    function
        .reg_types
        .keys()
        .filter_map(|reg| const_reg_value(function, Reg(*reg)).map(|value| (*reg, value)))
        .collect()
}

/// Resolve statically named callable values through the MIR's register/variable
/// plumbing. Generic lifted bodies are specialized at their indirect call site;
/// the boolean records whether direct-call rewriting may erase the environment.
fn function_callable_targets(function: &MirFunction) -> HashMap<u32, (String, bool)> {
    fn visit(
        blocks: &[MirBlock],
        registers: &mut HashMap<u32, (String, bool)>,
        variables: &mut HashMap<u32, (String, bool)>,
    ) -> bool {
        let mut changed = false;
        for block in blocks {
            for instruction in &block.instrs {
                let resolved = match instruction {
                    MirInstr::MakeClosure {
                        dest,
                        function,
                        captures,
                    } => Some((dest.0, (function.clone(), captures.is_empty()))),
                    MirInstr::Const {
                        dest,
                        k: Const::Function(function),
                    } => Some((dest.0, (function.clone(), true))),
                    MirInstr::CopyValue { dest, value } => registers
                        .get(&value.0)
                        .cloned()
                        .map(|value| (dest.0, value)),
                    MirInstr::UseVar { dest, var, .. } => {
                        variables.get(var).cloned().map(|value| (dest.0, value))
                    }
                    _ => None,
                };
                if let Some((dest, value)) = resolved
                    && registers.get(&dest) != Some(&value)
                {
                    registers.insert(dest, value);
                    changed = true;
                }
                if let MirInstr::DefVar { var, src, .. } = instruction
                    && let Some(value) = registers.get(&src.0).cloned()
                    && variables.get(var) != Some(&value)
                {
                    variables.insert(*var, value);
                    changed = true;
                }
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    changed |= visit(body, registers, variables);
                    if let Some((_, blocks)) = handler {
                        changed |= visit(blocks, registers, variables);
                    }
                    if let Some(blocks) = orelse {
                        changed |= visit(blocks, registers, variables);
                    }
                    if let Some(blocks) = finalbody {
                        changed |= visit(blocks, registers, variables);
                    }
                }
            }
        }
        changed
    }

    let mut registers = HashMap::new();
    let mut variables = HashMap::new();
    while visit(&function.blocks, &mut registers, &mut variables) {}
    registers
}

fn const_reg_value_inner(
    function: &MirFunction,
    reg: Reg,
    visiting: &mut HashSet<u32>,
) -> Option<CtValue> {
    if !visiting.insert(reg.0) {
        return None;
    }
    for block in &function.blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::Const { dest, k } if *dest == reg => {
                    return match k {
                        Const::Int(value) => Some(CtValue::Int(*value)),
                        Const::IntLiteral(literal) => literal.to_i64().map(CtValue::Int),
                        Const::Bool(value) => Some(CtValue::Bool(*value)),
                        Const::Function(function) => Some(CtValue::Str(function.clone())),
                        _ => None,
                    };
                }
                MirInstr::MaterializeLiteral { dest, value, .. } if *dest == reg => {
                    return const_reg_value_inner(function, *value, visiting);
                }
                _ => {}
            }
        }
    }
    None
}

fn dunder_method_call(
    dest: Reg,
    recv: Reg,
    method: &str,
    resolved: Option<String>,
    args: Vec<Reg>,
) -> MirInstr {
    MirInstr::MethodCall {
        dest,
        recv,
        method: method.to_string(),
        resolved,
        raises: None,
        reference_result: None,
        result_adapter: None,
        args,
        kwargs: Vec::new(),
        recv_place: None,
        recv_writes: false,
        arg_places: Vec::new(),
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: Vec::new(),
    }
}

/// How many leading method `param_decls` restate the owner struct's own
/// parameters: `__init__` declarations prepend them (`src/mir.rs`), and the
/// owner-bound instance identity already carries their solutions.
fn owner_covered_prefix(struct_params: &[ParamDecl], method_params: &[ParamDecl]) -> usize {
    if struct_params.is_empty() || method_params.len() < struct_params.len() {
        return 0;
    }
    if struct_params
        .iter()
        .zip(method_params)
        .all(|(s, m)| s.name() == m.name())
    {
        struct_params.len()
    } else {
        0
    }
}

/// Structural type equality that ignores `ref` and pointer origin components
/// (which erase from the runtime ABI and vary per call site), while still
/// requiring mutability agreement.
fn ty_equal_modulo_origins(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Ref(a), Ty::Ref(b)) => {
            a.mutability == b.mutability && ty_equal_modulo_origins(&a.referent, &b.referent)
        }
        (Ty::Pointer { element: a, .. }, Ty::Pointer { element: b, .. }) => {
            ty_equal_modulo_origins(a, b)
        }
        (Ty::Struct(a_name, a_args), Ty::Struct(b_name, b_args)) => {
            a_name == b_name
                && a_args.len() == b_args.len()
                && a_args.iter().zip(b_args).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => ty_equal_modulo_origins(a, b),
                    (TyArg::Origin(_), TyArg::Origin(_)) => true,
                    _ => a == b,
                })
        }
        (Ty::Tuple(a), Ty::Tuple(b))
        | (Ty::RuntimePack(a), Ty::RuntimePack(b))
        | (Ty::Variant(a), Ty::Variant(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| ty_equal_modulo_origins(a, b))
        }
        (Ty::ComptimeList(a), Ty::ComptimeList(b)) | (Ty::VariadicPack(a), Ty::VariadicPack(b)) => {
            ty_equal_modulo_origins(a, b)
        }
        // Callable environments (`thin` vs `capturing[origin@N]`) and
        // parameter-name/convention spellings erase from the runtime ABI:
        // one two-word value shape serves every `def(...)` contract with the
        // same parameter/return/raising structure.
        (
            Ty::Func {
                params: a_params,
                ret: a_ret,
                required: a_required,
                variadic: a_variadic,
                kw_variadic: a_kw_variadic,
                positional_only: a_positional_only,
                keyword_only: a_keyword_only,
                raises: a_raises,
                error: a_error,
                ..
            },
            Ty::Func {
                params: b_params,
                ret: b_ret,
                required: b_required,
                variadic: b_variadic,
                kw_variadic: b_kw_variadic,
                positional_only: b_positional_only,
                keyword_only: b_keyword_only,
                raises: b_raises,
                error: b_error,
                ..
            },
        ) => {
            let option_eq = |a: &Option<Box<Ty>>, b: &Option<Box<Ty>>| match (a, b) {
                (Some(a), Some(b)) => ty_equal_modulo_origins(a, b),
                (None, None) => true,
                _ => false,
            };
            a_raises == b_raises
                && a_required == b_required
                && a_positional_only == b_positional_only
                && a_keyword_only == b_keyword_only
                && a_params.len() == b_params.len()
                && a_params
                    .iter()
                    .zip(b_params)
                    .all(|(a, b)| ty_equal_modulo_origins(a, b))
                && ty_equal_modulo_origins(a_ret, b_ret)
                && option_eq(a_variadic, b_variadic)
                && option_eq(a_kw_variadic, b_kw_variadic)
                && option_eq(a_error, b_error)
        }
        _ => a == b,
    }
}

/// Erase the callable-environment spelling from every `Ty::Func` in `ty`,
/// recursively. Environments (`thin` vs `capturing[...]`) are semantic
/// origin facts with no runtime ABI: instance identity and binding solutions
/// must not split on them (`capturing[_]` vs `capturing[origin@N]` is the
/// same closure value).
fn canonicalize_callable(ty: &Ty) -> Ty {
    let mut canonical = ty.clone();
    erase_callable_environments(&mut canonical);
    canonical
}

fn erase_callable_environments(ty: &mut Ty) {
    match ty {
        Ty::Func {
            environment,
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            *environment = crate::origin::CallableEnvironment::Default;
            for param in params {
                erase_callable_environments(param);
            }
            erase_callable_environments(ret);
            if let Some(variadic) = variadic {
                erase_callable_environments(variadic);
            }
            if let Some(kw_variadic) = kw_variadic {
                erase_callable_environments(kw_variadic);
            }
            if let Some(error) = error {
                erase_callable_environments(error);
            }
        }
        Ty::Struct(_, args) => {
            for arg in args {
                if let TyArg::Ty(ty) = arg {
                    erase_callable_environments(ty);
                }
            }
        }
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            for element in elements {
                erase_callable_environments(element);
            }
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) => {
            erase_callable_environments(element);
        }
        Ty::Pointer { element, .. } => erase_callable_environments(element),
        Ty::Ref(reference) => erase_callable_environments(&mut reference.referent),
        _ => {}
    }
}

/// The referent behind any number of reference layers — the VM dereferences
/// `Value::Ref` operands before nominal dispatch.
fn peel_refs(ty: &Ty) -> &Ty {
    let mut ty = ty;
    while let Ty::Ref(reference) = ty {
        ty = &reference.referent;
    }
    ty
}

fn reg_ty<'a>(function: &'a MirFunction, reg: Reg, owner: &str) -> Result<&'a Ty, MonoError> {
    function.reg_types.get(&reg.0).ok_or_else(|| MonoError {
        function: Some(owner.to_string()),
        construct: format!("register r{} lacks a concrete type", reg.0),
    })
}

/// A `Some[Trait]` sugar parameter is infer-only and absent from the
/// declaration's `param_decls`, yet each binding selects a different body
/// (`update(value: Some[Hashable])` hashes an `Int` and a `Pair` through
/// different leaves). Its binding joins the instance identity; the builtin
/// string binding — the `Some[Writer]` display accumulator and the
/// declaration-order default — keeps the unsuffixed spelling.
fn push_sugar_arguments(
    declaration: &MirFunctionDeclaration,
    bindings: &Bindings,
    arguments: &mut Vec<InstanceArg>,
) {
    for ty in &declaration.param_types {
        if let Ty::Param { name, .. } = peel_refs(ty)
            && !declaration
                .param_decls
                .iter()
                .any(|decl| decl.name().trim_start_matches('*') == name)
            && let Some(bound) = bindings.types.get(name.as_str())
            && *bound != Ty::StringLiteral
        {
            arguments.push(InstanceArg::Ty(bound.clone()));
        }
    }
}

fn ordered_arguments(
    decls: &[ParamDecl],
    bindings: &Bindings,
    target: &str,
) -> Result<Vec<InstanceArg>, MonoError> {
    decls
        .iter()
        .map(|decl| {
            match decl {
                ParamDecl::Type { name, .. } => {
                    bindings.types.get(name).cloned().map(InstanceArg::Ty)
                }
                ParamDecl::Value { name, .. } => {
                    bindings.values.get(name).cloned().map(InstanceArg::Value)
                }
            }
            .ok_or_else(|| MonoError {
                function: Some(target.to_string()),
                construct: format!(
                    "monomorphization cannot resolve parameter `{}`",
                    decl.name()
                ),
            })
        })
        .collect()
}

fn bind_explicit_value_arguments(
    decls: &[ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    constant_values: &HashMap<u32, CtValue>,
    bindings: &mut Bindings,
    target: &str,
) -> Result<(), MonoError> {
    let mut positional = 0;
    for argument in arguments {
        let Some(value_reg) = argument.value else {
            if argument.name.is_none() {
                positional += 1;
            }
            continue;
        };
        let declaration = if let Some(name) = &argument.name {
            decls.iter().find(|declaration| declaration.name() == name)
        } else {
            let declaration = decls.get(positional);
            positional += 1;
            declaration
        };
        let Some(ParamDecl::Value { name, .. }) = declaration else {
            continue;
        };
        let value = constant_values
            .get(&value_reg.0)
            .cloned()
            .ok_or_else(|| MonoError {
                function: Some(target.to_string()),
                construct: format!("value parameter `{name}` is not compile-time constant"),
            })?;
        bindings.values.insert(name.clone(), value);
    }
    Ok(())
}

fn apply_defaults(decls: &[ParamDecl], bindings: &mut Bindings) -> Result<(), MonoError> {
    for decl in decls {
        match decl {
            ParamDecl::Type {
                name,
                default: Some(default),
                ..
            } if !bindings.types.contains_key(name) => {
                bindings
                    .types
                    .insert(name.clone(), substitute_ty(default, bindings)?);
            }
            ParamDecl::Value {
                name,
                default: Some(default),
                ..
            } if !bindings.values.contains_key(name) => {
                bindings
                    .values
                    .insert(name.clone(), eval_ct(default, bindings)?);
            }
            _ => {}
        }
    }
    Ok(())
}

fn bind_ty_args(
    decls: &[ParamDecl],
    args: &[TyArg],
    bindings: &mut Bindings,
) -> Result<(), String> {
    for (decl, arg) in decls.iter().zip(args) {
        match (decl, arg) {
            (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => bind_type(name, ty, bindings)?,
            (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                bind_value(name, value, bindings)?
            }
            (_, TyArg::Origin(_)) => {}
            _ => {
                return Err(format!(
                    "argument for `{}` has the wrong parameter kind",
                    decl.name()
                ));
            }
        }
    }
    Ok(())
}

fn unify(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    match pattern {
        Ty::Param { name, .. } => bind_type(name, actual, bindings),
        Ty::Assoc { .. } => {
            let key = pattern.to_string();
            match bindings.associated.get(&key) {
                Some(known) if known != actual => Err(format!(
                    "conflicting solutions for associated type `{key}`: `{known}` and `{actual}`"
                )),
                Some(_) => Ok(()),
                None => {
                    bindings.associated.insert(key, actual.clone());
                    Ok(())
                }
            }
        }
        // A literal-typed register materializes into whatever concrete
        // storage the checker admitted (`MaterializeLiteral` converts the
        // value at the boundary); the pattern constrains nothing here.
        _ if matches!(
            actual,
            Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral
        ) && pattern != actual =>
        {
            Ok(())
        }
        Ty::Struct(pn, pa) => match actual {
            Ty::Struct(an, _) if nominal_template(pn) == nominal_template(an) && pa.is_empty() => {
                Ok(())
            }
            Ty::Struct(an, aa)
                if nominal_template(pn) == nominal_template(an) && pa.len() == aa.len() =>
            {
                pa.iter()
                    .zip(aa)
                    .try_for_each(|(p, a)| unify_arg(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Tuple(p) | Ty::RuntimePack(p) | Ty::Variant(p) => match actual {
            Ty::Tuple(a) | Ty::RuntimePack(a) | Ty::Variant(a) if p.len() == a.len() => {
                p.iter().zip(a).try_for_each(|(p, a)| unify(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::ComptimeList(p) | Ty::VariadicPack(p) => match actual {
            Ty::ComptimeList(a) | Ty::VariadicPack(a) => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Pointer { element: p, .. } => match actual {
            Ty::Pointer { element: a, .. } => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Ref(p) => match actual {
            Ty::Ref(a) if p.mutability == a.mutability => unify(&p.referent, &a.referent, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        // Callable contracts unify on their runtime structure — parameters,
        // return, raising — never on the environment (`thin` vs
        // `capturing[...]`) or origin spellings, which erase from the ABI.
        Ty::Func {
            params: p_params,
            ret: p_ret,
            required: p_required,
            variadic: p_variadic,
            kw_variadic: p_kw_variadic,
            positional_only: p_positional_only,
            keyword_only: p_keyword_only,
            raises: p_raises,
            error: p_error,
            ..
        } => match actual {
            Ty::Func {
                params: a_params,
                ret: a_ret,
                required: a_required,
                variadic: a_variadic,
                kw_variadic: a_kw_variadic,
                positional_only: a_positional_only,
                keyword_only: a_keyword_only,
                raises: a_raises,
                error: a_error,
                ..
            } if p_params.len() == a_params.len()
                && p_required == a_required
                && p_positional_only == a_positional_only
                && p_keyword_only == a_keyword_only
                && p_raises == a_raises =>
            {
                let unify_option = |p: &Option<Box<Ty>>,
                                    a: &Option<Box<Ty>>,
                                    bindings: &mut Bindings|
                 -> Result<(), String> {
                    match (p, a) {
                        (Some(p), Some(a)) => unify(p, a, bindings),
                        (None, None) => Ok(()),
                        _ => Err(format!("expected `{pattern}`, found `{actual}`")),
                    }
                };
                p_params
                    .iter()
                    .zip(a_params)
                    .try_for_each(|(p, a)| unify(p, a, bindings))?;
                unify(p_ret, a_ret, bindings)?;
                unify_option(p_variadic, a_variadic, bindings)?;
                unify_option(p_kw_variadic, a_kw_variadic, bindings)?;
                unify_option(p_error, a_error, bindings)
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        _ if pattern == actual => Ok(()),
        _ => Err(format!("expected `{pattern}`, found `{actual}`")),
    }
}

/// Unify a callee's declared result against the caller's checked result type,
/// stripping `ref` layers on both sides first: a reference-returning call
/// spells its declared referent and the checked handle with differing layers.
fn unify_result(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    let mut pattern = pattern;
    while let Ty::Ref(reference) = pattern {
        pattern = &reference.referent;
    }
    let mut actual = actual;
    while let Ty::Ref(reference) = actual {
        actual = &reference.referent;
    }
    // A container element may itself be a reference. Receiver inference has
    // then already bound `T = ref U`, while the checker-flattened reference
    // result is spelled `ref U`; stripping its handle above leaves `U`.
    // Preserve the established element solution instead of mistaking the
    // flattened handle for a conflicting `T = U` solution.
    if let Ty::Param { name, .. } = pattern
        && let Some(Ty::Ref(reference)) = bindings.types.get(name)
        && ty_equal_modulo_origins(&reference.referent, actual)
    {
        return Ok(());
    }
    unify(pattern, actual, bindings)
}

fn unify_arg(pattern: &TyArg, actual: &TyArg, bindings: &mut Bindings) -> Result<(), String> {
    match (pattern, actual) {
        (TyArg::Ty(p), TyArg::Ty(a)) => unify(p, a, bindings),
        (TyArg::Val(CtValue::Param(name)), TyArg::Val(value)) => bind_value(name, value, bindings),
        (TyArg::Val(p), TyArg::Val(a)) if p == a => Ok(()),
        (TyArg::Origin(_), TyArg::Origin(_)) => Ok(()),
        _ => Err("generic application arguments disagree".to_string()),
    }
}

fn bind_type(name: &str, ty: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    if is_symbolic(ty) {
        return Err(format!("solution for `{name}` is not concrete: `{ty}`"));
    }
    // Solutions join instance identity: erase callable-environment spellings
    // so `capturing[origin@N]` and `thin` variants of one contract are one
    // instance.
    let ty = &canonicalize_callable(ty);
    let literal = |ty: &Ty| matches!(ty, Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral);
    match bindings.types.get(name) {
        // A literal-typed actual materializes into whatever concrete storage
        // is already bound, and a concrete solution upgrades an earlier
        // literal-only binding — mirroring `unify`'s literal escape. Binding
        // order varies by call shape (receiver-first vs result-last), so the
        // merge must be order-independent.
        Some(old) if literal(ty) && !literal(old) => Ok(()),
        Some(old) if literal(old) && !literal(ty) => {
            bindings.types.insert(name.to_string(), ty.clone());
            Ok(())
        }
        // Origins erase from the runtime ABI, so solutions differing only in
        // `ref`/pointer origins are one instance — the first spelling wins.
        // `Ty`'s `Display` collapses distinct types (`IntLiteral` renders as
        // `Int`), so the conflict text carries the structural form too.
        Some(old) if old != ty && !ty_equal_modulo_origins(old, ty) => Err(format!(
            "conflicting solutions for `{name}`: `{old}` ({old:?}) and `{ty}` ({ty:?})"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.types.insert(name.to_string(), ty.clone());
            Ok(())
        }
    }
}

fn bind_value(name: &str, value: &CtValue, bindings: &mut Bindings) -> Result<(), String> {
    if matches!(value, CtValue::Param(_)) {
        return Err(format!("solution for `{name}` is not constant"));
    }
    match bindings.values.get(name) {
        // As in `bind_type`, `Display` can collapse distinct values (an Int
        // and a UInt render alike), so the conflict text carries the
        // structural forms.
        Some(old) if old != value => Err(format!(
            "conflicting solutions for `{name}`: `{old}` ({old:?}) and `{value}` ({value:?})"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.values.insert(name.to_string(), value.clone());
            Ok(())
        }
    }
}

fn substitute_function(function: &mut MirFunction, bindings: &Bindings) -> Result<(), MonoError> {
    substitute_value_parameter_reads(
        &mut function.blocks,
        &function.var_names,
        &function.var_tys,
        bindings,
    )?;
    for (var, name) in function.var_names.iter().enumerate() {
        if let Some(value) = bindings.values.get(name) {
            let ty = match value {
                CtValue::Int(_) => Ty::Int,
                CtValue::Bool(_) => Ty::Bool,
                _ => continue,
            };
            function.var_tys.insert(var as u32, ty);
        }
    }
    for ty in &mut function.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.var_tys.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.reg_types.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.ret_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    substitute_blocks_metadata(&mut function.blocks, bindings)?;
    repair_storage_result_types(function);
    Ok(())
}

fn repair_storage_result_types(function: &mut MirFunction) {
    fn collect_retyped_iterator_slots(blocks: &[MirBlock], slots: &mut HashSet<u32>) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::GetIter { source, dest, .. } if source == dest => {
                        slots.insert(*dest);
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        collect_retyped_iterator_slots(body, slots);
                        if let Some((_, blocks)) = handler {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                        if let Some(blocks) = orelse {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                        if let Some(blocks) = finalbody {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn visit(
        blocks: &[MirBlock],
        var_tys: &HashMap<u32, Ty>,
        reg_tys: &HashMap<u32, Ty>,
        retyped_iterator_slots: &HashSet<u32>,
        reg_repairs: &mut Vec<(u32, Ty)>,
        var_repairs: &mut Vec<(u32, Ty)>,
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::UseVar { dest, var, .. } if !retyped_iterator_slots.contains(var) => {
                        if let Some(ty) = var_tys.get(var) {
                            reg_repairs.push((dest.0, ty.clone()));
                        }
                    }
                    MirInstr::LoadPlace { dest, place }
                        if place.proj.is_empty()
                            && !retyped_iterator_slots.contains(&place.root) =>
                    {
                        if let Some(ty) = var_tys.get(&place.root) {
                            reg_repairs.push((dest.0, ty.clone()));
                        }
                    }
                    MirInstr::DefVar { var, src, .. } if !retyped_iterator_slots.contains(var) => {
                        if let Some(ty) = reg_tys.get(&src.0) {
                            var_repairs.push((*var, ty.clone()));
                        }
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        visit(
                            body,
                            var_tys,
                            reg_tys,
                            retyped_iterator_slots,
                            reg_repairs,
                            var_repairs,
                        );
                        if let Some((_, blocks)) = handler {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                        if let Some(blocks) = orelse {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                        if let Some(blocks) = finalbody {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut retyped_iterator_slots = HashSet::new();
    collect_retyped_iterator_slots(&function.blocks, &mut retyped_iterator_slots);
    for _ in 0..3 {
        let mut reg_repairs = Vec::new();
        let mut var_repairs = Vec::new();
        visit(
            &function.blocks,
            &function.var_tys,
            &function.reg_types,
            &retyped_iterator_slots,
            &mut reg_repairs,
            &mut var_repairs,
        );
        function.reg_types.extend(reg_repairs);
        function.var_tys.extend(var_repairs);
    }
}

fn substitute_value_parameter_reads(
    blocks: &mut [MirBlock],
    var_names: &[String],
    var_tys: &HashMap<u32, Ty>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::UseVar { dest, var, .. } = instruction
                && let Some(name) = var_names.get(*var as usize)
                && let Some(value) = bindings.values.get(name)
            {
                let constant = if let Some(callable) = bindings.callables.get(name) {
                    Const::Function(callable.clone())
                } else {
                    match value {
                        CtValue::Int(value) => Const::Int(*value),
                        CtValue::Bool(value) => Const::Bool(*value),
                        CtValue::Str(value)
                            if matches!(
                                var_tys.get(var),
                                Some(Ty::Func { .. } | Ty::GenericFunc { .. })
                            ) =>
                        {
                            Const::Function(value.clone())
                        }
                        _ => {
                            return Err(MonoError {
                                function: None,
                                construct: format!("unsupported runtime value parameter `{value}`"),
                            });
                        }
                    }
                };
                *instruction = MirInstr::Const {
                    dest: *dest,
                    k: constant,
                };
            } else if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                substitute_value_parameter_reads(body, var_names, var_tys, bindings)?;
                if let Some((_, blocks)) = handler {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
                if let Some(blocks) = orelse {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
                if let Some(blocks) = finalbody {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
            }
        }
    }
    Ok(())
}

fn substitute_declaration(
    decl: &mut MirFunctionDeclaration,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for ty in &mut decl.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut decl.variadic {
        *ty = substitute_ty(ty, bindings)?;
        // An arity-specialized instance's pack reifies as the concrete
        // tuple shape the call site collected.
        if let Some(arity) = bindings.variadic_arity
            && !matches!(ty, Ty::RuntimePack(_) | Ty::Tuple(_))
        {
            *ty = Ty::RuntimePack(vec![ty.clone(); arity]);
        }
    }
    if let Some(ty) = &mut decl.kw_variadic {
        *ty = substitute_ty(ty, bindings)?;
    }
    decl.ret_ty = substitute_ty(&decl.ret_ty, bindings)?;
    if let Some(ty) = &mut decl.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}

fn substitute_blocks_metadata(
    blocks: &mut [MirBlock],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            substitute_instruction(instruction, bindings)?;
        }
    }
    Ok(())
}

fn substitute_instruction(
    instruction: &mut MirInstr,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    use MirInstr::*;
    match instruction {
        EstablishLoans { loans, .. } => {
            for loan in loans {
                substitute_place(&mut loan.place, bindings)?;
            }
        }
        MakeRef { place, .. }
        | MovePlace { place, .. }
        | Store { place, .. }
        | StoreRef { place, .. }
        | LoadPlace { place, .. }
        | ConsumePlace { place, .. } => substitute_place(place, bindings)?,
        MakeClosure { captures, .. } => {
            for capture in captures {
                substitute_place(&mut capture.place, bindings)?;
            }
        }
        MaterializeLiteral { target, .. }
        | SizeOf { ty: target, .. }
        | PointerStorageTake {
            element: target, ..
        }
        | PointerStorageDestroy {
            element: target, ..
        }
        | UninitStorageTake {
            element: target, ..
        }
        | UninitStorageDestroy {
            element: target, ..
        } => *target = substitute_ty(target, bindings)?,
        Next {
            call: Some(call), ..
        } => substitute_iterator_call(call, bindings)?,
        TryNext {
            call, exhaustion, ..
        } => {
            substitute_iterator_call(call, bindings)?;
            *exhaustion = substitute_ty(exhaustion, bindings)?;
        }
        DefVar {
            binding_ty: Some(ty),
            ..
        } => *ty = substitute_ty(ty, bindings)?,
        Call {
            raises,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        // `H()` on a type parameter constructs the bound struct: once the
        // binding is concrete this is an ordinary nullary constructor call,
        // which the call rewriting below then instantiates.
        ConstructTypeParam { dest, param } => {
            let Some(Ty::Struct(struct_name, _)) = bindings.types.get(param.as_str()) else {
                return Err(MonoError {
                    function: None,
                    construct: format!(
                        "constructing type parameter `{param}` without a concrete struct binding"
                    ),
                });
            };
            *instruction = Call {
                dest: *dest,
                func: crate::mir::FuncRef::named(struct_name),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
            };
        }
        CallIndirect {
            raises,
            callee_place,
            arg_places,
            kwarg_places,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_place_opt(callee_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(instantiated_contract, bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        MethodCall {
            raises,
            reference_result,
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_ref_opt(reference_result, bindings)?;
            sub_place_opt(recv_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        Index {
            base_place,
            index_place,
            call,
            ..
        } => {
            sub_place_opt(base_place, bindings)?;
            sub_place_opt(index_place, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        Slice {
            object_place,
            arg_places,
            call,
            ..
        }
        | MultiIndex {
            object_place,
            arg_places,
            call,
            ..
        } => {
            sub_place_opt(object_place, bindings)?;
            sub_places(arg_places, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            ..
        } => {
            sub_place_opt(receiver_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_place_opt(value_place, bindings)?;
            substitute_subscript_call(call, bindings)?;
        }
        MakeTuple {
            element_types: Some(types),
            ..
        }
        | MakeVariant {
            alternatives: types,
            ..
        } => {
            for ty in types {
                *ty = substitute_ty(ty, bindings)?;
            }
        }
        VariantSet { place, .. }
        | VariantSetInitWith { place, .. }
        | VariantReplace { place, .. } => substitute_place(place, bindings)?,
        Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            substitute_blocks_metadata(body, bindings)?;
            if let Some((_, b)) = handler {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = orelse {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = finalbody {
                substitute_blocks_metadata(b, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn substitute_subscript_call(
    call: &mut crate::mir::MirSubscriptCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    sub_opt_ty(&mut call.raises, bindings)?;
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)
}

fn substitute_place(place: &mut MirPlace, bindings: &Bindings) -> Result<(), MonoError> {
    sub_opt_ty(&mut place.root_ty, bindings)?;
    for ty in &mut place.projection_tys {
        *ty = substitute_ty(ty, bindings)?;
    }
    sub_opt_ty(&mut place.ty, bindings)
}
fn sub_places(places: &mut [Option<MirPlace>], bindings: &Bindings) -> Result<(), MonoError> {
    for place in places {
        sub_place_opt(place, bindings)?;
    }
    Ok(())
}
fn sub_place_opt(place: &mut Option<MirPlace>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(place) = place {
        substitute_place(place, bindings)?;
    }
    Ok(())
}
fn sub_opt_ty(ty: &mut Option<Ty>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}
fn sub_ref_opt(
    ty: &mut Option<crate::origin::RefTy>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty.referent = substitute_ty(&ty.referent, bindings)?;
    }
    Ok(())
}

fn substitute_iterator_call(
    call: &mut crate::checked::CheckedIteratorCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_opt_ty(&mut call.raises, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)?;
    Ok(())
}

fn substitute_ty(ty: &Ty, bindings: &Bindings) -> Result<Ty, MonoError> {
    let unsupported = |what: String| MonoError {
        function: None,
        construct: what,
    };
    Ok(match ty {
        Ty::Param { name, .. } => bindings
            .types
            .get(name)
            .cloned()
            .ok_or_else(|| unsupported(format!("unresolved type parameter `{name}`")))?,
        Ty::Struct(name, args) => {
            if args.is_empty() {
                // The bare in-body `self` spelling of a generic owner resolves
                // to the concrete instance being materialized; other bare
                // names are non-generic (or unresolvable, failing later).
                if let Some((template, concrete)) = &bindings.self_instance
                    && template == name
                {
                    return Ok(concrete.clone());
                }
                return Ok(Ty::Struct(name.clone(), Vec::new()));
            }
            let args = args
                .iter()
                .map(|arg| substitute_arg(arg, bindings))
                .collect::<Result<Vec<_>, _>>()?;
            // Every concrete application of a generic template takes its
            // instance symbol, so distinct instantiations get distinct output
            // declarations. Checker-specialized structs (empty `param_decls`)
            // and already-renamed instances keep their names; symbolic
            // applications stay for a later substitution or a contextual
            // rejection.
            let concrete_name = if args.iter().any(arg_has_symbolic)
                || nominal_template(name) != name
                || !bindings.generic_templates.contains(name.as_str())
            {
                name.clone()
            } else {
                crate::symbol::instance_symbol(
                    name,
                    &args
                        .iter()
                        .filter_map(|arg| match arg {
                            TyArg::Ty(ty) => Some(InstanceArg::Ty(ty.clone())),
                            TyArg::Val(value) => Some(InstanceArg::Value(value.clone())),
                            TyArg::Origin(_) => None,
                        })
                        .collect::<Vec<_>>(),
                )
            };
            Ty::Struct(concrete_name, args)
        }
        Ty::Tuple(v) => Ty::Tuple(sub_types(v, bindings)?),
        Ty::RuntimePack(v) => Ty::RuntimePack(sub_types(v, bindings)?),
        Ty::Variant(v) => Ty::Variant(sub_types(v, bindings)?),
        Ty::Overload(v) => Ty::Overload(sub_types(v, bindings)?),
        Ty::ComptimeList(v) => Ty::ComptimeList(Box::new(substitute_ty(v, bindings)?)),
        Ty::VariadicPack(v) => {
            let element = substitute_ty(v, bindings)?;
            match bindings.variadic_arity {
                // An unspecialized variadic callee instantiates at its
                // call-site arity: the pack becomes a concrete tuple shape.
                Some(arity) => Ty::RuntimePack(vec![element; arity]),
                None => Ty::VariadicPack(Box::new(element)),
            }
        }
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute_ty(element, bindings)?),
            origin: origin.clone(),
        },
        Ty::Ref(value) => {
            let mut value = value.clone();
            value.referent = Box::new(substitute_ty(&value.referent, bindings)?);
            Ty::Ref(value)
        }
        Ty::Dependent(DependentType::Indexed { elements, index }) => {
            let value = eval_ct(index, bindings)?;
            let index = match value {
                CtValue::Int(v) => usize::try_from(v).ok(),
                CtValue::UInt(v) => usize::try_from(v).ok(),
                _ => None,
            }
            .ok_or_else(|| {
                unsupported("dependent type index is not a non-negative integer".to_string())
            })?;
            substitute_ty(
                elements.get(index).ok_or_else(|| {
                    unsupported(format!("dependent type index {index} is out of range"))
                })?,
                bindings,
            )?
        }
        Ty::Assoc { .. } => bindings
            .associated
            .get(&ty.to_string())
            .cloned()
            .ok_or_else(|| {
                unsupported(format!(
                    "associated type `{ty}` has no concrete MIR declaration fact"
                ))
            })?,
        // A generic callable remains as a transient storage type until its
        // statically named producer and dependent call sites are rewritten.
        // `ensure_concrete_function` rejects it if any executable use survives.
        Ty::GenericFunc { .. } => ty.clone(),
        Ty::SelfType | Ty::Infer => {
            return Err(unsupported(format!("unresolved type `{ty}`")));
        }
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: sub_types(params, bindings)?,
            names: names.clone(),
            ret: Box::new(substitute_ty(ret, bindings)?),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        other => other.clone(),
    })
}

fn substitute_arg(arg: &TyArg, bindings: &Bindings) -> Result<TyArg, MonoError> {
    Ok(match arg {
        TyArg::Ty(ty) => TyArg::Ty(substitute_ty(ty, bindings)?),
        TyArg::Val(CtValue::Param(name)) => TyArg::Val(
            bindings
                .values
                .get(name)
                .cloned()
                .ok_or_else(|| MonoError {
                    function: None,
                    construct: format!("unresolved value parameter `{name}`"),
                })?,
        ),
        TyArg::Val(value) => TyArg::Val(value.clone()),
        TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
    })
}
fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}

fn eval_ct(expr: &CtExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    use CtExpr::*;
    let int = |value: CtValue| match value {
        CtValue::Int(v) => Ok(v),
        _ => Err(MonoError {
            function: None,
            construct: "dependent expression requires an Int value".to_string(),
        }),
    };
    Ok(match expr {
        Value(CtValue::Param(name)) | Param(name) => bindings
            .values
            .get(name)
            .cloned()
            .ok_or_else(|| MonoError {
                function: None,
                construct: format!("unresolved value parameter `{name}`"),
            })?,
        Value(value) => value.clone(),
        Neg(v) => CtValue::Int(int(eval_ct(v, bindings)?)?.wrapping_neg()),
        Add(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_add(int(eval_ct(b, bindings)?)?))
        }
        Sub(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_sub(int(eval_ct(b, bindings)?)?))
        }
        Mul(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_mul(int(eval_ct(b, bindings)?)?))
        }
        FloorDiv(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.div_euclid(int(eval_ct(b, bindings)?)?))
        }
        Mod(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.rem_euclid(int(eval_ct(b, bindings)?)?))
        }
        Pow(a, b) => CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_pow(
            u32::try_from(int(eval_ct(b, bindings)?)?).map_err(|_| MonoError {
                function: None,
                construct: "dependent exponent is out of range".to_string(),
            })?,
        )),
    })
}

fn is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer
        | Ty::Param { .. }
        | Ty::Assoc { .. }
        | Ty::Dependent(_)
        | Ty::SelfType
        | Ty::GenericFunc { .. } => true,
        Ty::Struct(_, args) => args.iter().any(arg_has_symbolic),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            v.iter().any(is_symbolic)
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            is_symbolic(v)
        }
        Ty::Ref(v) => is_symbolic(&v.referent),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(is_symbolic)
                || is_symbolic(ret)
                || variadic.as_deref().is_some_and(is_symbolic)
                || kw_variadic.as_deref().is_some_and(is_symbolic)
                || error.as_deref().is_some_and(is_symbolic)
        }
        _ => false,
    }
}
fn arg_has_symbolic(arg: &TyArg) -> bool {
    match arg {
        TyArg::Ty(ty) => is_symbolic(ty),
        TyArg::Val(CtValue::Param(_)) => true,
        TyArg::Val(_) | TyArg::Origin(_) => false,
    }
}
fn function_types(function: &MirFunction) -> impl Iterator<Item = &Ty> {
    function
        .param_types
        .iter()
        .chain(function.ret_ty.iter())
        .chain(function.error_ty.iter())
        .chain(function.var_tys.values())
        .chain(function.reg_types.values())
}

/// Dependent callable values are compile-time carriers once every indirect use
/// has become a direct specialized call. Remove their now-dead MIR plumbing so
/// neither verification nor backend lowering sees a fictitious runtime ABI.
fn erase_specialized_generic_callable_storage(function: &mut MirFunction) {
    fn erase(blocks: &mut [MirBlock], generic_regs: &HashSet<u32>, generic_vars: &HashSet<u32>) {
        for block in blocks {
            block.instrs.retain_mut(|instruction| {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    erase(body, generic_regs, generic_vars);
                    if let Some((_, blocks)) = handler {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    if let Some(blocks) = orelse {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    if let Some(blocks) = finalbody {
                        erase(blocks, generic_regs, generic_vars);
                    }
                    return true;
                }
                match instruction {
                    MirInstr::MakeClosure { dest, .. }
                    | MirInstr::Const { dest, .. }
                    | MirInstr::CopyValue { dest, .. }
                    | MirInstr::UseVar { dest, .. } => !generic_regs.contains(&dest.0),
                    MirInstr::DefVar { var, .. } => !generic_vars.contains(var),
                    _ => true,
                }
            });
        }
    }

    let generic_regs = function
        .reg_types
        .iter()
        .filter_map(|(reg, ty)| matches!(ty, Ty::GenericFunc { .. }).then_some(*reg))
        .collect::<HashSet<_>>();
    let generic_vars = function
        .var_tys
        .iter()
        .filter_map(|(var, ty)| matches!(ty, Ty::GenericFunc { .. }).then_some(*var))
        .collect::<HashSet<_>>();
    erase(&mut function.blocks, &generic_regs, &generic_vars);
    for reg in generic_regs {
        function.reg_types.insert(reg, Ty::Int);
    }
    for var in generic_vars {
        function.var_tys.insert(var, Ty::Int);
    }
}

/// Collect already-substituted types named only by instructions, recursing
/// into `try` regions. These types need layouts or lifecycle declarations even
/// when no register or variable carries them.
fn push_instruction_types(blocks: &[MirBlock], out: &mut Vec<Ty>) {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    push_instruction_types(body, out);
                    if let Some((_, blocks)) = handler {
                        push_instruction_types(blocks, out);
                    }
                    if let Some(blocks) = orelse {
                        push_instruction_types(blocks, out);
                    }
                    if let Some(blocks) = finalbody {
                        push_instruction_types(blocks, out);
                    }
                }
                MirInstr::SizeOf { ty, .. } => out.push(ty.clone()),
                MirInstr::PointerStorageTake { element, .. }
                | MirInstr::PointerStorageDestroy { element, .. }
                | MirInstr::UninitStorageTake { element, .. }
                | MirInstr::UninitStorageDestroy { element, .. } => out.push(element.clone()),
                _ => {}
            }
        }
    }
}
fn ensure_concrete_function(name: &str, function: &MirFunction) -> Result<(), MonoError> {
    if let Some(ty) = function_types(function).find(|ty| is_symbolic(ty)) {
        Err(MonoError {
            function: Some(name.to_string()),
            construct: format!("symbolic type `{ty}` remains after monomorphization"),
        })
    } else {
        Ok(())
    }
}
fn collect_nested_types(ty: &Ty, output: &mut Vec<Ty>) {
    match ty {
        Ty::Struct(_, args) => output.extend(args.iter().filter_map(|a| {
            if let TyArg::Ty(t) = a {
                Some(t.clone())
            } else {
                None
            }
        })),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            output.extend(v.iter().cloned())
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            output.push((**v).clone())
        }
        Ty::Ref(v) => output.push((*v.referent).clone()),
        _ => {}
    }
}
fn nominal_template(name: &str) -> &str {
    name.split("$mono").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specialized_main(source: &str) -> SpecializedProgram {
        let compiler = crate::Compiler::default().with_snippet_module_scope();
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
        let specialized =
            specialized_main("def main():\n    for x in range(3):\n        print(x)\n");
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
        let source = include_str!("../../assets/ok/generic_borrowed_dispatch_overloaded_iter.mojo");
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
        let source = include_str!("../../assets/ok/lambda_generic_comptime.mojo");
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
        let source = include_str!("../../assets/ok/container_owning_family_apis.mojo");
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
            origin: crate::origin::PointerOrigin::Static,
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
        let first = Ty::Ref(crate::origin::RefTy {
            referent: referent.clone(),
            origin: crate::origin::Origin::Static,
            mutability: crate::origin::Mutability::Immutable,
        });
        let second = Ty::Ref(crate::origin::RefTy {
            referent,
            origin: crate::origin::Origin::Untracked { mutable: false },
            mutability: crate::origin::Mutability::Immutable,
        });
        let mut bindings = Bindings::default();
        bind_type("T", &first, &mut bindings).unwrap();
        bind_type("T", &second, &mut bindings).unwrap();
        // First solution wins; a mutability disagreement still conflicts.
        assert_eq!(bindings.types.get("T"), Some(&first));
        let mutable = Ty::Ref(crate::origin::RefTy {
            referent: Box::new(Ty::Int),
            origin: crate::origin::Origin::Static,
            mutability: crate::origin::Mutability::Mutable,
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
}
