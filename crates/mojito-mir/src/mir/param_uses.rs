//! Parameter uses: which of a generic function's own compile-time
//! parameters its body and signature read, the analysis upstream's
//! `RemoveUnusedParams` runs before it drops a parameter from a generator.
//!
//! MIR names a binder by its identity wherever it refers to one: a type
//! parameter, a dependent type, a parameter expression, a call's parameter
//! arguments. So the walk is structural, after MLIR's walk over every
//! attribute and type an operation holds: it visits the value's serialized
//! form, and a `(owner, slot)` pair spelling one of the function's binders
//! is a read of it. A pair that only happens to spell one counts too, which
//! keeps a parameter and never drops one. Two further reads are not
//! identities: a value parameter's runtime reads go through the slot its
//! name binds, and a body nested in the function (named `f$…`) reads the
//! binders of its enclosing scope its own analysis kept, as upstream's
//! `RemoveUnusedParams` reads a lifted closure's generator after removing
//! its unused parameters. A body lifted from the function (named
//! `$comptime$f$…`, `$default$f$…`), and a nested body whose analysis is not
//! at hand, reads every parameter.

use crate::mir::verify::instruction_result_regs;
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg, MirTerm,
    TyArg, count_register_reads, instruction_regions, instruction_regions_mut, instruction_slots,
    matched_parameter_arguments,
};
use mojito_ast::ast::InfixOp;
use mojito_hir::hir::VarId;
use mojito_types::param_expr::ParamId;
use mojito_types::types::{ParamDecl, Ty, TyKind};
use serde::Serialize;
use serde::ser;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

/// The parameters upstream's `RemoveUnusedParams` removes from `name`.
///
/// For a top-level function they are its own compile-time parameters, and
/// for a nested one the parameters of the declarations it is nested in,
/// that nothing in its body `function` or its declaration's signature
/// reads, sorted by owner and slot. A nested body `function` names reads
/// the candidates its own removals `nested` keep; one `nested` does not
/// answer reads them all.
///
/// A default of another parameter is not a read, since a default is the
/// call site's. A function inside a `comptime for`, `__toplevel__`, and a
/// function without a declaration have no candidates, nor has a nested
/// function's own parameter or a callable value parameter, whose binding
/// can add runtime parameters to the instance.
pub fn unread_parameters(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    nested: &NestedRemovals<'_>,
) -> Vec<ParamId> {
    let Some(declaration) = declarations.function(name) else {
        return Vec::new();
    };
    if name == "__toplevel__" || !declaration.region_binders.is_empty() {
        return Vec::new();
    }
    let candidates = candidate_parameters(name, declaration, declarations);
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut owners: Vec<&str> = Vec::new();
    for decl in &candidates {
        if !owners.contains(&&*decl.id().owner) {
            owners.push(&decl.id().owner);
        }
    }
    let mut walker = Walker::new(owners, nested);
    walker.signature(declaration);
    walker.body(function);
    if walker.reads_all {
        return Vec::new();
    }
    let read_by_slot = slot_reads(function, &candidates);
    let read = |decl: &ParamDecl| {
        let id = decl.id();
        walker
            .owners
            .iter()
            .position(|owner| **owner == *id.owner)
            .is_some_and(|owner| walker.slots.contains(&(owner, id.slot)))
            || read_by_slot.contains(decl.name().trim_start_matches('*'))
            || walker
                .nested_removals
                .iter()
                .any(|removed| !removed.contains(id))
    };
    let mut unread: Vec<ParamId> = candidates
        .into_iter()
        .filter(|decl| !read(decl))
        .map(|decl| decl.id().clone())
        .collect();
    unread.sort();
    unread
}

/// The removals of a body nested in the function being analysed, by its
/// name: `None` when its analysis is not at hand.
pub type NestedRemovals<'a> = dyn Fn(&str) -> Option<&'a [ParamId]> + 'a;

/// A compiler-known scalar, whose drop runs no code.
pub fn has_no_destructor(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int
            | TyKind::UInt
            | TyKind::Bool
            | TyKind::Float64
            | TyKind::StringLiteral
            | TyKind::None
            | TyKind::IntLiteral
            | TyKind::FloatLiteral
            | TyKind::Dtype
            | TyKind::Simd { .. }
    )
}

/// `function` with every call dropping the arguments of the callee's
/// removed parameters, `removed(callee)`; `None` when no call drops one.
///
/// It is the body upstream's `RemoveUnusedParams` reads once it has
/// rewritten each call of a callee it processed before. Returns the view and the callees whose calls dropped an argument. The
/// view drops each such argument's register and expression, its entry in
/// the call's `instantiated_args` (when they align with the callee's
/// declarations), and then the instructions that only computed a dropped
/// argument: an unread copy of a slot whose type has no destructor, a
/// built-in operator that cannot trap, and anything trivially dead. A
/// removed parameter that a kept parameter's default reads keeps its
/// argument, which the call's omission of the kept one may need.
pub fn without_removed_forwards<'a>(
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> Option<(MirFunction, Vec<String>)> {
    if !calls_with_removed(&function.blocks, removed) {
        return None;
    }
    let mut view = function.clone();
    let mut discounted = BTreeSet::new();
    drop_removed_arguments(&mut view.blocks, declarations, removed, &mut discounted);
    if discounted.is_empty() {
        return None;
    }
    erase_forward_chains(&mut view);
    Some((view, discounted.into_iter().collect()))
}

/// The parameters `unread_parameters` may remove from `name`: its own
/// compile-time parameters, or a nested function's enclosing declarations'
/// parameters, each once, but a callable value parameter.
fn candidate_parameters<'d>(
    name: &str,
    declaration: &'d MirFunctionDeclaration,
    declarations: &'d MirDeclarations,
) -> Vec<&'d ParamDecl> {
    let mut candidates: Vec<&ParamDecl> = Vec::new();
    if declaration.enclosing.is_none() {
        candidates.extend(
            declaration
                .param_decls
                .iter()
                .filter(|decl| &*decl.id().owner == name),
        );
    }
    let mut link = declaration
        .enclosing
        .as_deref()
        .and_then(|enclosing| declarations.function(enclosing));
    while let Some(enclosing) = link {
        for decl in &enclosing.param_decls {
            if !candidates.iter().any(|seen| seen.id() == decl.id()) {
                candidates.push(decl);
            }
        }
        link = enclosing
            .enclosing
            .as_deref()
            .and_then(|enclosing| declarations.function(enclosing));
    }
    candidates.retain(|decl| !is_callable_value(decl));
    candidates
}

fn is_callable_value(decl: &ParamDecl) -> bool {
    matches!(
        decl,
        ParamDecl::Value { ty, callable_default, .. }
            if callable_default.is_some()
                || matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
    )
}

/// Whether `blocks` or their regions call a callee `removed` names
/// parameters of.
fn calls_with_removed<'a>(
    blocks: &[MirBlock],
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> bool {
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .any(|instruction| {
            let target = match instruction {
                MirInstr::Call { func, .. } => Some(func.0.as_str()),
                MirInstr::MethodCall { resolved, .. } => resolved.as_deref(),
                _ => None,
            };
            target.and_then(removed).is_some_and(|ids| !ids.is_empty())
                || instruction_regions(instruction)
                    .into_iter()
                    .any(|region| calls_with_removed(region, removed))
        })
}

/// Drop from each call in `blocks` and their regions the arguments of the
/// callee's removed parameters, adding each callee that lost one to
/// `discounted`.
fn drop_removed_arguments<'a>(
    blocks: &mut [MirBlock],
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
    discounted: &mut BTreeSet<String>,
) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        for region in instruction_regions_mut(instruction) {
            drop_removed_arguments(region, declarations, removed, discounted);
        }
        let (target, decls, param_arg_regs, instantiated_args) = match instruction {
            MirInstr::Call {
                func,
                param_arg_regs,
                instantiated_args,
                ..
            } => {
                let Some(callee) = declarations.function(&func.0) else {
                    continue;
                };
                (
                    &func.0,
                    callee.param_decls.as_slice(),
                    param_arg_regs,
                    instantiated_args,
                )
            }
            MirInstr::MethodCall {
                resolved: Some(target),
                param_arg_regs,
                param_decls,
                instantiated_args,
                ..
            } => (
                &*target,
                param_decls.as_slice(),
                param_arg_regs,
                instantiated_args,
            ),
            _ => continue,
        };
        if let Some(ids) = removed(target)
            && drop_arguments(target, decls, ids, param_arg_regs, instantiated_args)
        {
            discounted.insert(target.clone());
        }
    }
}

/// Drop from one call of `target`, whose compile-time parameters are
/// `decls`, the arguments of its removed parameters `ids`; returns whether
/// it dropped one.
fn drop_arguments(
    target: &str,
    decls: &[ParamDecl],
    ids: &[ParamId],
    param_arg_regs: &mut [MirParamArg],
    instantiated_args: &mut Vec<TyArg>,
) -> bool {
    let mut defaulted = Walker::new(vec![target], &|_| None);
    for decl in decls.iter().filter(|decl| !ids.contains(decl.id())) {
        match decl {
            ParamDecl::Type { default, .. } => defaulted.visit(default),
            ParamDecl::Value { default, .. } => defaulted.visit(default),
        }
    }
    let dropped: Vec<usize> = decls
        .iter()
        .enumerate()
        .filter(|(_, decl)| {
            ids.contains(decl.id())
                && !defaulted.reads_all
                && !defaulted.slots.contains(&(0, decl.id().slot))
        })
        .map(|(index, _)| index)
        .collect();
    let arguments: Vec<usize> = matched_parameter_arguments(decls, param_arg_regs)
        .into_iter()
        .filter(|(decl, _, _)| dropped.iter().any(|index| decls[*index].id() == decl.id()))
        .filter_map(|(_, _, argument)| {
            param_arg_regs
                .iter()
                .position(|candidate| std::ptr::eq(candidate, argument))
        })
        .collect();
    let mut changed = false;
    for index in arguments {
        let argument = &mut param_arg_regs[index];
        changed |= argument.value.take().is_some() | argument.expr.take().is_some();
    }
    if instantiated_args.len() == decls.len() {
        for &index in dropped.iter().rev() {
            instantiated_args.remove(index);
            changed = true;
        }
    }
    changed
}

/// Erase from `function` every instruction that only computes a value
/// nothing reads and that may go without a trace: one
/// [`MirInstr::would_be_trivially_dead`] allows, a copy of a slot whose
/// type has no destructor, or a built-in operator over such operands that
/// cannot trap (integer arithmetic wraps). Repeats until nothing changes,
/// and drops the erased registers' types.
fn erase_forward_chains(function: &mut MirFunction) {
    let mut erased = HashSet::new();
    loop {
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let before = erased.len();
        let MirFunction {
            blocks,
            var_tys,
            reg_types,
            ..
        } = &mut *function;
        let trivial = |reg: &crate::mir::Reg| reg_types.get(&reg.0).is_some_and(has_no_destructor);
        erase_unread(
            blocks,
            &mut |instruction| match instruction {
                MirInstr::UseVar { var, .. } => var_tys.get(var).is_some_and(has_no_destructor),
                MirInstr::UnOp {
                    a, raises: None, ..
                } => trivial(a),
                MirInstr::BinOp {
                    op,
                    a,
                    b,
                    resolved: None,
                    raises: None,
                    ..
                } => {
                    matches!(
                        op,
                        InfixOp::Add
                            | InfixOp::Sub
                            | InfixOp::Mul
                            | InfixOp::BitAnd
                            | InfixOp::BitOr
                            | InfixOp::BitXor
                            | InfixOp::Eq
                            | InfixOp::Ne
                            | InfixOp::Lt
                            | InfixOp::Gt
                            | InfixOp::Le
                            | InfixOp::Ge
                    ) && trivial(a)
                        && trivial(b)
                }
                _ => instruction.would_be_trivially_dead(),
            },
            &reads,
            &mut erased,
        );
        if erased.len() == before {
            break;
        }
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
}

/// Remove each instruction of `blocks` and their regions that `erasable`
/// accepts and that defines only unread results, adding them to `erased`.
fn erase_unread(
    blocks: &mut [MirBlock],
    erasable: &mut dyn FnMut(&MirInstr) -> bool,
    reads: &[u32],
    erased: &mut HashSet<u32>,
) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
                erase_unread(region, erasable, reads, erased);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            let dead = !results.is_empty()
                && results
                    .iter()
                    .all(|reg| reads.get(reg.0 as usize) == Some(&0))
                && erasable(instruction);
            if dead {
                erased.extend(results.iter().map(|reg| reg.0));
            }
            !dead
        });
    }
}

/// The names of the value parameters among `candidates` whose slot the body
/// reads: any instruction naming the slot, but a drop or a declaration of a
/// slot whose type has no destructor to run.
fn slot_reads(function: &MirFunction, candidates: &[&ParamDecl]) -> HashSet<String> {
    let names: HashSet<&str> = candidates
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .map(|decl| decl.name().trim_start_matches('*'))
        .collect();
    let slots: HashMap<VarId, &str> = function
        .var_names
        .iter()
        .enumerate()
        .filter_map(|(slot, name)| names.get(name.as_str()).map(|name| (slot as VarId, *name)))
        .collect();
    let mut reads = HashSet::new();
    if !slots.is_empty() {
        let mut cx = SlotReads {
            function,
            slots: &slots,
            reads: &mut reads,
        };
        cx.blocks(&function.blocks);
    }
    reads
}

struct SlotReads<'a> {
    function: &'a MirFunction,
    slots: &'a HashMap<VarId, &'a str>,
    reads: &'a mut HashSet<String>,
}

impl SlotReads<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                self.instruction(instruction);
                for region in instruction_regions(instruction) {
                    self.blocks(region);
                }
            }
            if let MirTerm::ReturnWithCleanup { cleanup, .. }
            | MirTerm::EscapeJump { cleanup, .. } = &block.term
            {
                self.drops(cleanup);
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        match instruction {
            MirInstr::DropVar { var }
            | MirInstr::ConsumeVar { var }
            | MirInstr::DeclareVar { var } => {
                self.drops(std::slice::from_ref(var));
            }
            MirInstr::Try {
                handler, cleanup, ..
            } => {
                if let Some((Some(binding), _)) = handler {
                    self.read(*binding);
                }
                self.drops(cleanup);
            }
            _ => {
                for slot in instruction_slots(instruction) {
                    self.read(slot);
                }
            }
        }
    }

    /// Dropping a slot reads its value only when a destructor runs on it.
    fn drops(&mut self, slots: &[VarId]) {
        for &slot in slots {
            if !self
                .function
                .var_tys
                .get(&slot)
                .is_some_and(has_no_destructor)
            {
                self.read(slot);
            }
        }
    }

    fn read(&mut self, slot: VarId) {
        if let Some(name) = self.slots.get(&slot) {
            self.reads.insert((*name).to_string());
        }
    }
}

/// The serializer [`unread_parameters`] walks a value with, looking for the
/// identities of the binders of `owners`.
struct Walker<'o, 'a, 'n> {
    owners: Vec<&'o str>,
    /// The removals of the nested bodies a walked value names.
    nested: &'n NestedRemovals<'a>,
    /// The binders a walked value names, as the index of their owner in
    /// `owners` and their slot.
    slots: BTreeSet<(usize, usize)>,
    /// The removals of each body nested in an owner that a walked value
    /// names: it reads every candidate they do not hold.
    nested_removals: Vec<&'a [ParamId]>,
    /// Whether a walked value names a body lifted from an owner, or nested
    /// in one with no removals at hand, or holds a leaf no walk can see
    /// into, so that every parameter counts as read.
    reads_all: bool,
    /// The last leaf a walk visited.
    leaf: Leaf,
    /// Each tuple the walk is inside: its length, how many of its elements
    /// were walked, and the owner its first named.
    tuples: Vec<(usize, usize, Option<usize>)>,
    /// The reads each type walked so far makes: a type is a uniqued handle
    /// that many registers and slots share.
    types: HashMap<Ty, TypeReads<'a>>,
}

/// The binders, nested bodies' removals, and opacity of one walked type.
type TypeReads<'a> = (BTreeSet<(usize, usize)>, Vec<&'a [ParamId]>, bool);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Other,
    Owner(usize),
    Slot(u64),
}

impl<'o, 'a, 'n> Walker<'o, 'a, 'n> {
    fn new(owners: Vec<&'o str>, nested: &'n NestedRemovals<'a>) -> Self {
        Self {
            owners,
            nested,
            slots: BTreeSet::new(),
            nested_removals: Vec::new(),
            reads_all: false,
            leaf: Leaf::Other,
            tuples: Vec::new(),
            types: HashMap::new(),
        }
    }

    /// The declaration's signature: its runtime parameters, collectors,
    /// result, error, runtime defaults, and `where` clauses, and each
    /// compile-time parameter's type and constraints. Not a compile-time
    /// parameter's default, which the call site evaluates.
    fn signature(&mut self, declaration: &MirFunctionDeclaration) {
        for ty in declaration
            .param_types
            .iter()
            .chain(&declaration.variadic)
            .chain(&declaration.kw_variadic)
            .chain(std::iter::once(&declaration.ret_ty))
            .chain(&declaration.error_ty)
        {
            self.ty(ty);
        }
        self.visit(&declaration.defaults);
        self.visit(&declaration.availability);
        for decl in &declaration.param_decls {
            match decl {
                ParamDecl::Type {
                    callable_bound,
                    constraints,
                    ..
                } => {
                    if let Some(bound) = callable_bound {
                        self.ty(bound);
                    }
                    self.visit(constraints);
                }
                ParamDecl::Value {
                    ty, constraints, ..
                } => {
                    self.ty(ty);
                    self.visit(constraints);
                }
            }
        }
    }

    /// The body: its blocks, and the types of its signature, slots, and
    /// registers.
    fn body(&mut self, function: &MirFunction) {
        self.visit(&function.blocks);
        for ty in function
            .param_types
            .iter()
            .chain(function.var_tys.values())
            .chain(&function.ret_ty)
            .chain(&function.error_ty)
            .chain(function.reg_types.values())
        {
            self.ty(ty);
        }
    }

    fn ty(&mut self, ty: &Ty) {
        if !self.types.contains_key(ty) {
            let mut walker = Walker::new(self.owners.clone(), self.nested);
            walker.visit(ty);
            self.types.insert(
                ty.clone(),
                (walker.slots, walker.nested_removals, walker.reads_all),
            );
        }
        let (slots, nested_removals, reads_all) = &self.types[ty];
        self.slots.extend(slots);
        self.nested_removals.extend(nested_removals);
        self.reads_all |= reads_all;
    }

    /// Walk `value`. A leaf that encodes only in its compilation's context
    /// (a binding owner, a source location) names no binder, and its
    /// siblings are still walked; a value serde cannot encode at all (an
    /// unbound parameter hole) may name anything.
    fn visit<T: Serialize + ?Sized>(&mut self, value: &T) {
        let depth = self.tuples.len();
        if let Err(Unwalkable(opaque)) = value.serialize(&mut *self) {
            self.reads_all |= opaque;
        }
        self.tuples.truncate(depth);
    }

    fn string(&mut self, value: &str) {
        let owned = |name: &str| {
            self.owners.iter().any(|owner| {
                name.strip_prefix(owner)
                    .is_some_and(|rest| rest.starts_with('$'))
            })
        };
        match value
            .strip_prefix("$comptime$")
            .or_else(|| value.strip_prefix("$default$"))
        {
            Some(lifted) => self.reads_all |= owned(lifted),
            None if owned(value) => match (self.nested)(value) {
                Some(removed) => self.nested_removals.push(removed),
                None => self.reads_all = true,
            },
            None => {}
        }
        self.leaf = self
            .owners
            .iter()
            .position(|owner| *owner == value)
            .map_or(Leaf::Other, Leaf::Owner);
    }

    fn element<T: Serialize + ?Sized>(&mut self, value: &T) {
        self.leaf = Leaf::Other;
        self.visit(value);
        if let Some((length, index, owner)) = self.tuples.last_mut() {
            if *length == 2 {
                match (*index, self.leaf) {
                    (0, Leaf::Owner(named)) => *owner = Some(named),
                    (1, Leaf::Slot(slot)) if let Some(named) = *owner => {
                        self.slots.insert((named, slot as usize));
                    }
                    _ => {}
                }
            }
            *index += 1;
        }
    }

    const fn compound(&mut self) -> &mut Self {
        self.leaf = Leaf::Other;
        self
    }
}

/// A value the walk could not encode: `true` when no context would encode
/// it, `false` for a leaf only its compilation's context encodes.
#[derive(Debug)]
struct Unwalkable(bool);

impl fmt::Display for Unwalkable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a value the parameter-use walk cannot encode")
    }
}

impl std::error::Error for Unwalkable {}

impl ser::Error for Unwalkable {
    fn custom<T: fmt::Display>(message: T) -> Self {
        // serde's own refusal of a skipped variant, as against a leaf's
        // refusal outside its context.
        Self(message.to_string().contains("cannot be serialized"))
    }
}

impl ser::Serializer for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_str(self, value: &str) -> Result<(), Unwalkable> {
        self.string(value);
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Slot(value);
        Ok(())
    }

    fn serialize_bool(self, _: bool) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i8(self, _: i8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i16(self, _: i16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i32(self, _: i32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i64(self, _: i64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i128(self, _: i128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u8(self, _: u8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u16(self, _: u16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u32(self, _: u32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u128(self, _: u128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f32(self, _: f32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f64(self, _: f64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_char(self, _: char) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_tuple(self, length: usize) -> Result<Self, Unwalkable> {
        self.tuples.push((length, 0, None));
        Ok(self.compound())
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }
}

impl ser::SerializeSeq for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.element(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.tuples.pop();
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeMap for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unwalkable> {
        self.visit(key);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}
