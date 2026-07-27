//! Stage 5: flatten the HIR's nested expressions into
//! **A-Normal Form** (three-address code) — every subexpression becomes a
//! `MirInstr` writing a fresh [`Reg`], so `foo(bar(x))` becomes
//! `t0 = bar(x); t1 = foo(t0)`. The flattened form is what Stages 6–7 dataflow
//! analysis (liveness / move / borrow) runs over, and what the backends consume.
//!
//! One current limitation, intentional at this stage:
//! * **Fields/methods are name-based.** There is no type/layout info here, so
//!   member access keeps the field *name*; the backend resolves it to an offset
//!   (Tier-2) or a `Vec` index (Tier-1 VM).
//!
//! Entry points: [`lower_cfg`] turns one HIR [`Cfg`] (a function body) into a
//! [`MirFunction`]; [`lower_program`] lowers a whole program to one
//! [`MirFunction`] per `def` / struct method plus a synthetic `__toplevel__`.
//!
//! Writes go through a **place** ([`MirPlace`] = a root variable + [`Proj`]
//! chain), mirroring `rustc` MIR's place/rvalue split, so `p.items[i].x = e` and
//! `xs[i] += e` lower uniformly (indices evaluated once).

use crate::ast::{
    ArgConvention, Dtype, Expr, ExprKind, FnParam, InfixOp, ParamArg, ParamKind, PrefixOp, Stmt,
    StmtKind, TStringPart,
};
use crate::call::{effective_keyword_only_index, regular_marker_index};
use crate::checked::{AnnotationSite, GenericSite};
use crate::checked::{CheckedConst, CheckedProgram};
use crate::hir::{self, Cfg, HirInstr, Terminator, VarId};
use crate::token::{DUMMY_SPAN, SourceSpan};
use crate::types::{ParamDecl, Ty, TyArg, dict_elements, tuple_elements};
use std::collections::{HashMap, HashSet};

mod ir;
pub use ir::*;
pub mod verify;

/// An expression's source span, stamped by the parser (`ast::Expr.span`). Fed
/// into the [`SpanTable`] so each temporary can be traced back to its origin.
fn span(e: &Expr) -> SourceSpan {
    e.source_span()
}

/// Return a source integer literal as a host index without materializing it as
/// a runtime scalar. This intentionally recognizes only exact literal syntax:
/// arbitrary index expressions still lower through a register and retain their
/// ordinary evaluation semantics.
fn exact_nonnegative_index(expression: &Expr) -> Option<usize> {
    let ExprKind::Int(value) = &expression.kind else {
        return None;
    };
    value.to_u64().and_then(|value| usize::try_from(value).ok())
}

/// Derive the executable capability produced by `MakeRef` from the complete
/// typed place. A projection through an existing capability preserves that
/// capability's permission; borrowing ordinary storage creates a new handle
/// with the requested permission (mutable for legacy unchecked HIR).
fn mir_place_handle_ty(
    place: &MirPlace,
    requested: Option<crate::origin::Mutability>,
) -> Option<Ty> {
    let storage = place.ty.clone()?;
    let root = place.root_ty.as_ref()?;
    let source_mutability = match root {
        Ty::Ref(reference) => Some(reference.mutability),
        Ty::Pointer { origin, .. } => match origin {
            crate::origin::PointerOrigin::Place { mutable, .. } => Some(if *mutable {
                crate::origin::Mutability::Mutable
            } else {
                crate::origin::Mutability::Immutable
            }),
            crate::origin::PointerOrigin::Param { mutability, .. } => Some(*mutability),
            crate::origin::PointerOrigin::Legacy
            | crate::origin::PointerOrigin::Static
            | crate::origin::PointerOrigin::Untracked { .. }
            | crate::origin::PointerOrigin::UnsafeAny { .. } => None,
        },
        _ => None,
    };
    if source_mutability.is_some() && place.proj.is_empty() {
        return Some(root.clone());
    }
    let mutability = source_mutability
        .or(requested)
        .unwrap_or(crate::origin::Mutability::Mutable);
    Some(Ty::Ref(crate::origin::RefTy {
        referent: Box::new(storage),
        origin: crate::origin::Origin::Untracked {
            mutable: mutability != crate::origin::Mutability::Immutable,
        },
        mutability,
    }))
}

fn generic_callable_param_decls(callable: &Ty) -> Vec<ParamDecl> {
    match callable {
        Ty::GenericFunc { decls, .. } => decls.clone(),
        Ty::Param {
            callable_bound: Some(bound),
            ..
        } => generic_callable_param_decls(bound),
        _ => Vec::new(),
    }
}

/// A nested `def` lifted to a top-level function: the mangled name it becomes and
/// the enclosing locals it captures. Captures are passed as leading **`mut`**
/// parameters (so a read *or* a write of a captured variable works — reference
/// semantics via the existing write-back), prepended to a call by name.
#[derive(Clone)]
struct NestedInfo {
    binding: crate::origin::OwnerId,
    source_name: String,
    mangled: String,
    /// True when this declaration belongs to the function currently being
    /// lowered, so its statement creates a closure slot that later uses load.
    /// Inherited entries (including self-recursion) rebuild from forwarded
    /// environment parameters because their declaration slot lives in an outer
    /// frame.
    materialized_here: bool,
    /// Captured enclosing-local names, in a deterministic (sorted) order shared by
    /// the lifted function's parameter list and every rewritten call site.
    captures: Vec<NestedCapture>,
    /// The checker's exact callable type for the nested `def`, used to type
    /// synthetic closure-value registers. `None` only on unchecked paths.
    callable_ty: Option<Ty>,
}

#[derive(Clone, PartialEq, Eq)]
struct NestedCapture {
    name: String,
    binding: crate::origin::OwnerId,
    ty: Ty,
    kind: crate::ast::CaptureKind,
}

#[derive(Clone)]
struct ExprFacts {
    ty: Option<Ty>,
    place_ty: Option<Ty>,
    owner: Option<crate::origin::OwnerId>,
    raises: Option<Ty>,
    adjustments: Vec<crate::SemanticAdjustment>,
    comprehension_bindings: Vec<crate::checked::CheckedComprehensionBinding>,
}

fn expression_children(expression: &Expr) -> Vec<&Expr> {
    fn param_value(argument: &ParamArg) -> Option<&Expr> {
        match argument {
            ParamArg::Value(value) => Some(value),
            ParamArg::Named { value, .. } => match &**value {
                ParamArg::Value(value) => Some(value),
                ParamArg::Type(_) | ParamArg::Named { .. } => None,
            },
            ParamArg::Type(_) => None,
        }
    }

    match &expression.kind {
        ExprKind::Prefix(_, value)
        | ExprKind::Transfer(value)
        | ExprKind::Spread(value)
        | ExprKind::Named { value, .. } => {
            vec![value]
        }
        ExprKind::Infix(_, left, right)
        | ExprKind::Index {
            object: left,
            index: right,
        } => {
            vec![left, right]
        }
        ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } => param_args
            .iter()
            .filter_map(param_value)
            .chain(args.iter())
            .chain(kwargs.iter().map(|argument| &argument.value))
            .collect(),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => std::iter::once(callee.as_ref())
            .chain(param_args.iter().filter_map(param_value))
            .chain(args.iter())
            .chain(kwargs.iter().map(|argument| &argument.value))
            .collect(),
        ExprKind::Member { object, .. } => vec![object],
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => std::iter::once(object.as_ref())
            .chain(args.iter())
            .chain(kwargs.iter().map(|argument| &argument.value))
            .collect(),
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => values.iter().collect(),
        ExprKind::BraceLit(values) => values
            .iter()
            .flat_map(|(key, value)| std::iter::once(key).chain(value.iter()))
            .collect(),
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => clauses
            .iter()
            .map(|clause| match clause {
                crate::ast::ComprehensionClause::For { iter, .. } => iter.as_ref(),
                crate::ast::ComprehensionClause::If(condition) => condition.as_ref(),
            })
            .chain(key.iter().map(Box::as_ref))
            .chain(std::iter::once(value.as_ref()))
            .collect(),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => vec![cond, then_branch, else_branch],
        ExprKind::Compare { first, rest } => std::iter::once(first.as_ref())
            .chain(rest.iter().map(|(_, value)| value))
            .collect(),
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => std::iter::once(object.as_ref())
            .chain(
                [lower, upper, step]
                    .into_iter()
                    .filter_map(|value| value.as_deref()),
            )
            .collect(),
        ExprKind::MultiIndex { object, args } => {
            let mut children = vec![object.as_ref()];
            for argument in args {
                match argument {
                    crate::ast::SubscriptArg::Index(value) => children.push(value),
                    crate::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    } => {
                        children.extend([lower, upper, step].into_iter().flatten().map(Box::as_ref))
                    }
                }
            }
            children
        }
        ExprKind::TString { parts, .. } => parts
            .iter()
            .filter_map(|part| match part {
                TStringPart::Expr(value) => Some(value.as_ref()),
                TStringPart::Literal(_) => None,
            })
            .collect(),
        ExprKind::TypeApply { args, .. } => args.iter().filter_map(param_value).collect(),
        _ => Vec::new(),
    }
}

fn statement_expression_roots(statement: &Stmt) -> Vec<&Expr> {
    match &statement.kind {
        StmtKind::VarDecl { value, .. }
        | StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Comptime { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Expr(value) => vec![value],
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            vec![place, value]
        }
        StmtKind::Unpack { targets, value } => {
            let mut roots: Vec<&Expr> = targets.iter().collect();
            roots.push(value);
            roots
        }
        StmtKind::Return(Some(value)) => vec![value],
        StmtKind::With { items, .. } => items.iter().map(|item| &item.context).collect(),
        _ => Vec::new(),
    }
}

fn index_hir_expression(
    syntax: &Expr,
    expression: &crate::hir::HirExpr,
    index: &mut HashMap<usize, ExprFacts>,
) {
    index.insert(
        syntax as *const Expr as usize,
        ExprFacts {
            ty: expression.ty.clone(),
            place_ty: expression.place.as_ref().map(|place| place.ty.clone()),
            owner: expression
                .binding
                .or_else(|| expression.place.as_ref().map(|place| place.owner)),
            raises: expression.effects.raises.clone(),
            adjustments: expression.adjustments.clone(),
            comprehension_bindings: expression.comprehension_bindings.clone(),
        },
    );
    for (child_syntax, child) in expression_children(syntax)
        .into_iter()
        .zip(&expression.children)
    {
        index_hir_expression(child_syntax, child, index);
    }
}

/// Flattens nested `Expr`s into a block's instruction list. `cur` is the block
/// currently being appended to.
struct Flatten<'a> {
    f: &'a mut MirFunction,
    cur: MirBlockId,
    next_reg: u32,
    /// Interner: a variable name's first appearance assigns its `VarId`.
    vars: Vec<String>,
    /// Checked storage type for each interned variable. This is populated from
    /// checked parameters/uses and `HirInstr::Bind` before places are emitted.
    var_types: HashMap<VarId, Ty>,
    /// Runtime slots assigned to checked binding identities that do not have a
    /// statement-level HIR declaration, notably comprehension generators.
    owner_vars: HashMap<crate::origin::OwnerId, VarId>,
    /// Nested `def`s in scope (name → lifted target + captures); a call to one is
    /// rewritten to the mangled function with its captures prepended, and the
    /// nested `def` statement itself lowers to nothing.
    nested: HashMap<crate::origin::OwnerId, NestedInfo>,
    /// The program's overloaded declarations. Kept only for unchecked HIR tests;
    /// production lowering consumes `ResolveCallable` checked adjustments.
    overloads: crate::symbol::OverloadSets,
    checked_expressions: HashMap<crate::CheckedNodeId, crate::CheckedExpr>,
    checked_declarations: Vec<crate::CheckedDeclaration>,
    /// Semantic facts indexed by the in-memory identity of the active HIR syntax
    /// tree. Maps are installed only while lowering that expression/statement;
    /// source spans are never used as semantic keys.
    active_semantics: Vec<HashMap<usize, ExprFacts>>,
    /// Local reference slot to its frozen owner place and permission.
    aliases: HashMap<VarId, MirLoan>,
    runtime_aliases: std::collections::HashSet<VarId>,
    /// Persistent owner loans carried by reference-bearing aggregate variables.
    /// The runtime value contains the handles; this map transfers their static
    /// loans when an aggregate is moved or forwarded into a new binding.
    aggregate_loans: HashMap<VarId, Vec<MirLoan>>,
    /// Names rebound more than once, or captured by a nested `def`. A pointer
    /// variable outside this set keeps one statically known loan place for its
    /// whole live range, so deref sites may substitute the owner place.
    reassigned_names: std::collections::HashSet<String>,
    returns_reference: bool,
}

/// Borrowed syntax and checked binder data for one structured `try`.
/// Keeping these parts together makes the primary HIR and fallback lowering
/// paths share one region-lowering contract.
struct TryRegions<'a> {
    body: &'a [Stmt],
    except: &'a Option<(Option<String>, Vec<Stmt>)>,
    orelse: &'a Option<Vec<Stmt>>,
    finalbody: &'a Option<Vec<Stmt>>,
    handler_binding: Option<crate::origin::OwnerId>,
}

/// The invariant construction state shared by every recursive level of a
/// collection comprehension. Only the clause cursor and checked bindings vary
/// as the nested control-flow tree is emitted.
struct ComprehensionPlan<'a> {
    collection: VarId,
    target: &'a Ty,
    insert: &'a str,
    key: Option<&'a Expr>,
    value: &'a Expr,
}

/// Names whose binding may change after the first assignment (or that a nested
/// `def` captures). CFG-lowered rebindings appear as `HirInstr::Bind`; opaque
/// statements — notably `try` regions, whose sub-CFGs lower separately — are
/// scanned recursively.
fn reassigned_names(
    cfg: &Cfg,
    nested: &HashMap<crate::origin::OwnerId, NestedInfo>,
) -> std::collections::HashSet<String> {
    fn bump(counts: &mut HashMap<String, usize>, name: &str) {
        *counts.entry(name.to_string()).or_default() += 1;
    }
    fn scan(stmt: &Stmt, counts: &mut HashMap<String, usize>) {
        match &stmt.kind {
            StmtKind::VarDecl { name, .. }
            | StmtKind::Assign { name, .. }
            | StmtKind::RefDecl { name, .. }
            | StmtKind::Comptime { name, .. } => bump(counts, name),
            StmtKind::AugAssign { place, .. } | StmtKind::SetPlace { place, .. } => {
                if let ExprKind::Identifier(name) = &place.kind {
                    bump(counts, name);
                }
            }
            StmtKind::Unpack { targets, .. } => {
                for target in targets {
                    if let ExprKind::Identifier(name) = &target.kind {
                        bump(counts, name);
                    }
                }
            }
            StmtKind::If { branches, orelse } => {
                for (_, body) in branches {
                    for inner in body {
                        scan(inner, counts);
                    }
                }
                for inner in orelse.iter().flatten() {
                    scan(inner, counts);
                }
            }
            StmtKind::While { body, orelse, .. } => {
                for inner in body.iter().chain(orelse.iter().flatten()) {
                    scan(inner, counts);
                }
            }
            StmtKind::For {
                var, body, orelse, ..
            } => {
                bump(counts, var);
                for inner in body.iter().chain(orelse.iter().flatten()) {
                    scan(inner, counts);
                }
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let handler = except.iter().flat_map(|(_, handler)| handler.iter());
                for inner in body
                    .iter()
                    .chain(handler)
                    .chain(orelse.iter().flatten())
                    .chain(finalbody.iter().flatten())
                {
                    scan(inner, counts);
                }
            }
            _ => {}
        }
    }
    let mut counts = HashMap::new();
    for hb in cfg.g.node_indices() {
        for instr in &cfg.g[hb].instrs {
            match instr {
                HirInstr::Bind { dest, .. } => {
                    if let Some(name) = cfg.vars.get(*dest as usize) {
                        bump(&mut counts, name);
                    }
                }
                HirInstr::Stmt(statement) => scan(&statement.syntax, &mut counts),
                _ => {}
            }
        }
    }
    for info in nested.values() {
        for capture in &info.captures {
            bump(&mut counts, &capture.name);
            bump(&mut counts, &capture.name);
        }
    }
    counts
        .into_iter()
        .filter(|(_, occurrences)| *occurrences > 1)
        .map(|(name, _)| name)
        .collect()
}

impl Flatten<'_> {
    fn facts(&self, expression: &Expr) -> Option<&ExprFacts> {
        let key = expression as *const Expr as usize;
        self.active_semantics
            .iter()
            .rev()
            .find_map(|index| index.get(&key))
    }

    fn checked_ty(&self, expression: &Expr) -> Option<Ty> {
        self.facts(expression).and_then(|facts| facts.ty.clone())
    }

    fn checked_place_ty(&self, expression: &Expr) -> Option<Ty> {
        self.facts(expression)
            .and_then(|facts| facts.place_ty.clone())
    }

    fn checked_raises(&self, expression: &Expr) -> Option<Ty> {
        self.checked_call_contract(expression)
            .and_then(|contract| contract.raises)
            .or_else(|| {
                self.facts(expression)
                    .and_then(|facts| facts.raises.clone())
            })
    }

    fn checked_owner(&self, expression: &Expr) -> Option<crate::origin::OwnerId> {
        self.facts(expression).and_then(|facts| facts.owner)
    }

    fn comprehension_bindings(
        &self,
        expression: &Expr,
    ) -> Vec<crate::checked::CheckedComprehensionBinding> {
        self.facts(expression)
            .map(|facts| facts.comprehension_bindings.clone())
            .unwrap_or_default()
    }

    fn expression_var(&mut self, name: &str, expression: &Expr) -> VarId {
        if let Some(owner) = self.checked_owner(expression) {
            return self.binding_var(owner, name);
        }
        self.var(name)
    }

    fn nested_info(&self, expression: &Expr) -> Option<NestedInfo> {
        self.checked_owner(expression)
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
    }
    /// Every owner loan carried into an aggregate expression.  An aggregate may
    /// contain more than one reference-valued field, so this must remain plural:
    /// keeping only the first borrow makes later fields dangling-capable.
    fn aggregate_borrows(&mut self, expression: &Expr) -> Vec<MirLoan> {
        let borrow = self
            .checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::BorrowShared => Some(false),
                crate::SemanticAdjustment::BorrowMutable => Some(true),
                _ => None,
            });
        if let Some(mutable) = borrow
            && let ExprKind::Identifier(name) = &expression.kind
        {
            let var = self.expression_var(name, expression);
            if let Some(loans) = self.aggregate_loans.get(&var) {
                return loans
                    .iter()
                    .cloned()
                    .map(|mut loan| {
                        loan.mutable = mutable;
                        loan
                    })
                    .collect();
            }
            return self
                .aliases
                .get(&var)
                .cloned()
                .map(|mut loan| {
                    loan.mutable = mutable;
                    vec![loan]
                })
                .unwrap_or_default();
        }
        if let Some(mutable) = borrow
            && matches!(
                expression.kind,
                ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::TypeApply { .. }
            )
        {
            let place = self.place(expression);
            let interiors = self.checked_interior_references(expression);
            if interiors.is_empty() {
                return vec![MirLoan {
                    place,
                    mutable,
                    interior: None,
                }];
            }
            return interiors
                .into_iter()
                .filter_map(|origin| {
                    self.mir_interior_origin(&origin, Some(place.root))
                        .map(|interior| MirLoan {
                            place: place.clone(),
                            mutable,
                            interior: Some(interior),
                        })
                })
                .collect();
        }
        if let ExprKind::Identifier(name) = &expression.kind {
            let var = self.expression_var(name, expression);
            if let Some(loans) = self.aggregate_loans.get(&var) {
                return loans.clone();
            }
        }
        match &expression.kind {
            ExprKind::Call { args, kwargs, .. } => {
                // A checked pointer construction loans exactly its source
                // place, with the mutability the checker inferred from the
                // owner binding.
                if let Some(crate::SemanticAdjustment::PointerToPlace { mutable }) = self
                    .checked_adjustments(expression)
                    .into_iter()
                    .find(|adjustment| {
                        matches!(adjustment, crate::SemanticAdjustment::PointerToPlace { .. })
                    })
                {
                    let place = self.place(
                        &kwargs
                            .first()
                            .expect("checked pointer construction has a 'to=' argument")
                            .value,
                    );
                    return vec![MirLoan {
                        place,
                        mutable,
                        interior: None,
                    }];
                }
                args.iter()
                    .chain(kwargs.iter().map(|argument| &argument.value))
                    .flat_map(|argument| self.aggregate_borrows(argument))
                    .collect()
            }
            ExprKind::Transfer(inner) => self.aggregate_borrows(inner),
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => values
                .iter()
                .flat_map(|value| self.aggregate_borrows(value))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn checked_adjustments(&self, expression: &Expr) -> Vec<crate::SemanticAdjustment> {
        self.facts(expression)
            .map(|facts| facts.adjustments.clone())
            .unwrap_or_default()
    }

    fn tuple_unpack_plan(
        &self,
        expression: &Expr,
    ) -> Option<Vec<crate::checked::CheckedTupleUnpackElement>> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::TupleUnpack { elements } => Some(elements),
                _ => None,
            })
    }

    fn instantiated_callable_contract(&self, expression: &Expr) -> Option<(Ty, Vec<TyArg>)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                } => Some((contract, arguments)),
                _ => None,
            })
    }

    fn checked_call_contract(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedCallContract> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SelectedCall(contract) => Some(*contract),
                _ => None,
            })
    }

    fn checked_augmented_subscript(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedAugmentedSubscript> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::AugmentedSubscript(contract) => Some(*contract),
                _ => None,
            })
    }

    fn subscript_call_contract(
        &self,
        expression: &Expr,
        evaluated: &[(SourceSpan, Reg)],
    ) -> Option<MirSubscriptCall> {
        let contract = self.checked_call_contract(expression)?;
        Some(self.mir_subscript_call_contract(contract, evaluated))
    }

    fn mir_subscript_call_contract(
        &self,
        contract: crate::checked::CheckedCallContract,
        evaluated: &[(SourceSpan, Reg)],
    ) -> MirSubscriptCall {
        let capture_accesses = contract
            .captures
            .iter()
            .filter_map(|capture| {
                let crate::origin::Origin::Place(place) = &capture.origin else {
                    return None;
                };
                self.owner_vars
                    .get(&place.root)
                    .copied()
                    .map(|root| MirCaptureAccess {
                        root,
                        path: place.path.clone(),
                        access: capture.access,
                    })
            })
            .collect();
        let param_arg_regs = contract
            .parameter_arguments
            .iter()
            .map(|argument| MirParamArg {
                name: argument.name.clone(),
                value: argument.value_source.as_ref().and_then(|source| {
                    evaluated.iter().find_map(|(candidate, register)| {
                        (candidate == source).then_some(*register)
                    })
                }),
            })
            .collect();
        MirSubscriptCall {
            target: contract.target,
            raises: contract.raises,
            result_ty: contract.result_ty,
            receiver_requires_place: contract.receiver_requires_place,
            receiver_convention: contract.receiver_convention,
            arguments: contract.arguments,
            capture_accesses,
            reference_result: contract.reference_result,
            param_arg_regs,
            param_decls: contract.param_decls,
        }
    }

    fn checked_call_capture_accesses(&self, expression: &Expr) -> Vec<MirCaptureAccess> {
        let captures = self
            .checked_call_contract(expression)
            .map(|contract| contract.captures)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::CallableCaptureAccesses(captures) => {
                            Some(captures)
                        }
                        _ => None,
                    })
            });
        captures
            .map(|captures| {
                captures
                    .into_iter()
                    .filter_map(|capture| {
                        let crate::origin::Origin::Place(place) = capture.origin else {
                            return None;
                        };
                        self.owner_vars
                            .get(&place.root)
                            .copied()
                            .map(|root| MirCaptureAccess {
                                root,
                                path: place.path,
                                access: capture.access,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn checked_borrow_mutability(&self, expression: &Expr) -> Option<bool> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::BorrowShared => Some(false),
                crate::SemanticAdjustment::BorrowMutable => Some(true),
                _ => None,
            })
    }

    /// Lower one ordinary call argument together with the caller storage that
    /// checking selected for a `mut`/`ref` parameter. Dynamic indexed places
    /// cannot be reconstructed after evaluating their value: doing so either
    /// evaluates the index twice or loses the write-back place altogether.
    /// Flatten those actuals once and retain the resulting typed place. A
    /// nominal accessor-produced reference uses the same hidden handle slot as
    /// a chained method receiver.
    fn lower_call_argument(&mut self, expression: &Expr) -> (Reg, Option<MirPlace>) {
        let retains_place = self
            .checked_adjustments(expression)
            .iter()
            .any(|adjustment| matches!(adjustment, crate::SemanticAdjustment::RetainCallPlace));
        if !retains_place {
            return (self.expr(expression), None);
        }

        // A pure root/field place needs no emitted projection state, so keep
        // the existing expression lowering (notably its reference-field
        // handling) and attach the place afterward.
        if let Some(place) = self.simple_place(expression) {
            return (self.expr(expression), Some(place));
        }

        if self.reference_result(expression).is_some() {
            return self.lower_call_receiver(expression);
        }

        // `container[index].field` is not a raw place when the selected
        // `__getitem__` returns a reference. Evaluate that accessor once into
        // its hidden caller-handle slot, then retain the ordinary projections
        // below the returned referent. Falling through to `try_place` would
        // manufacture `container[Index].field` and bypass the selected call.
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        // The checker rejects a non-place actual for a place-requiring
        // parameter. Keep lowering total so the verifier can diagnose corrupt
        // checked input without manufacturing a caller place.
        (self.expr(expression), None)
    }

    /// Evaluate an augmented-subscript argument before either accessor call,
    /// without applying a conversion selected for one particular accessor.
    /// Getter and setter contracts may adapt the same source expression to
    /// different parameter types, but Mojo still evaluates that expression
    /// exactly once.
    fn expr_without_call_value_adjustments(&mut self, expression: &Expr) -> Reg {
        if let Some(reference) = self.reference_result(expression) {
            let handle = self.reference_handle(expression);
            let value_ty = (*reference.referent).clone();
            let read = self.fresh_typed(expression.source_span(), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            let copied = self.fresh_typed(expression.source_span(), None, value_ty);
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: read,
            });
            return copied;
        }

        let value = self.expr_unconverted(expression);
        if let Some(ty) = self.checked_ty(expression) {
            self.f.reg_types.entry(value.0).or_insert(ty);
        }
        value
    }

    /// Lower one source operand shared by the getter and setter of an
    /// augmented subscript. `retain_place` is the union of both call contracts:
    /// it lets a mutating getter write back now and lets lowering reload that
    /// updated value before the setter, without re-evaluating the source.
    fn lower_augmented_argument_source(
        &mut self,
        expression: &Expr,
        retain_place: bool,
    ) -> (Reg, Option<MirPlace>) {
        if !retain_place {
            return (self.expr_without_call_value_adjustments(expression), None);
        }
        if let Some(place) = self.simple_place(expression) {
            return (
                self.expr_without_call_value_adjustments(expression),
                Some(place),
            );
        }
        if self.reference_result(expression).is_some() {
            return self.lower_call_receiver(expression);
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        (self.expr_without_call_value_adjustments(expression), None)
    }

    fn checked_call_source_requires_place(
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract
            .arguments
            .iter()
            .any(|argument| argument.source == source && argument.requires_place)
    }

    fn checked_call_source_mutates(
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract.arguments.iter().any(|argument| {
            argument.source == source
                && matches!(
                    argument.convention,
                    Some(
                        crate::ast::ArgConvention::Mut
                            | crate::ast::ArgConvention::Ref
                            | crate::ast::ArgConvention::Out
                    )
                )
        })
    }

    fn checked_call_source_place(
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
        place: &Option<MirPlace>,
    ) -> Option<MirPlace> {
        Self::checked_call_source_requires_place(contract, source)
            .then(|| place.clone())
            .flatten()
    }

    /// Apply the adaptations frozen on one selected call to an already
    /// evaluated source register. This deliberately ignores the expression's
    /// compatibility adjustment table: getter and setter facts can share the
    /// same source span and must not overwrite each other.
    fn apply_checked_call_value_adjustments(
        &mut self,
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
        raw: Reg,
        site: SourceSpan,
    ) -> Reg {
        let parameter_ty = contract
            .arguments
            .iter()
            .find(|argument| argument.source == source)
            .map(|argument| argument.parameter_ty.clone());
        let adjustments = contract
            .boundary
            .arguments
            .iter()
            .find(|argument| argument.source == source)
            .map(|argument| argument.adjustments.as_slice())
            .unwrap_or_default();
        let mut value = raw;
        for adjustment in adjustments {
            value = match adjustment {
                crate::checked::CheckedCallValueAdjustment::ResolveCallable { target } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::Function(target.clone()),
                    });
                    dest
                }
                crate::checked::CheckedCallValueAdjustment::ImplicitConversion { target } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::Call {
                        dest,
                        func: FuncRef::named(target),
                        raises: None,
                        args: vec![value],
                        kwargs: Vec::new(),
                        arg_places: vec![None],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                    });
                    dest
                }
                crate::checked::CheckedCallValueAdjustment::IndexNormalization { target } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or(Ty::Int),
                    );
                    self.emit(MirInstr::MethodCall {
                        dest,
                        recv: value,
                        method: "__mlir_index__".to_string(),
                        resolved: Some(target.clone()),
                        raises: None,
                        args: Vec::new(),
                        kwargs: Vec::new(),
                        recv_place: None,
                        arg_places: Vec::new(),
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                    });
                    dest
                }
                crate::checked::CheckedCallValueAdjustment::MaterializeLiteral { target } => {
                    let target = target.as_ref().clone();
                    let dest = self.fresh_typed(site.clone(), None, target.clone());
                    self.emit(MirInstr::MaterializeLiteral {
                        dest,
                        value,
                        target,
                    });
                    dest
                }
            };
        }
        if adjustments.is_empty()
            && let Some(parameter_ty) = parameter_ty
        {
            value = self.materialize_register(value, &parameter_ty, site);
        }
        value
    }

    fn emit_checked_call_boundary(
        &mut self,
        contract: &crate::checked::CheckedCallContract,
        site: SourceSpan,
    ) {
        for argument in &contract.boundary.arguments {
            self.emit_interior_invalidation_facts(
                &argument.invalidations,
                argument.value_source.clone(),
                None,
            );
        }
        self.emit_interior_invalidation_facts(&contract.boundary.invalidations, site, None);
    }

    fn reload_augmented_source(
        &mut self,
        raw: Reg,
        place: &Option<MirPlace>,
        mutated: bool,
        site: SourceSpan,
    ) -> Reg {
        if !mutated {
            return raw;
        }
        let Some(place) = place else {
            return raw;
        };
        let value = self.fresh_typed(
            site,
            Some(place.root),
            place.ty.clone().unwrap_or(Ty::Error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        value
    }

    fn lower_call_arguments(&mut self, arguments: &[Expr]) -> (Vec<Reg>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument(argument);
            registers.push(register);
            places.push(place);
        }
        (registers, places)
    }

    fn lower_call_keywords(
        &mut self,
        arguments: &[crate::ast::KwArg],
    ) -> (Vec<(String, Reg)>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument(&argument.value);
            registers.push((argument.name.clone(), register));
            places.push(place);
        }
        (registers, places)
    }

    /// Store an accessor-produced reference in a hidden local and establish its
    /// checked owner loans.  This turns the handle into the same persistent,
    /// analyzable call-place representation as an explicit `ref` binding while
    /// evaluating the accessor exactly once.
    fn materialize_call_reference_place(
        &mut self,
        expression: &Expr,
        handle: Reg,
        reference: crate::origin::RefTy,
    ) -> MirPlace {
        let variable = self.var(&format!("$call_ref_r{}", handle.0));
        let storage_ty = Ty::Ref(reference.clone());
        self.var_types.insert(variable, storage_ty.clone());
        self.runtime_aliases.insert(variable);
        self.emit(MirInstr::DefVar {
            var: variable,
            src: handle,
            binding_ty: Some(storage_ty.clone()),
        });

        let mut loans = Vec::new();
        for origin in self.checked_reference_places(expression) {
            let Some(canonical) = self.mir_interior_origin(&origin, None) else {
                continue;
            };
            let interior = canonical
                .path
                .iter()
                .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                .then_some(canonical.clone());
            loans.push(MirLoan {
                place: MirPlace::root(canonical.root, self.var_types.get(&canonical.root).cloned()),
                mutable: reference.mutability == crate::origin::Mutability::Mutable,
                interior,
            });
        }
        if !loans.is_empty() {
            let marker = self.fresh_typed(
                expression.source_span(),
                Some(loans[0].place.root),
                Ty::None,
            );
            self.emit(MirInstr::EstablishLoans {
                reference: variable,
                loans: loans.clone(),
                marker,
            });
            self.aggregate_loans.insert(variable, loans);
        }

        let mut place = MirPlace::root(variable, Some(storage_ty));
        place.ty = Some((*reference.referent).clone());
        place.through = Some(variable);
        place
    }

    /// Materialize one checker-selected reference-returning expression as a
    /// stable hidden caller place without reading its referent. Projection
    /// chains reuse this handle, so the selected accessor is evaluated exactly
    /// once and the VM never has to reinterpret a nominal index as raw storage.
    fn materialize_reference_result_place(&mut self, expression: &Expr) -> Option<MirPlace> {
        let reference = self.reference_result(expression)?;
        let handle = self.reference_handle(expression);
        // `reference_handle` may peel an outer reference-valued aggregate layer.
        // Materialize the handle's actual type rather than recreating `ref ref T`.
        let materialized_reference = match self.f.reg_types.get(&handle.0) {
            Some(Ty::Ref(reference)) => reference.clone(),
            _ => reference,
        };
        Some(self.materialize_call_reference_place(expression, handle, materialized_reference))
    }

    /// Lower ordinary field/intrinsic-index projections whose base is produced
    /// by a reference-returning call. Nominal index steps keep their own checked
    /// call path and are deliberately not generalized into raw projections.
    fn lower_projected_reference_place(&mut self, expression: &Expr) -> Option<MirPlace> {
        let base_place = |this: &mut Self, base: &Expr| {
            if this.reference_result(base).is_some() {
                this.materialize_reference_result_place(base)
            } else {
                this.lower_projected_reference_place(base)
            }
        };
        match &expression.kind {
            ExprKind::Member { object, field } => {
                let mut place = base_place(self, object)?;
                if let Some(ty) = self
                    .checked_place_ty(expression)
                    .or_else(|| self.checked_ty(expression))
                {
                    place.project(Proj::Field(field.clone()), ty);
                } else {
                    place.proj.push(Proj::Field(field.clone()));
                }
                Some(place)
            }
            ExprKind::Index { object, index }
                if self.checked_call_contract(expression).is_none()
                    && matches!(
                        self.intrinsic_index_dispatch(object),
                        Some(
                            MirIntrinsicSubscript::TupleStorage
                                | MirIntrinsicSubscript::VariadicStorage
                                | MirIntrinsicSubscript::Simd
                                | MirIntrinsicSubscript::Pointer
                        )
                    ) =>
            {
                let mut place = base_place(self, object)?;
                let projection = match self.checked_ty(object) {
                    Some(Ty::Tuple(_)) => exact_nonnegative_index(index)
                        .map(Proj::ConstIndex)
                        .unwrap_or_else(|| Proj::Index(self.expr(index))),
                    _ => Proj::Index(self.expr(index)),
                };
                if let Some(ty) = self
                    .checked_place_ty(expression)
                    .or_else(|| self.checked_ty(expression))
                {
                    place.project(projection, ty);
                } else {
                    place.proj.push(projection);
                }
                Some(place)
            }
            _ => None,
        }
    }

    /// Evaluate a call receiver and retain its executable place when checking
    /// selected reference/write-back semantics. Accessor-produced references
    /// become hidden reference locals; value-returning accessors remain values
    /// and are never reconstructed as raw index projections.
    fn lower_call_receiver(&mut self, expression: &Expr) -> (Reg, Option<MirPlace>) {
        if let Some(place) = self.materialize_reference_result_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place.ty.clone().unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if self.checked_call_contract(expression).is_some() {
            return (self.expr(expression), None);
        }
        match self.try_place(expression) {
            Some(place) => {
                let value = self.fresh(expression.source_span(), Some(place.root));
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                (value, Some(place))
            }
            None => (self.expr(expression), None),
        }
    }

    /// Retain storage for any callable place. Nominal callable receivers use it
    /// for `mut self`; declaration-owned closure environments use it so their
    /// copy/move capture slots are borrowed in place across repeated calls.
    fn callable_receiver_place(&mut self, expression: &Expr) -> Option<MirPlace> {
        let place = self.simple_place(expression)?;
        place.is_typed().then_some(place)
    }

    fn checked_reference_places(&self, expression: &Expr) -> Vec<crate::origin::OriginPlace> {
        fn collect(origin: &crate::origin::Origin, places: &mut Vec<crate::origin::OriginPlace>) {
            match origin {
                crate::origin::Origin::Place(place) => places.push(place.clone()),
                crate::origin::Origin::Union(members) => {
                    for member in members {
                        collect(member, places);
                    }
                }
                _ => {}
            }
        }

        let mut places = Vec::new();
        for adjustment in self.checked_adjustments(expression) {
            match adjustment {
                crate::SemanticAdjustment::InteriorReference { origin } => places.push(origin),
                crate::SemanticAdjustment::ReferenceResult { reference } => {
                    collect(&reference.origin, &mut places);
                }
                _ => {}
            }
        }
        if let Some(Ty::Ref(reference)) = self.checked_ty(expression) {
            collect(&reference.origin, &mut places);
        }
        places.sort();
        places.dedup();
        places
    }

    fn checked_interior_references(&self, expression: &Expr) -> Vec<crate::origin::OriginPlace> {
        self.checked_reference_places(expression)
            .into_iter()
            .filter(|place| {
                place
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
            })
            .collect()
    }

    fn mir_interior_origin(
        &mut self,
        origin: &crate::origin::OriginPlace,
        fallback: Option<VarId>,
    ) -> Option<MirInteriorOrigin> {
        let root = self.owner_vars.get(&origin.root).copied().or(fallback)?;
        self.owner_vars.entry(origin.root).or_insert(root);
        Some(MirInteriorOrigin {
            root,
            path: origin.path.clone(),
        })
    }

    fn checked_interior_invalidations(
        &self,
        expression: &Expr,
    ) -> Vec<crate::checked::InteriorInvalidation> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::InvalidateInteriors { invalidations } => {
                    Some(invalidations)
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Emit checker-selected invalidations at the precise operation boundary.
    /// `fallback` is used only for a whole-binding redefinition whose target
    /// owner has no expression occurrence before this instruction.
    fn emit_interior_invalidations(&mut self, expression: &Expr, fallback: Option<VarId>) {
        let invalidations = self.checked_interior_invalidations(expression);
        self.emit_interior_invalidation_facts(&invalidations, expression.source_span(), fallback);
    }

    fn emit_interior_invalidation_facts(
        &mut self,
        invalidations: &[crate::checked::InteriorInvalidation],
        site: SourceSpan,
        fallback: Option<VarId>,
    ) {
        for invalidation in invalidations {
            let Some(base) = self.mir_interior_origin(&invalidation.base, fallback) else {
                // Establishing an interior generation installs this checked
                // OwnerId's MIR slot. If no mapping exists, no earlier live
                // generation in this function can match the fact. Skipping also
                // keeps a same-span fact from another specialized clone inert.
                continue;
            };
            let except = invalidation
                .except
                .and_then(|owner| self.owner_vars.get(&owner).copied());
            let marker = self.fresh_typed(site.clone(), Some(base.root), Ty::None);
            self.emit(MirInstr::InvalidateInteriors {
                base,
                except,
                include_base_generation: invalidation.include_base_generation,
                marker,
            });
        }
    }

    /// Emit all invalidations whose semantic boundary is this call.  Argument
    /// facts are deliberately delayed until every argument has been evaluated:
    /// the callee, rather than evaluation of the place expression, performs the
    /// mutation.
    fn emit_call_invalidations(
        &mut self,
        call: &Expr,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) {
        for argument in args {
            self.emit_interior_invalidations(argument, None);
        }
        for argument in kwargs {
            self.emit_interior_invalidations(&argument.value, None);
        }
        self.emit_interior_invalidations(call, None);
    }

    /// Whether an expression's checked type is a pointer whose provenance
    /// designates checked storage. Dereferencing such a pointer goes through
    /// its frame/slot handle instead of allocation arithmetic.
    fn is_origin_bearing_pointer(&self, expression: &Expr) -> bool {
        matches!(
            self.checked_ty(expression),
            Some(Ty::Pointer { origin, .. }) if origin.as_origin().is_some()
        )
    }

    /// The statically known storage place behind a stably bound origin-bearing
    /// pointer variable. Substituting it at deref sites touches the owner at
    /// each access — the liveness contract `ref` aliases have — so ASAP
    /// destruction and loan conflicts stay exact. A reassigned or captured
    /// pointer, or a handle loaded from a field, reads through its runtime
    /// handle instead.
    fn pointer_deref_place(&mut self, object: &Expr) -> Option<MirPlace> {
        let ExprKind::Identifier(name) = &object.kind else {
            return None;
        };
        if !self.is_origin_bearing_pointer(object) || self.reassigned_names.contains(name) {
            return None;
        }
        let var = self.expression_var(name, object);
        let loans = self.aggregate_loans.get(&var)?;
        let [loan] = loans.as_slice() else {
            return None;
        };
        let mut place = loan.place.clone();
        place.through = Some(var);
        Some(place)
    }

    fn resolved_callable(&self, expression: &Expr) -> Option<String> {
        self.checked_call_contract(expression)
            .map(|contract| contract.target)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::ResolveCallable(target) => Some(target),
                        _ => None,
                    })
            })
    }

    fn implicit_conversion(&self, expression: &Expr) -> Option<String> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ImplicitConversion(target) => Some(target),
                _ => None,
            })
    }

    fn index_normalization(&self, expression: &Expr) -> Option<String> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::IndexNormalization { target } => Some(target),
                _ => None,
            })
    }

    fn implicitly_copies_consuming_receiver(&self, expression: &Expr) -> bool {
        self.checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::ImplicitlyCopyConsumingReceiver
                )
            })
    }

    fn literal_materialization(&self, expression: &Expr) -> Option<Ty> {
        if !matches!(
            self.checked_ty(expression),
            Some(Ty::IntLiteral | Ty::FloatLiteral)
        ) {
            // Checker fact tables are still keyed by source span while HIR owns
            // stable node identities. Comptime expansion can produce several
            // nodes at one source span; only a node whose checked source type is
            // actually a literal may consume a materialization adjustment.
            return None;
        }
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::MaterializeLiteral(target) => Some(target),
                _ => None,
            })
    }

    fn is_slice_descriptor(&self, expression: &Expr) -> bool {
        matches!(
            self.checked_ty(expression),
            Some(Ty::Struct(name, args))
                if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                    && args.is_empty()
        )
    }

    fn intrinsic_index_dispatch(&self, object: &Expr) -> Option<MirIntrinsicSubscript> {
        let ty = self.checked_ty(object)?;
        match &ty {
            Ty::Tuple(_) | Ty::RuntimePack(_) => Some(MirIntrinsicSubscript::TupleStorage),
            Ty::VariadicPack(_) => Some(MirIntrinsicSubscript::VariadicStorage),
            Ty::Simd { .. } => Some(MirIntrinsicSubscript::Simd),
            Ty::Pointer { .. } => Some(MirIntrinsicSubscript::Pointer),
            Ty::ComptimeList(_) => Some(MirIntrinsicSubscript::ComptimeList),
            // `Slice.indices` has a public nominal Tuple type but crosses the
            // VM intrinsic boundary as compiler-private `Value::Tuple`
            // storage. A checker-selected public Tuple accessor carries a
            // call contract and therefore never consults this fallback.
            Ty::Struct(..) if tuple_elements(&ty).is_some() => {
                Some(MirIntrinsicSubscript::TupleStorage)
            }
            _ => None,
        }
    }

    fn intrinsic_slice_dispatch(&self, object: &Expr) -> Option<MirIntrinsicSubscript> {
        matches!(self.checked_ty(object), Some(Ty::String)).then_some(MirIntrinsicSubscript::String)
    }

    /// Peel container slots that themselves store reference handles until one
    /// read through the returned handle yields `target`. This distinguishes
    /// writing a `List[ref T]` element's referent from replacing the stored
    /// `ref T` handle.
    fn peel_reference_handle_to(&mut self, mut handle: Reg, target: &Ty, site: SourceSpan) -> Reg {
        while let Some(Ty::Ref(outer)) = self.f.reg_types.get(&handle.0).cloned() {
            if outer.referent.as_ref() == target {
                break;
            }
            let Ty::Ref(inner) = outer.referent.as_ref() else {
                break;
            };
            let dest = self.fresh_typed(site.clone(), None, Ty::Ref(inner.clone()));
            self.emit(MirInstr::ReadRef {
                dest,
                reference: handle,
            });
            handle = dest;
        }
        handle
    }

    fn reference_handle(&mut self, expression: &Expr) -> Reg {
        if let Some(reference) = self.reference_result(expression) {
            if let Some(place) = self.lower_projected_reference_place(expression) {
                let dest = self.fresh_typed(
                    expression.source_span(),
                    Some(place.root),
                    Ty::Ref(reference),
                );
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
            let mut result = self.expr_unconverted(expression);
            self.f
                .reg_types
                .insert(result.0, Ty::Ref(reference.clone()));
            // A reference-returning subscript over reference-valued storage has
            // an outer handle to the container slot and an inner handle stored
            // in that slot. Peel only the outer layers that expression typing
            // read through. The returned register remains a handle whose single
            // read produces the checker's ordinary expression type.
            if let Some(checked) = self.checked_ty(expression) {
                result = self.peel_reference_handle_to(result, &checked, expression.source_span());
            }
            return result;
        }
        if let ExprKind::Identifier(name) = &expression.kind {
            let var = self.expression_var(name, expression);
            if let Some(loan) = self.aliases.get(&var).cloned() {
                let handle_ty = self
                    .var_types
                    .get(&var)
                    .filter(|ty| matches!(ty, Ty::Ref(_)))
                    .cloned()
                    .or_else(|| {
                        mir_place_handle_ty(
                            &loan.place,
                            Some(if loan.mutable {
                                crate::origin::Mutability::Mutable
                            } else {
                                crate::origin::Mutability::Immutable
                            }),
                        )
                    })
                    .unwrap_or(Ty::Error);
                let dest =
                    self.fresh_typed(expression.source_span(), Some(loan.place.root), handle_ty);
                self.emit(MirInstr::MakeRef {
                    dest,
                    place: loan.place,
                });
                return dest;
            }
            if self.runtime_aliases.contains(&var) {
                let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                place.through = Some(var);
                let handle_ty = mir_place_handle_ty(&place, None).unwrap_or(Ty::Error);
                let dest = self.fresh_typed(expression.source_span(), Some(var), handle_ty);
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
        }
        if matches!(expression.kind, ExprKind::TypeApply { .. })
            && self
                .checked_adjustments(expression)
                .iter()
                .any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::VariantProject { .. })
                })
        {
            let place = self.place(expression);
            let handle_ty = mir_place_handle_ty(
                &place,
                self.checked_borrow_mutability(expression).map(|mutable| {
                    if mutable {
                        crate::origin::Mutability::Mutable
                    } else {
                        crate::origin::Mutability::Immutable
                    }
                }),
            )
            .unwrap_or(Ty::Error);
            let dest = self.fresh_typed(expression.source_span(), Some(place.root), handle_ty);
            self.emit(MirInstr::MakeRef { dest, place });
            return dest;
        }
        if matches!(
            expression.kind,
            ExprKind::Member { .. } | ExprKind::Index { .. }
        ) {
            // A reference-producing projection may begin with a nominal
            // accessor call (`return self.entries[i].value`). Forward through
            // that selected call's materialized handle just as a `ref` binding
            // or `mut`/`ref` actual does; rebuilding the syntax as a raw List or
            // Dict place would bypass the checked accessor contract.
            if let Some(place) = self.lower_projected_reference_place(expression) {
                let handle_ty = mir_place_handle_ty(&place, None).unwrap_or(Ty::Error);
                let dest = self.fresh_typed(expression.source_span(), Some(place.root), handle_ty);
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
            let place = self.place(expression);
            let handle_ty = mir_place_handle_ty(
                &place,
                self.checked_borrow_mutability(expression).map(|mutable| {
                    if mutable {
                        crate::origin::Mutability::Mutable
                    } else {
                        crate::origin::Mutability::Immutable
                    }
                }),
            )
            .unwrap_or(Ty::Error);
            let stored_ty = place.ty.clone().unwrap_or(Ty::Error);
            let storage = self.fresh_typed(expression.source_span(), Some(place.root), handle_ty);
            self.emit_interior_invalidations(expression, None);
            self.emit(MirInstr::MakeRef {
                dest: storage,
                place,
            });
            let dest = self.fresh_typed(expression.source_span(), None, stored_ty);
            self.emit(MirInstr::ReadRef {
                dest,
                reference: storage,
            });
            return dest;
        }
        // A reference-valued field load produces the stored frame/slot handle.
        self.expr_unconverted(expression)
    }

    fn fresh(&mut self, span: SourceSpan, origin: Option<VarId>) -> Reg {
        let r = self.next_reg;
        self.next_reg += 1;
        self.f.spans.0.insert(r, (span.without_syntax(), origin));
        Reg(r)
    }

    /// [`Self::fresh`] for a synthetic register with no checked expression
    /// node: the caller supplies the register's type directly. Loan and
    /// consumption markers, which never hold a runtime value, use `Ty::None`.
    fn fresh_typed(&mut self, span: SourceSpan, origin: Option<VarId>, ty: Ty) -> Reg {
        let r = self.fresh(span, origin);
        self.f.reg_types.insert(r.0, ty);
        r
    }

    fn emit(&mut self, i: MirInstr) {
        self.f.blocks[self.cur].instrs.push(i);
    }

    /// The callee for a free call the checker recorded no exact target for: the
    /// plain name when it isn't overloaded (the common case), else a poison
    /// marker — an overloaded call off the checked path must not guess.
    fn overloaded_name(&self, name: &str, argc: usize) -> String {
        if self.overloads.function_is_overloaded(name, argc) {
            crate::symbol::unresolved_overload_marker(name, argc)
        } else {
            name.to_string()
        }
    }

    /// Intern a variable name to a stable `VarId` (matches `hir::Lower::var`).
    fn var(&mut self, name: &str) -> VarId {
        if let Some(i) = self.vars.iter().position(|n| n == name) {
            i as VarId
        } else {
            self.vars.push(name.to_string());
            (self.vars.len() - 1) as VarId
        }
    }

    /// Intern the runtime slot for one checked binding. Same-spelled lexical
    /// declarations deliberately receive different slots.
    fn binding_var(&mut self, binding: crate::origin::OwnerId, name: &str) -> VarId {
        if let Some(var) = self.owner_vars.get(&binding).copied() {
            return var;
        }
        // Function parameters and HIR-declared slots are already interned. An
        // owner first encountered through an explicit-but-unused capture still
        // needs to attach to that existing slot. Opaque binder syntax such as
        // tuple unpacking may retain the source spelling, however, so an
        // already-claimed candidate belongs to a different stable binding and
        // must receive a distinct slot.
        let candidate = self.var(name);
        let var = if self
            .owner_vars
            .iter()
            .any(|(owner, var)| *owner != binding && *var == candidate)
        {
            let runtime_name = format!("{name}$binding{}", binding.0);
            self.var(&runtime_name)
        } else {
            candidate
        };
        self.owner_vars.insert(binding, var);
        var
    }

    fn declare_binding_var(&mut self, binding: crate::origin::OwnerId, name: &str) -> VarId {
        if let Some(var) = self.owner_vars.get(&binding).copied() {
            return var;
        }
        let runtime_name = if self.vars.iter().any(|candidate| candidate == name) {
            format!("{name}$binding{}", binding.0)
        } else {
            name.to_string()
        };
        let var = self.var(&runtime_name);
        self.owner_vars.insert(binding, var);
        var
    }

    fn binding_place(&mut self, binding: crate::origin::OwnerId, name: &str) -> MirPlace {
        let var = self.binding_var(binding, name);
        self.aliases
            .get(&var)
            .map(|loan| {
                let mut place = loan.place.clone();
                place.through = Some(var);
                place
            })
            .unwrap_or_else(|| MirPlace::root(var, self.var_types.get(&var).cloned()))
    }

    fn resolved_place(&mut self, name: &str) -> MirPlace {
        let var = self.var(name);
        self.aliases
            .get(&var)
            .map(|loan| {
                let mut place = loan.place.clone();
                place.through = Some(var);
                place
            })
            .unwrap_or_else(|| {
                let ty = self.var_types.get(&var).cloned();
                MirPlace::root(var, ty)
            })
    }

    fn expression_place_root(&mut self, name: &str, expression: &Expr) -> MirPlace {
        let checked_var = self
            .checked_owner(expression)
            .map(|owner| self.binding_var(owner, name));
        let mut place = if let Some(var) = checked_var {
            self.aliases
                .get(&var)
                .map(|loan| {
                    let mut place = loan.place.clone();
                    place.through = Some(var);
                    place
                })
                .unwrap_or_else(|| MirPlace::root(var, self.var_types.get(&var).cloned()))
        } else {
            self.resolved_place(name)
        };
        if place.root_ty.is_none() {
            let ty = self
                .checked_place_ty(expression)
                .or_else(|| self.checked_ty(expression));
            place.root_ty = ty.clone();
            place.ty = ty.clone();
            if let Some(ty) = ty {
                self.var_types.insert(place.root, ty);
            }
        }
        place
    }

    /// Flatten one or more argument expressions to their result registers.
    fn args(&mut self, args: &[Expr]) -> Vec<Reg> {
        args.iter().map(|a| self.expr(a)).collect()
    }

    fn param_arg_reg(&mut self, argument: &ParamArg) -> Option<Reg> {
        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => match &**value {
                ParamArg::Value(expression) => expression,
                ParamArg::Type(_) => return None,
                ParamArg::Named { .. } => unreachable!(),
            },
            ParamArg::Type(_) => return None,
        };
        if self
            .checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::EraseCompileTimeArgument
                )
            })
        {
            None
        } else {
            Some(self.expr(expression))
        }
    }

    /// Semantic Origin/OriginSet arguments are retained in source long enough
    /// for the checker to solve reference and capture contracts, but they have
    /// no slot in `ParamDecl`.  Drop those arguments from the runtime parameter
    /// vector instead of emitting an ambiguous `None` that would shift every
    /// following callable/scalar value parameter.
    fn param_arg_is_erased(&self, argument: &ParamArg) -> bool {
        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => match &**value {
                ParamArg::Value(expression) => expression,
                ParamArg::Type(_) => return false,
                ParamArg::Named { .. } => unreachable!(),
            },
            ParamArg::Type(_) => return false,
        };
        self.checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::EraseCompileTimeArgument
                )
            })
    }

    fn param_arg_regs(&mut self, arguments: &[ParamArg]) -> Vec<MirParamArg> {
        let mut registers = Vec::new();
        for argument in arguments {
            if !self.param_arg_is_erased(argument) {
                let name = match argument {
                    ParamArg::Named { name, .. } => Some(name.clone()),
                    ParamArg::Type(_) | ParamArg::Value(_) => None,
                };
                registers.push(MirParamArg {
                    name,
                    value: self.param_arg_reg(argument),
                });
            }
        }
        registers
    }

    /// Intern a fresh synthetic variable (a `$`-prefixed name never produced by
    /// the parser), used to carry a short-circuit result across CFG blocks.
    fn fresh_var(&mut self) -> VarId {
        let id = self.vars.len();
        self.vars.push(format!("$sc{id}"));
        id as VarId
    }

    /// Append a new empty basic block (placeholder terminator) and return its id.
    fn new_block(&mut self) -> MirBlockId {
        self.f.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Return(None),
        });
        self.f.blocks.len() - 1
    }

    /// Lower `a and b` / `a or b` into control flow so the right operand is only
    /// evaluated when needed (Python/Mojo short-circuit semantics). The result is
    /// carried in a synthetic variable across the branch and read back in the
    /// merge block. (Preserving the short-circuit — vs an eager `BinOp` — matters
    /// both for observable side effects and for Stage 6 ownership, where a moved
    /// operand on the not-taken side must not count as moved.)
    fn short_circuit(&mut self, op: InfixOp, a: &Expr, b: &Expr, span: SourceSpan) -> Reg {
        let ra = self.expr(a);
        let result = self.fresh_var();
        // Seed the result with the left operand's value: for `and` a false `ra`
        // is the answer; for `or` a true `ra` is. The rhs block overwrites it.
        self.emit(MirInstr::DefVar {
            var: result,
            src: ra,
            binding_ty: None,
        });

        let rhs_blk = self.new_block();
        let merge_blk = self.new_block();
        // `and`: evaluate rhs only when `ra` is true; `or`: only when false.
        let (then_b, else_b) = match op {
            InfixOp::And => (rhs_blk, merge_blk),
            _ => (merge_blk, rhs_blk),
        };
        self.f.blocks[self.cur].term = MirTerm::Branch {
            cond: ra,
            then_b,
            else_b,
        };

        self.cur = rhs_blk;
        let rb = self.expr(b); // may itself split blocks (nested and/or)
        self.emit(MirInstr::DefVar {
            var: result,
            src: rb,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);

        self.cur = merge_blk;
        let d = self.fresh(span, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Lower a ternary `then_e if cond else else_e` to a value: branch on `cond`,
    /// each arm writing the result variable, then read it at the merge.
    fn ternary(&mut self, cond: &Expr, then_e: &Expr, else_e: &Expr, sp: SourceSpan) -> Reg {
        let rc = self.expr(cond);
        let result = self.fresh_var();
        let then_blk = self.new_block();
        let else_blk = self.new_block();
        let merge_blk = self.new_block();
        self.f.blocks[self.cur].term = MirTerm::Branch {
            cond: rc,
            then_b: then_blk,
            else_b: else_blk,
        };
        self.cur = then_blk;
        let rt = self.expr(then_e);
        self.emit(MirInstr::DefVar {
            var: result,
            src: rt,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
        self.cur = else_blk;
        let re = self.expr(else_e);
        self.emit(MirInstr::DefVar {
            var: result,
            src: re,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
        self.cur = merge_blk;
        let d = self.fresh(sp, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Lower a chained comparison `a op1 b op2 c …` to a `Bool`. Each operand is
    /// evaluated **once**, left to right; a false link short-circuits the rest (the
    /// remaining operands are not evaluated). The result variable holds the last
    /// comparison evaluated (which is `false` on the link that failed).
    fn compare_chain(&mut self, first: &Expr, rest: &[(InfixOp, Expr)], sp: SourceSpan) -> Reg {
        let result = self.fresh_var();
        let merge_blk = self.new_block();
        let mut prev = self.expr(first);
        for (i, (op, operand)) in rest.iter().enumerate() {
            let cur = self.expr(operand);
            let cmp = self.fresh(sp.clone(), None);
            self.emit(MirInstr::BinOp {
                op: *op,
                dest: cmp,
                a: prev,
                b: cur,
                resolved: None,
            });
            self.emit(MirInstr::DefVar {
                var: result,
                src: cmp,
                binding_ty: None,
            });
            if i + 1 == rest.len() {
                self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
            } else {
                // A false link is the answer (result is already it); a true link
                // continues to the next comparison.
                let next_blk = self.new_block();
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: cmp,
                    then_b: next_blk,
                    else_b: merge_blk,
                };
                self.cur = next_blk;
                prev = cur;
            }
        }
        self.cur = merge_blk;
        let d = self.fresh(sp, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Return the collection constructor and insertion protocol selected by the
    /// checker.  Collection syntax is deliberately absent from this decision:
    /// lowering consumes the nominal plan just like any other resolved call.
    fn collection_plan(&self, expression: &Expr) -> Option<(Ty, Option<String>)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ConstructCollection { target, insert } => {
                    Some((target, insert))
                }
                _ => None,
            })
    }

    /// Construct an empty nominal collection and bind it to a synthetic slot so
    /// each checked mutating insertion can use the ordinary method-call ABI.
    fn begin_nominal_collection(&mut self, expression: &Expr, target: &Ty) -> VarId {
        let Ty::Struct(name, _) = target else {
            unreachable!("checked collection target is nominal")
        };
        let empty = self.fresh_typed(expression.source_span(), None, target.clone());
        self.emit(MirInstr::Call {
            dest: empty,
            func: FuncRef::named(name),
            raises: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
        });
        let collection = self.fresh_var();
        self.var_types.insert(collection, target.clone());
        self.emit(MirInstr::DefVar {
            var: collection,
            src: empty,
            binding_ty: Some(target.clone()),
        });
        collection
    }

    /// Execute one checked append/add/setitem operation on a synthetic nominal
    /// collection slot.  Borrowing the receiver avoids invoking its copy
    /// constructor; a `mut self` implementation commits through `recv_place`.
    fn insert_nominal_collection(
        &mut self,
        expression: &Expr,
        collection: VarId,
        target: &Ty,
        resolved: &str,
        args: Vec<Reg>,
    ) {
        let method = resolved
            .rsplit_once('.')
            .map_or(resolved, |(_, method)| method)
            .to_string();
        let recv = self.fresh_typed(expression.source_span(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: recv,
            var: collection,
            mode: UseMode::BorrowMut,
        });
        let dest = self.fresh_typed(expression.source_span(), None, Ty::None);
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved: Some(resolved.to_string()),
            raises: None,
            args: args.clone(),
            kwargs: Vec::new(),
            recv_place: Some(MirPlace::root(collection, Some(target.clone()))),
            arg_places: vec![None; args.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
        });
    }

    fn finish_nominal_collection(
        &mut self,
        expression: &Expr,
        collection: VarId,
        target: &Ty,
    ) -> Reg {
        let result = self.fresh_typed(expression.source_span(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: result,
            var: collection,
            mode: UseMode::Move,
        });
        result
    }

    /// Lower comprehension clauses directly into MIR control flow. This is the
    /// same left-to-right nesting as an explicit series of `for`/`if` blocks;
    /// the final leaf performs the collection family's insertion protocol.
    fn comprehension_clauses(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        bindings: &[crate::checked::CheckedComprehensionBinding],
        index: usize,
        plan: &ComprehensionPlan<'_>,
    ) {
        if index == clauses.len() {
            // Dictionary evaluation is key-before-value, matching an ordinary
            // display and indexed assignment. List/set leaves evaluate one item.
            let key = plan.key.map(|expression| self.expr(expression));
            let value_reg = self.expr(plan.value);
            let mut arguments = Vec::with_capacity(1 + usize::from(key.is_some()));
            if let Some(key) = key {
                arguments.push(key);
            }
            arguments.push(value_reg);
            self.insert_nominal_collection(
                plan.value,
                plan.collection,
                plan.target,
                plan.insert,
                arguments,
            );
            return;
        }

        match &clauses[index] {
            crate::ast::ComprehensionClause::If(condition) => {
                let condition = self.expr(condition);
                let body = self.new_block();
                let continuation = self.new_block();
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: condition,
                    then_b: body,
                    else_b: continuation,
                };
                self.cur = body;
                self.comprehension_clauses(clauses, bindings, index + 1, plan);
                self.f.blocks[self.cur].term = MirTerm::Jump(continuation);
                self.cur = continuation;
            }
            crate::ast::ComprehensionClause::For {
                var, owned, iter, ..
            } => {
                let iterator_name = format!("$compiter{}", self.vars.len());
                let iterator = self.var(&iterator_name);
                let iterator_ty = self.checked_ty(iter);
                if let Some(ty) = iterator_ty.clone() {
                    self.var_types.insert(iterator, ty);
                }
                let protocol = self
                    .checked_adjustments(iter)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::Iterate(protocol) => Some(protocol),
                        _ => None,
                    })
                    .unwrap_or(crate::IterationProtocol {
                        mode: if *owned {
                            crate::IterationMode::Owned
                        } else {
                            crate::IterationMode::Borrowed
                        },
                        borrowed_origin: None,
                        reference: None,
                        prepare: Vec::new(),
                        has_next: None,
                        next: None,
                        exhaustion: None,
                    });
                if let Some(origin) = &protocol.borrowed_origin {
                    let place = self.place(iter);
                    let value_ty = iterator_ty
                        .clone()
                        .or_else(|| place.ty.clone())
                        .expect("checked borrowed comprehension iterable has a type");
                    let iterator_value =
                        self.fresh_typed(iter.source_span(), Some(place.root), value_ty);
                    self.emit(MirInstr::LoadPlace {
                        dest: iterator_value,
                        place: place.clone(),
                    });
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty.clone(),
                    });
                    let canonical = self
                        .mir_interior_origin(origin, Some(place.root))
                        .expect("checked borrowed comprehension origin has a MIR owner");
                    let loans = vec![MirLoan {
                        place,
                        mutable: false,
                        interior: Some(canonical),
                    }];
                    let marker =
                        self.fresh_typed(iter.source_span(), Some(loans[0].place.root), Ty::None);
                    self.emit(MirInstr::EstablishLoans {
                        reference: iterator,
                        loans: loans.clone(),
                        marker,
                    });
                    self.aggregate_loans.insert(iterator, loans);
                } else {
                    let iterator_value = self.expr(iter);
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty.clone(),
                    });
                }
                self.emit(MirInstr::GetIter {
                    iter: iterator,
                    mode: protocol.mode,
                    prepare: protocol.prepare.clone(),
                });

                let header = self.new_block();
                let body = self.new_block();
                let exit = self.new_block();
                let binding_index = clauses[..index]
                    .iter()
                    .filter(|clause| matches!(clause, crate::ast::ComprehensionClause::For { .. }))
                    .count();
                let binding = bindings
                    .get(binding_index)
                    .expect("checked comprehension binder metadata");
                let element_value = self.fresh(iter.source_span(), Some(iterator));
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = header;
                let has_next = self.fresh(iter.source_span(), Some(iterator));
                if let Some(exhaustion) = protocol.exhaustion.clone() {
                    self.emit(MirInstr::TryNext {
                        dest: element_value,
                        yielded: has_next,
                        iter: iterator,
                        method: protocol
                            .next
                            .clone()
                            .expect("raising iterator has checked __next__ symbol"),
                        exhaustion,
                    });
                } else {
                    self.emit(MirInstr::HasNext {
                        dest: has_next,
                        iter: iterator,
                        method: protocol.has_next.clone(),
                    });
                }
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: has_next,
                    then_b: body,
                    else_b: exit,
                };

                self.cur = body;
                if protocol.exhaustion.is_none() {
                    self.emit(MirInstr::Next {
                        dest: element_value,
                        iter: iterator,
                        method: protocol.next.clone(),
                    });
                }
                let binding_var = self.var(&format!("$comp{}${}", var, binding.owner.0));
                self.owner_vars.insert(binding.owner, binding_var);
                let element_ty = Some(binding.ty.clone());
                if let Some(ty) = element_ty.clone() {
                    self.var_types.insert(binding_var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var: binding_var,
                    src: element_value,
                    binding_ty: element_ty,
                });
                self.comprehension_clauses(clauses, bindings, index + 1, plan);
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = exit;
            }
        }
    }

    fn comprehension(
        &mut self,
        expression: &Expr,
        _kind: crate::ast::CollectionKind,
        key: Option<&Expr>,
        value: &Expr,
        clauses: &[crate::ast::ComprehensionClause],
    ) -> Reg {
        let (target, insert) = self
            .collection_plan(expression)
            .expect("checked collection comprehension has a nominal construction plan");
        let insert = insert.expect("list/set/dict comprehension has an insertion method");
        let collection = self.begin_nominal_collection(expression, &target);
        let bindings = self.comprehension_bindings(expression);
        let plan = ComprehensionPlan {
            collection,
            target: &target,
            insert: &insert,
            key,
            value,
        };
        self.comprehension_clauses(clauses, &bindings, 0, &plan);
        self.finish_nominal_collection(expression, collection, &target)
    }

    /// Post-order: each subexpression emits one instruction and yields its result
    /// `Reg`, so `foo(bar(x))` → `t0 = bar(x); t1 = foo(t0)`. Total over `Expr`.
    fn expr_hir(&mut self, expression: &crate::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.expr(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    fn reference_handle_hir(&mut self, expression: &crate::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.reference_handle(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    fn projected_reference_place_hir(
        &mut self,
        expression: &crate::hir::HirExpr,
    ) -> Option<MirPlace> {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.lower_projected_reference_place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    fn place_hir(&mut self, expression: &crate::hir::HirExpr) -> MirPlace {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    fn expr(&mut self, e: &Expr) -> Reg {
        let result = self.expr_with_adjustments(e);
        // An emit-site type (a conversion result, a closure value) is more
        // precise than the source expression's pre-adjustment checked type.
        if let Some(ty) = self.checked_ty(e) {
            self.f.reg_types.entry(result.0).or_insert(ty);
        }
        result
    }

    fn expr_with_adjustments(&mut self, e: &Expr) -> Reg {
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                crate::SemanticAdjustment::BorrowShared | crate::SemanticAdjustment::BorrowMutable
            )
        }) {
            return self.reference_handle(e);
        }
        if let Some(reference) = self.reference_result(e) {
            let handle = self.reference_handle(e);
            let value_ty = match self.f.reg_types.get(&handle.0) {
                Some(Ty::Ref(reference)) => (*reference.referent).clone(),
                _ => (*reference.referent).clone(),
            };
            let read = self.fresh_typed(span(e), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            // A reference-returning expression has two checked uses. `ref x =
            // expression` is intercepted by the Borrow adjustment above and
            // retains its handle. Every ordinary value use reads the referent
            // into independently owned storage, so lifecycle types must run
            // their copy initializer rather than alias backing storage.
            let dest = self.fresh_typed(span(e), None, value_ty);
            self.emit(MirInstr::CopyValue { dest, value: read });
            return dest;
        }
        if let Some(target) = self.index_normalization(e) {
            // The source Indexer is evaluated exactly once. The checked target
            // may be concrete or an abstract trait-dispatch symbol; MethodCall
            // already retargets the latter from the runtime receiver while
            // preserving the selected signature.
            let recv = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(recv.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, Ty::Int);
            self.emit(MirInstr::MethodCall {
                dest,
                recv,
                method: "__mlir_index__".to_string(),
                resolved: Some(target),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                recv_place: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            });
            return dest;
        }
        if let Some(target) = self.implicit_conversion(e) {
            let argument = self.expr_unconverted(e);
            // The conversion result is the constructed type, not the source
            // expression's checked type; targets are concrete constructors.
            let dest = match target.split(".__init__").next() {
                Some(constructed) if !constructed.is_empty() => self.fresh_typed(
                    span(e),
                    None,
                    Ty::Struct(constructed.to_string(), Vec::new()),
                ),
                _ => self.fresh(span(e), None),
            };
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises: None,
                args: vec![argument],
                kwargs: Vec::new(),
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
            });
            return dest;
        }
        if let Some(target) = self.literal_materialization(e) {
            let value = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(value.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, target.clone());
            self.emit(MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            });
            return dest;
        }
        self.expr_unconverted(e)
    }

    fn reference_result(&self, expression: &Expr) -> Option<crate::origin::RefTy> {
        // The selected-call contract is the canonical checked handoff.  In
        // particular, a free-function reference result may share its source
        // expression with another compatibility adjustment, so consulting only
        // the legacy single-operation slot can silently lose `ref[a, b] T` and
        // type a runtime handle local as ordinary `T` storage.
        self.checked_call_contract(expression)
            .and_then(|contract| contract.reference_result)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::ReferenceResult { reference } => Some(reference),
                        _ => None,
                    })
            })
    }

    fn expr_unconverted(&mut self, e: &Expr) -> Reg {
        match &e.kind {
            // --- Literals ------------------------------------------------------
            ExprKind::Int(n) => self.constant(e, Const::IntLiteral(n.clone())),
            ExprKind::Float(x) => self.constant(e, Const::FloatLiteral(x.clone())),
            ExprKind::Bool(b) => self.constant(e, Const::Bool(*b)),
            ExprKind::Str(s) => self.constant(e, Const::Str(s.clone())),
            ExprKind::None => self.constant(e, Const::None),
            ExprKind::Uninitialized => self.constant(e, Const::None),
            ExprKind::Spread(_) => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(
                    "unexpanded call spread reached MIR lowering".to_string(),
                ));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }

            // --- Variable reads ------------------------------------------------
            // A bare read defaults to `Copy`; a call site refines it to
            // `Borrow*`/`Move` per the callee's convention (Stage 6).
            ExprKind::Identifier(name) => {
                if let Some(target) = self.resolved_callable(e) {
                    return self.constant(e, Const::Function(target));
                }
                if let Some(info) = self.nested_info(e) {
                    return self.load_nested_closure(name, &info, span(e));
                }
                if !self.vars.iter().any(|candidate| candidate == name)
                    && self.overloads.is_function(name)
                {
                    return self.constant(e, Const::Function(name.clone()));
                }
                let var = self.expression_var(name, e);
                let d = self.fresh(span(e), Some(var));
                if self.is_origin_bearing_pointer(e) {
                    // Reading a pointer variable produces its handle value;
                    // `UseVar` would read through the stored `Value::Ref` the
                    // way a `ref` binding does. `MakeRef` on the root forwards
                    // the existing handle unchanged.
                    self.emit(MirInstr::MakeRef {
                        dest: d,
                        place: MirPlace::root(var, self.var_types.get(&var).cloned()),
                    });
                    return d;
                }
                if let Some(loan) = self.aliases.get(&var).cloned() {
                    let mut place = loan.place;
                    place.through = Some(var);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                } else if self.runtime_aliases.contains(&var) {
                    let handle = self.fresh(e.source_span(), Some(var));
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place: {
                            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                            place.through = Some(var);
                            place
                        },
                    });
                    self.emit(MirInstr::ReadRef {
                        dest: d,
                        reference: handle,
                    });
                } else {
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Copy,
                    });
                }
                d
            }
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            ExprKind::Transfer(inner) => {
                if let ExprKind::Identifier(name) = &inner.kind {
                    let var = self.expression_var(name, inner);
                    let d = self.fresh(span(e), Some(var));
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Move,
                    });
                    d
                } else if let Some(place) = self.pure_field_place(inner) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else if let ExprKind::Index { object, .. } = &inner.kind
                    && matches!(self.checked_ty(object), Some(Ty::Tuple(_)))
                    && let Some(place) = self.try_place(inner)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else {
                    self.expr(inner)
                }
            }

            // --- Operators -----------------------------------------------------
            ExprKind::Prefix(op, a) => {
                let ra = self.expr(a);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::UnOp {
                    op: *op,
                    dest: d,
                    a: ra,
                });
                d
            }
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), a, b) => {
                self.short_circuit(*op, a, b, span(e))
            }
            // A checked nominal membership operation is an ordinary borrowed
            // `container.__contains__(value)` call.  Keeping it as a value-only
            // `BinOp` loses the receiver place; the VM would then install a
            // shallow struct value in the callee's `self` slot and destroy its
            // owned fields on return.  For pointer-backed collections that can
            // free the caller's storage.  Preserve source evaluation order
            // (value before container), the selected overload, and the normal
            // method-call place/capture contract.
            ExprKind::Infix(op @ (InfixOp::In | InfixOp::NotIn), value, container)
                if matches!(self.checked_ty(container), Some(Ty::Struct(..))) =>
            {
                let (argument, arg_place) = self.lower_call_argument(value);
                let (recv, recv_place) = self.lower_call_receiver(container);
                let contains = self.fresh_typed(span(e), None, Ty::Bool);
                self.emit_interior_invalidations(container, None);
                self.emit_call_invalidations(e, std::slice::from_ref(value), &[]);
                self.emit(MirInstr::MethodCall {
                    dest: contains,
                    recv,
                    method: "__contains__".to_string(),
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: vec![argument],
                    kwargs: Vec::new(),
                    recv_place,
                    arg_places: vec![arg_place],
                    kwarg_places: Vec::new(),
                    capture_accesses: self.checked_call_capture_accesses(e),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                });
                self.emit_nested_closure_argument_keepalives(std::slice::from_ref(value), &[]);
                if matches!(op, InfixOp::NotIn) {
                    let dest = self.fresh_typed(span(e), None, Ty::Bool);
                    self.emit(MirInstr::UnOp {
                        op: PrefixOp::Not,
                        dest,
                        a: contains,
                    });
                    dest
                } else {
                    contains
                }
            }
            ExprKind::Infix(op, a, b) => {
                let ra = self.expr(a); // operands left-to-right (evaluation order is explicit)
                let rb = self.expr(b);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::BinOp {
                    op: *op,
                    dest: d,
                    a: ra,
                    b: rb,
                    resolved: self.resolved_callable(e),
                });
                d
            }

            // --- Calls / access ------------------------------------------------
            // NOTE: keyword args + default-slot matching (`call::match_call_slots`)
            // are a follow-up; the checker has already validated them, so only the
            // positional `args` are flattened here.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                // A checked pointer construction materializes the frame/slot
                // handle for its source place; the checked pointer type keeps
                // the origin while the runtime value erases it.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::PointerToPlace { .. })
                }) {
                    let value = &kwargs
                        .first()
                        .expect("checked pointer construction has a 'to=' argument")
                        .value;
                    let place = self.place(value);
                    let dest = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MakeRef { dest, place });
                    return dest;
                }
                if let Some(crate::SemanticAdjustment::ConstructVariant {
                    alternatives,
                    index,
                }) = self.checked_adjustments(e).into_iter().find(|adjustment| {
                    matches!(
                        adjustment,
                        crate::SemanticAdjustment::ConstructVariant { .. }
                    )
                }) {
                    let value = self.expr(
                        args.first()
                            .expect("checked Variant construction has one payload"),
                    );
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeVariant {
                        dest,
                        alternatives,
                        index,
                        value,
                    });
                    return dest;
                }
                // SIMD construction resolves its `[DType.<dt>, width]` parameters
                // here (the MIR is otherwise untyped about them).
                if let Some(r) = self.try_simd_call(e, args) {
                    return r;
                }
                // A call to a nested `def` (a closure, called by name in scope):
                // rewrite to its lifted function, prepending the captured enclosing
                // locals as leading arguments (passed as places, so the `mut`
                // capture parameters write back — reference-capture semantics).
                if let Some(info) = self.nested_info(e) {
                    return self.lower_nested_call(e, &info, param_args, args, kwargs);
                }
                // A local with a function type (normally a callable parameter)
                // shadows any global function of the same name.
                if self.vars.iter().any(|candidate| candidate == name) {
                    let callee = self.expr(&Expr {
                        kind: ExprKind::Identifier(name.clone()),
                        span: e.span,
                        source: e.source.clone(),
                        syntax_id: crate::token::SyntaxId::fresh(),
                    });
                    let callable_ty = self
                        .vars
                        .iter()
                        .position(|candidate| candidate == name)
                        .and_then(|variable| self.var_types.get(&(variable as VarId)))
                        .cloned()
                        .or_else(|| self.f.reg_types.get(&callee.0).cloned());
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let param_decls = callable_ty
                        .as_ref()
                        .map(generic_callable_param_decls)
                        .unwrap_or_default();
                    let (regs, arg_places) = self.lower_call_arguments(args);
                    let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                    let place = self.resolved_place(name);
                    let callee_place = place.is_typed().then_some(place);
                    let dest = self.fresh(span(e), None);
                    self.emit_call_invalidations(e, args, kwargs);
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    let (instantiated_contract, instantiated_args) = self
                        .instantiated_callable_contract(e)
                        .map_or((None, Vec::new()), |(contract, arguments)| {
                            (Some(contract), arguments)
                        });
                    self.emit(MirInstr::CallIndirect {
                        dest,
                        callee,
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        callee_place,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        param_decls,
                        instantiated_contract,
                        instantiated_args,
                    });
                    return dest;
                }
                // `__RuntimeTuple` is the compiler-private heterogeneous pack
                // storage primitive. Public `Tuple` is an ordinary nominal
                // variadic struct and follows the call path below.
                if name == "__RuntimeTuple"
                    && kwargs.is_empty()
                    && !self.overloads.is_function(name)
                {
                    let regs = self.args(args);
                    let element_types = match self.checked_ty(e) {
                        Some(Ty::Tuple(elements)) => Some(elements),
                        _ => None,
                    };
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeTuple {
                        dest,
                        elems: regs,
                        element_types,
                    });
                    return dest;
                }
                // Compile-time parameter arguments (`Name[param_args](...)`),
                // evaluated before ordinary call arguments: a
                // **value** parameter is a comptime `Int` expression flattened to a
                // register; a **type** parameter is erased (`None`).
                let param_arg_regs = self.param_arg_regs(param_args);
                // Retain only checker-selected `mut`/`ref` caller places. A
                // syntactically simple copied argument remains eligible for
                // ASAP destruction after its value has been evaluated.
                let (regs, arg_places) = self.lower_call_arguments(args);
                let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                let target = self
                    .resolved_callable(e)
                    .unwrap_or_else(|| self.overloaded_name(name, args.len()));
                let d = self.fresh(span(e), None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(&target),
                    raises: self.checked_raises(e),
                    args: regs,
                    kwargs: kw,
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                d
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                if let Some(operation) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::VariantIs { .. }
                                | crate::SemanticAdjustment::VariantTypeSupported { .. }
                                | crate::SemanticAdjustment::VariantSet { .. }
                                | crate::SemanticAdjustment::VariantTake { .. }
                                | crate::SemanticAdjustment::VariantReplace { .. }
                        )
                    })
                {
                    let ExprKind::Member { object, .. } = &callee.kind else {
                        unreachable!("checked Variant operation has a member callee")
                    };
                    match operation {
                        crate::SemanticAdjustment::VariantIs { index, .. } => {
                            let variant = self.expr(object);
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::VariantIs {
                                dest,
                                variant,
                                index,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTypeSupported { supported } => {
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest,
                                k: Const::Bool(supported),
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantSet { index, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.set receiver is a writable place");
                            let value = self
                                .expr(args.first().expect("checked Variant.set has one payload"));
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantSet {
                                dest,
                                place,
                                index,
                                value,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTake { index, checked, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.take receiver is an owned place");
                            let variant = self.fresh(span(object), None);
                            self.emit(MirInstr::MovePlace {
                                dest: variant,
                                place,
                            });
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantTake {
                                dest,
                                variant,
                                index,
                                checked,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantReplace {
                            input_index,
                            output_index,
                            checked,
                            ..
                        } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.replace receiver is writable");
                            let value = self.expr(
                                args.first()
                                    .expect("checked Variant.replace has one payload"),
                            );
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantReplace {
                                dest,
                                place,
                                input_index,
                                output_index,
                                value,
                                checked,
                            });
                            return dest;
                        }
                        _ => unreachable!("filtered Variant operation"),
                    }
                }
                if let Some(param_decls) = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::ParameterizedMethodCall { param_decls } => {
                            Some(param_decls)
                        }
                        _ => None,
                    },
                ) {
                    let ExprKind::Member { object, field } = &callee.kind else {
                        unreachable!("checked parameterized method call has a member callee")
                    };
                    // Keep this as a direct method invocation. In particular,
                    // do not synthesize a bound-method value (which would make
                    // its receiver/environment escapable).
                    let (recv, recv_place) = self.lower_call_receiver(object);
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let (argument_regs, arg_places) = self.lower_call_arguments(args);
                    let (keyword_regs, kwarg_places) = self.lower_call_keywords(kwargs);
                    let dest = self.fresh(span(e), None);
                    let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                    self.emit_interior_invalidations(object, None);
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::MethodCall {
                        dest,
                        recv,
                        method: field.clone(),
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        args: argument_regs,
                        kwargs: keyword_regs,
                        recv_place: if implicitly_copied_receiver {
                            None
                        } else {
                            recv_place
                        },
                        arg_places,
                        kwarg_places,
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs,
                        param_decls,
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return dest;
                }
                let callee_place = self.callable_receiver_place(callee);
                let callable_ty = self.checked_ty(callee);
                let callee = self.expr(callee);
                let callable_ty = callable_ty.or_else(|| self.f.reg_types.get(&callee.0).cloned());
                let param_arg_regs = self.param_arg_regs(param_args);
                let param_decls = callable_ty
                    .as_ref()
                    .map(generic_callable_param_decls)
                    .unwrap_or_default();
                let (arg_regs, arg_places) = self.lower_call_arguments(args);
                let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs);
                let dest = self.fresh(span(e), None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                let (instantiated_contract, instantiated_args) = self
                    .instantiated_callable_contract(e)
                    .map_or((None, Vec::new()), |(contract, arguments)| {
                        (Some(contract), arguments)
                    });
                self.emit(MirInstr::CallIndirect {
                    dest,
                    callee,
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: arg_regs,
                    kwargs: kw_regs,
                    callee_place,
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs,
                    param_decls,
                    instantiated_contract,
                    instantiated_args,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                dest
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let pointer_storage = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::PointerStorageTake { element } => {
                            Some((true, element))
                        }
                        crate::SemanticAdjustment::PointerStorageDestroy { element } => {
                            Some((false, element))
                        }
                        _ => None,
                    },
                );
                if let Some((take, element)) = pointer_storage {
                    let pointer = self.expr(object);
                    let index = self.expr(
                        args.first()
                            .expect("checked pointer storage operation has one index"),
                    );
                    debug_assert!(kwargs.is_empty());
                    let dest = self.fresh(span(e), None);
                    self.emit(if take {
                        MirInstr::PointerStorageTake {
                            dest,
                            pointer,
                            index,
                            element,
                        }
                    } else {
                        MirInstr::PointerStorageDestroy {
                            dest,
                            pointer,
                            index,
                            element,
                        }
                    });
                    return dest;
                }
                let explicit_destroy = self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::ExplicitDestroy)
                });
                let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                if let ExprKind::Identifier(type_name) = &object.kind
                    && !self.vars.iter().any(|name| name == type_name)
                {
                    let (regs, arg_places) = self.lower_call_arguments(args);
                    let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                    let d = self.fresh(span(e), None);
                    let target = self
                        .resolved_callable(e)
                        .unwrap_or_else(|| format!("{type_name}.{method}"));
                    self.emit_call_invalidations(e, args, kwargs);
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&target),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // A **static** method on a parameterized built-in type — the receiver
                // is a type, not a value (`UnsafePointer[T].alloc(n)`). Lower to a
                // builtin call `Type.method(args)`; the element type is erased.
                if let ExprKind::TypeApply { name, .. } = &object.kind {
                    let regs = self.args(args);
                    let kw: Vec<(String, Reg)> = kwargs
                        .iter()
                        .map(|k| (k.name.clone(), self.expr(&k.value)))
                        .collect();
                    let d = self.fresh(span(e), None);
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&format!("{name}.{method}")),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places: vec![None; args.len()],
                        kwarg_places: vec![None; kwargs.len()],
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // If the receiver is a place, load it through that place (indices
                // evaluated once) and keep the place for write-back; otherwise it is
                // a temporary evaluated for its value only.
                let receiver_expr = if explicit_destroy {
                    match &object.kind {
                        ExprKind::Transfer(inner) => inner.as_ref(),
                        _ => object.as_ref(),
                    }
                } else {
                    object.as_ref()
                };
                let (recv, recv_place) = self.lower_call_receiver(receiver_expr);
                // Retain checker-selected `mut`/`ref` ordinary-argument places,
                // mirroring a free-function `Call`.
                let (regs, arg_places) = self.lower_call_arguments(args);
                let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(receiver_expr, None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                // An ordinary method call can still select a generic method and
                // infer all of its compile-time arguments from runtime actuals.
                // Preserve that declaration vocabulary even though there are no
                // explicit `method[...]` value arguments to lower.
                let param_decls = self
                    .checked_call_contract(e)
                    .map(|contract| contract.param_decls)
                    .unwrap_or_default();
                self.emit(MirInstr::MethodCall {
                    dest: d,
                    recv,
                    method: method.clone(),
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: regs,
                    kwargs: kw,
                    recv_place: if explicit_destroy || implicitly_copied_receiver {
                        None
                    } else {
                        recv_place
                    },
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs: Vec::new(),
                    param_decls,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                if explicit_destroy
                    && !implicitly_copied_receiver
                    && let Some(place) = self.try_place(receiver_expr)
                {
                    if place.proj.is_empty() {
                        self.emit(MirInstr::ConsumeVar { var: place.root });
                    } else {
                        self.emit(MirInstr::ConsumePlace {
                            place,
                            marker: recv,
                        });
                    }
                }
                d
            }
            ExprKind::Member { object, field } => {
                // A pure field chain rooted at a variable (`p.a`, `p.a.b`) lowers to
                // a `LoadPlace` (a place read) so the ownership analysis sees *which*
                // field is read — enabling field-sensitive partial-move checking
                // (reading `p.b` after `p.a^` stays legal). A member of a temporary
                // or an indexed base keeps the register-based `GetField`.
                let descriptor_field = matches!(
                    self.checked_ty(object),
                    Some(Ty::Struct(name, args))
                        if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                            && args.is_empty()
                );
                if !descriptor_field && let Some(place) = self.pure_field_place(e) {
                    let place_root = place.root;
                    let place_ty = place.ty.clone();
                    let loaded = self.fresh_typed(
                        span(e),
                        Some(place_root),
                        place_ty
                            .clone()
                            .or_else(|| self.checked_ty(e))
                            .unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::LoadPlace {
                        dest: loaded,
                        place,
                    });
                    // A field expression selected by the checker for a
                    // consuming value context owns its result just like a
                    // bare-variable `UseVar { Copy }`. Keep `LoadPlace` itself
                    // handle-preserving for method receivers, borrowed call
                    // arguments, iteration, and other explicit place
                    // operations; make only the checked value-copy boundary
                    // visible here so a nested lifecycle field runs its
                    // `__copyinit__` instead of merely duplicating an owning
                    // UnsafePointer.
                    //
                    // Reference-valued fields retain their existing handle/read
                    // path.  Their ordinary referent copies are selected by the
                    // checked `ReferenceResult` adjustment, not by this nominal
                    // field rule.
                    if !matches!(place_ty, Some(Ty::Ref(_)))
                        && self.checked_adjustments(e).iter().any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                        })
                    {
                        let copied = self.fresh_typed(
                            span(e),
                            Some(place_root),
                            self.checked_ty(e).unwrap_or(Ty::Error),
                        );
                        self.emit(MirInstr::CopyValue {
                            dest: copied,
                            value: loaded,
                        });
                        copied
                    } else {
                        loaded
                    }
                } else {
                    let base = if self.reference_result(object).is_some() {
                        self.lower_call_receiver(object).0
                    } else {
                        self.expr(object)
                    };
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::GetField {
                        dest: d,
                        base,
                        field: field.clone(),
                    });
                    d
                }
            }
            ExprKind::Index { object, index } => {
                // An indexed reference-bearing aggregate element is a storage
                // place whose checked type is `ref T`; load through the stored
                // handle exactly like a direct reference field.  Ordinary
                // indexing remains the register-based operation below. A
                // checker-selected nominal accessor must stay on that dispatch
                // path: projecting the nominal struct as raw indexed storage
                // would lose its concrete `__getitem__$N` target.
                if matches!(self.checked_place_ty(e), Some(Ty::Ref(_)))
                    && self.resolved_callable(e).is_none()
                    && !matches!(self.checked_ty(object), Some(Ty::Struct(..)))
                    && let Some(place) = self.try_place(e)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit_interior_invalidations(e, None);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                // Dereferencing an origin-bearing pointer reads its source
                // place; the checker fixed the offset to 0. A stably bound
                // pointer substitutes the owner place directly, keeping the
                // owner touched (and so droppable) at each access; otherwise
                // the access reads through the runtime handle.
                if let Some(place) = self.pointer_deref_place(object) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                if self.is_origin_bearing_pointer(object) {
                    let reference = self.expr(object);
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::ReadRef { dest: d, reference });
                    return d;
                }
                let has_call = self.checked_call_contract(e).is_some();
                let (base, base_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let (idx, index_place) = self.lower_call_argument(index);
                let call = self.subscript_call_contract(e, &[(index.source_span(), idx)]);
                let intrinsic = call
                    .is_none()
                    .then(|| self.intrinsic_index_dispatch(object))
                    .flatten();
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(index, None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Index {
                    dest: d,
                    base,
                    index: idx,
                    base_place,
                    index_place,
                    call,
                    intrinsic,
                });
                d
            }

            // --- Aggregates ----------------------------------------------------
            ExprKind::ListLit(elems) => {
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    for element in elems {
                        let value = self.expr(element);
                        self.insert_nominal_collection(
                            element,
                            collection,
                            &target,
                            &insert,
                            vec![value],
                        );
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // The unchecked CFG helper has no semantic facts. Keep it
                // syntax-total by emitting an ordinary constructor call; the
                // production checked path above always carries an exact target
                // and insertion method.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("List"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::BraceLit(entries) => {
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    let dictionary = dict_elements(&target).is_some();
                    for (key, value) in entries {
                        let key = self.expr(key);
                        let mut arguments = vec![key];
                        if dictionary {
                            arguments.push(
                                self.expr(
                                    value
                                        .as_ref()
                                        .expect("checked dictionary display has paired values"),
                                ),
                            );
                        }
                        self.insert_nominal_collection(e, collection, &target, &insert, arguments);
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // As above, this is only the syntax-only CFG compatibility
                // path. A verified program never guesses its collection kind.
                let dictionary = entries.first().is_none_or(|(_, value)| value.is_some());
                let d = self.fresh(span(e), None);
                let regs = if dictionary {
                    entries
                        .iter()
                        .flat_map(|(key, value)| {
                            let key = self.expr(key);
                            let value = value.as_ref().map(|value| self.expr(value));
                            std::iter::once(key).chain(value)
                        })
                        .collect::<Vec<_>>()
                } else {
                    entries
                        .iter()
                        .map(|(key, _)| self.expr(key))
                        .collect::<Vec<_>>()
                };
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(if dictionary { "Dict" } else { "Set" }),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => self.comprehension(e, *kind, key.as_deref(), value, clauses),
            ExprKind::TupleLit(elems) => {
                if let Some((target, None)) = self.collection_plan(e)
                    && let Ty::Struct(name, _) = &target
                {
                    let regs = self.args(elems);
                    let dest = self.fresh_typed(span(e), None, target.clone());
                    self.emit(MirInstr::Call {
                        dest,
                        func: FuncRef::named(name),
                        raises: None,
                        args: regs.clone(),
                        kwargs: Vec::new(),
                        arg_places: vec![None; regs.len()],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                    });
                    return dest;
                }
                // Syntax-only lowering cannot select a variadic specialization,
                // but it still emits an ordinary public constructor call. The
                // private `MakeTuple` opcode is reserved for `__RuntimeTuple`.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("Tuple"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }

            // Walrus `:=` reaches MIR after type checking. Preserve an explicit
            // unsupported boundary rather than assigning accidental semantics.
            ExprKind::Named { name, value } => {
                let value = self.expr(value);
                let var = self.var(name);
                self.emit(MirInstr::DefVar {
                    var,
                    src: value,
                    binding_ty: None,
                });
                value
            }
            // Ternary `a if cond else b` — a value-producing branch (like the
            // short-circuit lowering, but both arms assign the result).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => self.ternary(cond, then_branch, else_branch, span(e)),
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            ExprKind::Compare { first, rest } => self.compare_chain(first, rest, span(e)),
            // Slice `object[lower:upper:step]` → a new List/String.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                let has_call = self.checked_call_contract(e).is_some();
                let (obj, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let lower = lower.as_ref().map(|b| self.expr(b));
                let upper = upper.as_ref().map(|b| self.expr(b));
                let step = step.as_ref().map(|b| self.expr(b));
                let call = self.subscript_call_contract(e, &[]);
                let intrinsic = call
                    .is_none()
                    .then(|| self.intrinsic_slice_dispatch(object))
                    .flatten();
                let kind = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            descriptors.first().copied().flatten()
                        }
                        _ => None,
                    })
                    .expect("checked slice has a selected descriptor");
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Slice {
                    dest: d,
                    object: obj,
                    kind,
                    lower,
                    upper,
                    step,
                    object_place,
                    arg_places: vec![None],
                    call,
                    intrinsic,
                });
                d
            }
            ExprKind::MultiIndex { object, args } => {
                let has_call = self.checked_call_contract(e).is_some();
                let (object, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let descriptors = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            Some(descriptors)
                        }
                        _ => None,
                    })
                    .expect("checked multi-subscript has descriptor metadata");
                let mut arg_places = Vec::with_capacity(args.len());
                let mut parameter_sources = Vec::new();
                let lowered_args = args
                    .iter()
                    .zip(descriptors)
                    .map(|(argument, descriptor)| match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            debug_assert!(descriptor.is_none());
                            let (register, place) = self.lower_call_argument(value);
                            arg_places.push(place);
                            parameter_sources.push((value.source_span(), register));
                            MirSubscriptArg::Index(register)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            arg_places.push(None);
                            MirSubscriptArg::Slice {
                                kind: descriptor.expect("slice argument has descriptor kind"),
                                lower: lower.as_ref().map(|value| self.expr(value)),
                                upper: upper.as_ref().map(|value| self.expr(value)),
                                step: step.as_ref().map(|value| self.expr(value)),
                            }
                        }
                    })
                    .collect();
                let call = self.subscript_call_contract(e, &parameter_sources);
                let dest = self.fresh(span(e), None);
                for argument in args {
                    if let crate::ast::SubscriptArg::Index(argument) = argument {
                        self.emit_interior_invalidations(argument, None);
                    }
                }
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::MultiIndex {
                    dest,
                    object,
                    args: lowered_args,
                    object_place,
                    arg_places,
                    call,
                });
                dest
            }
            // These are flagged `Unsupported`/rejected by the *checker*, so a checked
            // program never reaches MIR lowering with them. A bare `TypeApply` is a
            // type used as a value (only valid as a static-method receiver, handled
            // in the `MethodCall` arm above).
            ExprKind::TString { parts, .. } => {
                let mut result = self.fresh(span(e), None);
                self.emit(MirInstr::Const {
                    dest: result,
                    k: Const::Str(String::new()),
                });
                for part in parts {
                    let piece = match part {
                        TStringPart::Literal(text) => {
                            let register = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest: register,
                                k: Const::Str(text.clone()),
                            });
                            register
                        }
                        TStringPart::Expr(value) => {
                            let argument = self.expr(value);
                            // Interpolation's implicit `String(value)` call has
                            // no source expression of its own.  Give the
                            // synthetic result its checked intrinsic type here
                            // instead of asking declaration-based MIR closure to
                            // rediscover the return type of the builtin.
                            let register = self.fresh_typed(span(value), None, Ty::String);
                            self.emit(MirInstr::Call {
                                dest: register,
                                func: FuncRef::named("String"),
                                raises: None,
                                args: vec![argument],
                                kwargs: Vec::new(),
                                arg_places: vec![None],
                                kwarg_places: Vec::new(),
                                capture_accesses: Vec::new(),
                                param_arg_regs: Vec::new(),
                            });
                            register
                        }
                    };
                    let joined = self.fresh(span(e), None);
                    self.emit(MirInstr::BinOp {
                        op: InfixOp::Add,
                        dest: joined,
                        a: result,
                        b: piece,
                        resolved: None,
                    });
                    result = joined;
                }
                result
            }
            ExprKind::TypeApply { name, .. }
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::VariantProject { .. })
                }) =>
            {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("checked Variant projection carries a tag");
                let mut place = self.resolved_place(name);
                if place.root_ty.is_none() {
                    place.root_ty = Some(Ty::Variant(
                        self.checked_adjustments(e)
                            .into_iter()
                            .find_map(|adjustment| match adjustment {
                                crate::SemanticAdjustment::VariantProject {
                                    alternatives, ..
                                } => Some(alternatives),
                                _ => None,
                            })
                            .unwrap_or_default(),
                    ));
                }
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                place.project(Proj::Variant(index), ty);
                let dest = self.fresh(span(e), Some(place.root));
                self.emit(MirInstr::LoadPlace { dest, place });
                dest
            }
            ExprKind::TypeApply { name, .. } if self.nested_info(e).is_some() => {
                let info = self
                    .nested_info(e)
                    .expect("guard established a checked nested declaration");
                let dest = self.load_nested_closure(name, &info, span(e));
                // The closure slot carries the declaration's generic callable
                // type; this expression carries the checker's concrete Origin
                // substitution and must win at the MIR value boundary.
                if let Some(specialized) = self.checked_ty(e) {
                    self.f.reg_types.insert(dest.0, specialized);
                }
                dest
            }
            ExprKind::TypeApply { .. } if self.resolved_callable(e).is_some() => self.constant(
                e,
                Const::Function(
                    self.resolved_callable(e)
                        .expect("checked callable TypeApply has a lowered target"),
                ),
            ),
            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. } => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked expression reached MIR lowering: {:?}",
                    e.kind
                )));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }
        }
    }

    /// If `name(...)` is a SIMD construction — `SIMD[DType.<dt>, width](elems)` or
    /// a scalar alias (`Int32(x)`, `Float32(x)`, …) — resolve its dtype/width and
    /// emit a [`MirInstr::MakeSimd`], returning its result register. Otherwise
    /// `None`, and the caller lowers it as an ordinary call.
    fn try_simd_call(&mut self, e: &Expr, args: &[Expr]) -> Option<Reg> {
        let (dtype, width) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ConstructSimd { dtype, width } => {
                    usize::try_from(width).ok().map(|width| (dtype, width))
                }
                _ => None,
            })?;
        let elems = self.args(args);
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::MakeSimd {
            dest: d,
            dtype,
            width,
            elems,
        });
        Some(d)
    }

    /// Lower a call to a nested `def` through the same closure-environment path as
    /// a first-class closure value. This preserves reference handles across sibling
    /// calls and recursion; it does not rely on call-return write-back.
    fn lower_nested_call(
        &mut self,
        e: &Expr,
        info: &NestedInfo,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Reg {
        let name = match &e.kind {
            ExprKind::Call { name, .. } => name.as_str(),
            _ => unreachable!("nested direct call has call syntax"),
        };
        let callee = self.load_nested_closure(name, info, span(e));
        let callable_ty = info
            .callable_ty
            .clone()
            .or_else(|| self.f.reg_types.get(&callee.0).cloned());
        let param_arg_regs = self.param_arg_regs(param_args);
        let param_decls = callable_ty
            .as_ref()
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let (arg_regs, arg_places) = self.lower_call_arguments(args);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs);
        let d = self.fresh(span(e), None);
        self.emit_call_invalidations(e, args, kwargs);
        let callee_place = self
            .owner_vars
            .contains_key(&info.binding)
            .then(|| self.binding_place(info.binding, name));
        let capture_accesses = self.checked_call_capture_accesses(e);
        let (instantiated_contract, instantiated_args) = self
            .instantiated_callable_contract(e)
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        self.emit(MirInstr::CallIndirect {
            dest: d,
            callee,
            // The checked owner already selects this exact lifted closure.
            // `resolved` is reserved for nominal/trait `__call__` dispatch;
            // attaching that abstract target here can disagree with an erased
            // variadic closure ABI even though execution never consults it.
            resolved: None,
            raises: self.checked_raises(e),
            args: arg_regs,
            kwargs: kw_regs,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        let mut owners = Vec::new();
        let mut seen = HashSet::new();
        for capture in &info.captures {
            self.collect_capture_keepalives(capture, &mut owners, &mut seen);
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
        d
    }

    fn collect_capture_keepalives(
        &self,
        capture: &NestedCapture,
        owners: &mut Vec<VarId>,
        seen: &mut HashSet<crate::origin::OwnerId>,
    ) {
        if capture.kind == crate::ast::CaptureKind::Move || !seen.insert(capture.binding) {
            return;
        }
        if let Some(var) = self.owner_vars.get(&capture.binding).copied()
            && !owners.contains(&var)
        {
            owners.push(var);
        }
        // A captured closure slot can itself retain reference captures. Keep
        // those owners alive transitively; owned copy/move environment entries
        // are self-contained and deliberately stop the walk.
        if let Some(callable) = self.nested.get(&capture.binding) {
            for nested in &callable.captures {
                if matches!(
                    nested.kind,
                    crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(nested, owners, seen);
                }
            }
        }
    }

    /// A capture-bearing nested callable passed to another non-escaping call
    /// can leave its environment handle in an SSA register. Keep the referenced
    /// owner storage alive through that consuming call without creating a
    /// persistent access loan (Mojo permits intervening owner mutation).
    fn emit_nested_closure_argument_keepalives(
        &mut self,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) {
        let mut owners = Vec::new();
        for expression in args
            .iter()
            .chain(kwargs.iter().map(|argument| &argument.value))
        {
            let expression = match &expression.kind {
                ExprKind::Named { value, .. } => value.as_ref(),
                _ => expression,
            };
            let ExprKind::Identifier(_) = &expression.kind else {
                continue;
            };
            let Some(info) = self.nested_info(expression) else {
                continue;
            };
            let mut seen = HashSet::new();
            let callable_capture = NestedCapture {
                name: info.source_name.clone(),
                binding: info.binding,
                ty: info.callable_ty.clone().unwrap_or(Ty::Param {
                    name: "$capture".to_string(),
                    bounds: Vec::new(),
                    callable_bound: None,
                }),
                kind: crate::ast::CaptureKind::Read,
            };
            self.collect_capture_keepalives(&callable_capture, &mut owners, &mut seen);
            for capture in info.captures {
                if matches!(
                    capture.kind,
                    crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(&capture, &mut owners, &mut seen);
                }
            }
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
    }

    fn emit_nested_closure(
        &mut self,
        info: &NestedInfo,
        at: SourceSpan,
        forward_existing_environment: bool,
    ) -> Reg {
        let captures = info
            .captures
            .iter()
            .map(|capture| MirClosureCapture {
                place: self.binding_place(capture.binding, &capture.name),
                mode: if forward_existing_environment {
                    // In a lifted body these names are already references into
                    // the declaration-created environment. Recursion and calls
                    // to inherited siblings forward those handles; they must
                    // never repeat a copy/move capture operation.
                    MirCaptureMode::Reference
                } else {
                    match capture.kind {
                        crate::ast::CaptureKind::Copy => MirCaptureMode::Copy,
                        crate::ast::CaptureKind::Move => MirCaptureMode::Move,
                        crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref => MirCaptureMode::Reference,
                    }
                },
            })
            .collect();
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at, None, ty.clone()),
            None => self.fresh(at, None),
        };
        self.emit(MirInstr::MakeClosure {
            dest,
            function: info.mangled.clone(),
            captures,
        });
        dest
    }

    fn load_nested_closure(&mut self, name: &str, info: &NestedInfo, at: SourceSpan) -> Reg {
        if !info.materialized_here && !self.owner_vars.contains_key(&info.binding) {
            // A lifted body has no direct access to an outer frame's closure slot.
            // Its inherited/self callable is reconstructed from the environment
            // parameters forwarded into this frame; direct declarations never use
            // this path after their statement has materialized them.
            return self.emit_nested_closure(info, at, true);
        }
        let var = self.binding_var(info.binding, name);
        if let Some(ty) = &info.callable_ty {
            self.var_types.entry(var).or_insert_with(|| ty.clone());
        }
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at.clone(), Some(var), ty.clone()),
            None => self.fresh(at.clone(), Some(var)),
        };
        if let Some(loan) = self.aliases.get(&var).cloned() {
            let mut place = loan.place;
            place.through = Some(var);
            self.emit(MirInstr::LoadPlace { dest, place });
        } else if self.runtime_aliases.contains(&var) {
            let handle = self.fresh(at, Some(var));
            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
            place.through = Some(var);
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place,
            });
            self.emit(MirInstr::ReadRef {
                dest,
                reference: handle,
            });
        } else {
            self.emit(MirInstr::UseVar {
                dest,
                var,
                // Calling a closure borrows its declaration-created environment;
                // neither loading it for a call nor a repeated call consumes or
                // duplicates that environment.
                mode: UseMode::BorrowShared,
            });
        }
        dest
    }

    /// Emit a `Const` writing a fresh register.
    fn constant(&mut self, e: &Expr, k: Const) -> Reg {
        let constant_ty = match &k {
            Const::Int(_) => Some(Ty::Int),
            Const::Float(_) => Some(Ty::Float64),
            Const::IntLiteral(_) => Some(Ty::IntLiteral),
            Const::FloatLiteral(_) => Some(Ty::FloatLiteral),
            Const::Bool(_) => Some(Ty::Bool),
            Const::Str(_) => Some(Ty::String),
            Const::None => Some(Ty::None),
            Const::Function(_) => self.checked_ty(e),
        };
        let d = match constant_ty {
            Some(ty) => self.fresh_typed(span(e), None, ty),
            None => self.fresh(span(e), None),
        };
        self.emit(MirInstr::Const { dest: d, k });
        d
    }

    fn materialize_register(&mut self, value: Reg, target: &Ty, source: SourceSpan) -> Reg {
        let Some(found) = self.f.reg_types.get(&value.0) else {
            return value;
        };
        let compatible = match (found, target) {
            (Ty::IntLiteral, Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }) => true,
            (Ty::FloatLiteral, Ty::Float64) => true,
            (Ty::FloatLiteral, Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            _ => false,
        };
        if !compatible {
            return value;
        }
        let dest = self.fresh_typed(source, None, target.clone());
        self.emit(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
        dest
    }

    // --- The driver's per-instruction / per-terminator lowering -----------------

    /// Lower one straight-line HIR instruction into `self.cur`. `outer_map` is the
    /// enclosing **function**'s HIR→MIR block map, used to resolve a `try`'s
    /// escape targets (`break`/`continue` to an outer loop); most arms ignore it.
    fn lower_instr(&mut self, i: &HirInstr, outer_map: &HashMap<hir::BlockId, MirBlockId>) {
        match i {
            HirInstr::Bind {
                dest,
                expr,
                binding_ty,
                binding,
            } => {
                let mut index = HashMap::new();
                index_hir_expression(&expr.syntax, expr, &mut index);
                self.active_semantics.push(index);
                let mut src = self.expr(&expr.syntax);
                if let Some(target) = binding_ty.as_ref() {
                    src = self.materialize_register(src, target, expr.source_span());
                }
                let writes_through_reference =
                    self.aliases.contains_key(dest) || self.runtime_aliases.contains(dest);
                if !writes_through_reference
                    && let Some(ty) = self
                        .f
                        .reg_types
                        .get(&src.0)
                        .cloned()
                        .or_else(|| expr.ty.clone())
                        .or_else(|| binding_ty.clone())
                {
                    self.var_types.insert(*dest, ty);
                }
                if let Some(binding) = binding {
                    self.owner_vars.insert(*binding, *dest);
                }
                // The initializer is evaluated before replacing the old
                // binding.  Whole-owner invalidation therefore sits here,
                // immediately before the write to the destination slot.
                self.emit_interior_invalidations(&expr.syntax, Some(*dest));
                if let Some(loan) = self.aliases.get(dest).cloned() {
                    let mut place = loan.place;
                    place.through = Some(*dest);
                    self.emit(MirInstr::Store { place, src });
                } else if self.runtime_aliases.contains(dest) {
                    let handle = self.fresh(expr.source_span(), Some(*dest));
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place: {
                            let mut place =
                                MirPlace::root(*dest, self.var_types.get(dest).cloned());
                            place.through = Some(*dest);
                            place
                        },
                    });
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: src,
                    });
                } else {
                    self.emit(MirInstr::DefVar {
                        var: *dest,
                        src,
                        binding_ty: binding_ty.clone(),
                    });
                    let aggregate_loans = self.aggregate_borrows(expr);
                    if let Some(first) = aggregate_loans.first() {
                        let marker =
                            self.fresh_typed(expr.source_span(), Some(first.place.root), Ty::None);
                        self.emit(MirInstr::EstablishLoans {
                            reference: *dest,
                            loans: aggregate_loans.clone(),
                            marker,
                        });
                    }
                    if aggregate_loans.is_empty() {
                        self.aggregate_loans.remove(dest);
                    } else {
                        self.aggregate_loans.insert(*dest, aggregate_loans);
                    }
                }
                self.active_semantics.pop();
            }
            HirInstr::BorrowIter { dest, expr, origin } => {
                let place = self.place_hir(expr);
                let value_ty = expr
                    .ty
                    .clone()
                    .or_else(|| place.ty.clone())
                    .expect("checked borrowed iterator place has a type");
                self.var_types.insert(*dest, value_ty.clone());
                let source = self.fresh_typed(expr.source_span(), Some(place.root), value_ty);
                // `LoadPlace` is a handle-preserving place read. In particular,
                // it does not run List.__copyinit__, so the iterator observes
                // element replacement in the original allocation.
                self.emit(MirInstr::LoadPlace {
                    dest: source,
                    place: place.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: source,
                    binding_ty: expr.ty.clone(),
                });
                let canonical = self
                    .mir_interior_origin(origin, Some(place.root))
                    .expect("checked borrowed iterator origin has a MIR owner");
                let loans = vec![MirLoan {
                    place,
                    mutable: false,
                    interior: Some(canonical),
                }];
                let marker =
                    self.fresh_typed(expr.source_span(), Some(loans[0].place.root), Ty::None);
                self.emit(MirInstr::EstablishLoans {
                    reference: *dest,
                    loans: loans.clone(),
                    marker,
                });
                self.aggregate_loans.insert(*dest, loans);
            }
            HirInstr::Eval(e) => {
                let _ = self.expr_hir(e); // evaluated for its effect; result discarded
            }
            HirInstr::Stmt(s) => self.lower_hir_stmt(s, outer_map),
            // A `try` whose enclosing loops are function-level: lower each sub-region
            // seeded with those loops (`loop_targets`, HIR function block ids), so an
            // outward `break`/`continue` becomes an `EscapeJump` resolved via
            // `outer_map`.
            HirInstr::Try { stmt, loop_targets } => {
                let mut index = HashMap::new();
                for (syntax, expression) in statement_expression_roots(&stmt.syntax)
                    .into_iter()
                    .zip(&stmt.expressions)
                {
                    index_hir_expression(syntax, expression, &mut index);
                }
                self.active_semantics.push(index);
                if let StmtKind::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } = &stmt.syntax.kind
                {
                    self.emit_try(
                        TryRegions {
                            body,
                            except,
                            orelse,
                            finalbody,
                            handler_binding: stmt.binding,
                        },
                        loop_targets,
                        outer_map,
                    );
                } else {
                    self.emit(MirInstr::Unsupported(
                        "malformed HIR try instruction".to_string(),
                    ));
                }
                self.active_semantics.pop();
            }
            HirInstr::Drop(var) => {
                self.emit(MirInstr::DropVar { var: *var });
            }
            // Iterator protocol: compute into a register, then store to the target
            // variable (so the header's branch can read `has_next` as a `UseVar`,
            // and the body binds the loop variable).
            HirInstr::GetIter { iter, protocol } => {
                self.emit(MirInstr::GetIter {
                    iter: *iter,
                    mode: protocol.mode,
                    prepare: protocol.prepare.clone(),
                });
            }
            HirInstr::HasNext { iter, dest, method } => {
                let r = self.fresh(SourceSpan::new(None, DUMMY_SPAN), None);
                self.emit(MirInstr::HasNext {
                    dest: r,
                    iter: *iter,
                    method: method.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: r,
                    binding_ty: None,
                });
            }
            HirInstr::Next {
                iter,
                dest,
                method,
                element_ty,
                binding,
            } => {
                if let Some(binding) = binding {
                    self.owner_vars.insert(*binding, *dest);
                }
                let r = self.fresh_typed(
                    SourceSpan::new(None, DUMMY_SPAN),
                    Some(*iter),
                    element_ty.clone(),
                );
                self.emit(MirInstr::Next {
                    dest: r,
                    iter: *iter,
                    method: method.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: r,
                    binding_ty: Some(element_ty.clone()),
                });
            }
            HirInstr::TryNext {
                iter,
                dest,
                yielded,
                method,
                exhaustion,
                element_ty,
                binding,
            } => {
                if let Some(binding) = binding {
                    self.owner_vars.insert(*binding, *dest);
                }
                let element = self.fresh(SourceSpan::new(None, DUMMY_SPAN), Some(*iter));
                let has_element = self.fresh(SourceSpan::new(None, DUMMY_SPAN), Some(*iter));
                self.emit(MirInstr::TryNext {
                    dest: element,
                    yielded: has_element,
                    iter: *iter,
                    method: method.clone(),
                    exhaustion: exhaustion.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: element,
                    binding_ty: Some(element_ty.clone()),
                });
                self.emit(MirInstr::DefVar {
                    var: *yielded,
                    src: has_element,
                    binding_ty: Some(Ty::Bool),
                });
            }
        }
    }

    /// Decompose a place expression (`x`, `p.a.b`, `xs[i]`, `p.items[i].x`) into a
    /// [`MirPlace`] — a root variable plus a projection chain — flattening any
    /// subscript index into a register **once**. The checker guarantees the root
    /// is a variable (or `self`), so a non-variable root is unreachable.
    fn place(&mut self, e: &Expr) -> MirPlace {
        match &e.kind {
            ExprKind::Identifier(name) => self.expression_place_root(name, e),
            ExprKind::Member { object, field } => {
                let mut p = self.place(object);
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                p
            }
            ExprKind::Index { object, index } => {
                let mut p = self.place(object);
                let idx = self.expr(index); // evaluated once, before the store
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Index(idx), ty);
                } else {
                    p.proj.push(Proj::Index(idx));
                }
                p
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("only checked Variant projection is a place TypeApply");
                let mut p = self.resolved_place(name);
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                p.project(Proj::Variant(index), ty);
                p
            }
            other => {
                self.emit(MirInstr::Unsupported(format!(
                    "invalid assignment place reached MIR lowering: {other:?}"
                )));
                MirPlace::root(self.var("$invalid_place"), None)
            }
        }
    }

    /// Lower `receiver[arguments] OP= rhs` from the complete checked accessor
    /// contracts. A value getter evaluates raw receiver/index sources, then the
    /// RHS, then getter-specific adaptations and the getter; the result is sent
    /// through independently adapted setter arguments. A mutable-reference
    /// getter instead establishes the lvalue before the RHS and finishes with a
    /// direct `WriteRef`, exactly as current Mojo does. In both paths each source
    /// expression and slice bound is evaluated once.
    fn lower_augmented_subscript(
        &mut self,
        target: &Expr,
        op: InfixOp,
        rhs_expression: &Expr,
    ) -> bool {
        let Some(plan) = self.checked_augmented_subscript(target) else {
            return false;
        };
        let (descriptors, value_keyword) = self
            .checked_adjustments(target)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    set_value_keyword,
                } => Some((descriptors, set_value_keyword)),
                _ => None,
            })
            .expect("checked augmented subscript has descriptor metadata");

        match &target.kind {
            ExprKind::Index { object, index } => {
                debug_assert_eq!(descriptors, vec![None]);
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let index_source = crate::checked::CheckedCallArgumentSource::Positional(0);
                let retain_index =
                    Self::checked_call_source_requires_place(&plan.getter, index_source)
                        || plan.setter.as_ref().is_some_and(|setter| {
                            Self::checked_call_source_requires_place(setter, index_source)
                        });
                let (raw_index, index_place) =
                    self.lower_augmented_argument_source(index, retain_index);

                if plan.setter.is_none() {
                    let getter_index = self.apply_checked_call_value_adjustments(
                        &plan.getter,
                        index_source,
                        raw_index,
                        index.source_span(),
                    );
                    let getter_call = self.mir_subscript_call_contract(
                        plan.getter.clone(),
                        &[(index.source_span(), getter_index)],
                    );
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::Index {
                        dest: handle,
                        base: receiver,
                        index: getter_index,
                        base_place: receiver_place,
                        index_place: Self::checked_call_source_place(
                            &plan.getter,
                            index_source,
                            &index_place,
                        ),
                        call: Some(getter_call),
                        intrinsic: None,
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                // Value-getter ordering is raw receiver/index, RHS, accessor
                // adaptation/getter, operator, setter adaptation/setter.
                let rhs = self.expr(rhs_expression);
                let getter_index = self.apply_checked_call_value_adjustments(
                    &plan.getter,
                    index_source,
                    raw_index,
                    index.source_span(),
                );
                let getter_call = self.mir_subscript_call_contract(
                    plan.getter.clone(),
                    &[(index.source_span(), getter_index)],
                );
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::Index {
                    dest: current,
                    base: receiver,
                    index: getter_index,
                    base_place: receiver_place.clone(),
                    index_place: Self::checked_call_source_place(
                        &plan.getter,
                        index_source,
                        &index_place,
                    ),
                    call: Some(getter_call),
                    intrinsic: None,
                });
                let result = self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                self.emit(MirInstr::BinOp {
                    op,
                    dest: result,
                    a: current,
                    b: rhs,
                    resolved: None,
                });

                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let setter_raw_index = self.reload_augmented_source(
                    raw_index,
                    &index_place,
                    Self::checked_call_source_mutates(&plan.getter, index_source),
                    index.source_span(),
                );
                let setter_index = self.apply_checked_call_value_adjustments(
                    setter,
                    index_source,
                    setter_raw_index,
                    index.source_span(),
                );
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(1)
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let setter_sources = [
                    (index.source_span(), setter_index),
                    (
                        plan.value_source
                            .clone()
                            .unwrap_or_else(|| target.source_span()),
                        value,
                    ),
                ];
                let setter_call = self.mir_subscript_call_contract(setter.clone(), &setter_sources);
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: vec![MirSubscriptArg::Index(setter_index)],
                    arg_places: vec![Self::checked_call_source_place(
                        setter,
                        index_source,
                        &index_place,
                    )],
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                let kind = descriptors
                    .first()
                    .copied()
                    .flatten()
                    .expect("augmented slice has a descriptor kind");
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let lower_reg = lower.as_ref().map(|bound| self.expr(bound));
                let upper_reg = upper.as_ref().map(|bound| self.expr(bound));
                let step_reg = step.as_ref().map(|bound| self.expr(bound));
                let getter_call = self.mir_subscript_call_contract(plan.getter.clone(), &[]);
                if plan.setter.is_none() {
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::Slice {
                        dest: handle,
                        object: receiver,
                        kind,
                        lower: lower_reg,
                        upper: upper_reg,
                        step: step_reg,
                        object_place: receiver_place,
                        arg_places: vec![None],
                        call: Some(getter_call),
                        intrinsic: None,
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                let rhs = self.expr(rhs_expression);
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::Slice {
                    dest: current,
                    object: receiver,
                    kind,
                    lower: lower_reg,
                    upper: upper_reg,
                    step: step_reg,
                    object_place: receiver_place.clone(),
                    arg_places: vec![None],
                    call: Some(getter_call),
                    intrinsic: None,
                });
                let result = self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                self.emit(MirInstr::BinOp {
                    op,
                    dest: result,
                    a: current,
                    b: rhs,
                    resolved: None,
                });
                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(1)
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let setter_call = self.mir_subscript_call_contract(
                    setter.clone(),
                    &[(
                        plan.value_source
                            .clone()
                            .unwrap_or_else(|| target.source_span()),
                        value,
                    )],
                );
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: vec![MirSubscriptArg::Slice {
                        kind,
                        lower: lower_reg,
                        upper: upper_reg,
                        step: step_reg,
                    }],
                    arg_places: vec![None],
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            ExprKind::MultiIndex {
                object,
                args: source,
            } => {
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let mut source_places = Vec::with_capacity(source.len());
                let mut source_spans = Vec::with_capacity(source.len());
                let raw_args = source
                    .iter()
                    .zip(&descriptors)
                    .enumerate()
                    .map(|(position, (argument, descriptor))| match argument {
                        crate::ast::SubscriptArg::Index(index) => {
                            debug_assert!(descriptor.is_none());
                            let argument_source =
                                crate::checked::CheckedCallArgumentSource::Positional(position);
                            let retain_place = Self::checked_call_source_requires_place(
                                &plan.getter,
                                argument_source,
                            ) || plan.setter.as_ref().is_some_and(|setter| {
                                Self::checked_call_source_requires_place(setter, argument_source)
                            });
                            let (register, place) =
                                self.lower_augmented_argument_source(index, retain_place);
                            source_places.push(place);
                            source_spans.push(Some(index.source_span()));
                            MirSubscriptArg::Index(register)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            source_places.push(None);
                            source_spans.push(None);
                            MirSubscriptArg::Slice {
                                kind: descriptor.expect("slice argument has a descriptor kind"),
                                lower: lower.as_ref().map(|bound| self.expr(bound)),
                                upper: upper.as_ref().map(|bound| self.expr(bound)),
                                step: step.as_ref().map(|bound| self.expr(bound)),
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                // Value getters defer every call-local argument adaptation
                // until after the RHS. Building the raw descriptor/index list
                // above has already performed each source evaluation once.
                let value_rhs = plan.setter.as_ref().map(|_| self.expr(rhs_expression));
                let getter_args = raw_args
                    .iter()
                    .enumerate()
                    .map(|(position, argument)| match argument {
                        MirSubscriptArg::Index(register) => MirSubscriptArg::Index(
                            self.apply_checked_call_value_adjustments(
                                &plan.getter,
                                crate::checked::CheckedCallArgumentSource::Positional(position),
                                *register,
                                source_spans[position]
                                    .clone()
                                    .unwrap_or_else(|| target.source_span()),
                            ),
                        ),
                        slice => slice.clone(),
                    })
                    .collect::<Vec<_>>();
                let getter_sources = getter_args
                    .iter()
                    .zip(&source_spans)
                    .filter_map(|(argument, source)| match (argument, source) {
                        (MirSubscriptArg::Index(register), Some(source)) => {
                            Some((source.clone(), *register))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let getter_places = source_places
                    .iter()
                    .enumerate()
                    .map(|(position, place)| {
                        Self::checked_call_source_place(
                            &plan.getter,
                            crate::checked::CheckedCallArgumentSource::Positional(position),
                            place,
                        )
                    })
                    .collect::<Vec<_>>();
                let getter_call =
                    self.mir_subscript_call_contract(plan.getter.clone(), &getter_sources);

                if plan.setter.is_none() {
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::MultiIndex {
                        dest: handle,
                        object: receiver,
                        args: getter_args,
                        object_place: receiver_place,
                        arg_places: getter_places,
                        call: Some(getter_call),
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                let rhs = value_rhs.expect("value-returning augmented getter has an RHS");
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::MultiIndex {
                    dest: current,
                    object: receiver,
                    args: getter_args,
                    object_place: receiver_place.clone(),
                    arg_places: getter_places,
                    call: Some(getter_call),
                });
                let result = self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                self.emit(MirInstr::BinOp {
                    op,
                    dest: result,
                    a: current,
                    b: rhs,
                    resolved: None,
                });
                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let setter_args = raw_args
                    .iter()
                    .enumerate()
                    .map(|(position, argument)| match argument {
                        MirSubscriptArg::Index(register) => {
                            let source_kind =
                                crate::checked::CheckedCallArgumentSource::Positional(position);
                            let raw = self.reload_augmented_source(
                                *register,
                                &source_places[position],
                                Self::checked_call_source_mutates(&plan.getter, source_kind),
                                source_spans[position]
                                    .clone()
                                    .unwrap_or_else(|| target.source_span()),
                            );
                            MirSubscriptArg::Index(
                                self.apply_checked_call_value_adjustments(
                                    setter,
                                    source_kind,
                                    raw,
                                    source_spans[position]
                                        .clone()
                                        .unwrap_or_else(|| target.source_span()),
                                ),
                            )
                        }
                        slice => slice.clone(),
                    })
                    .collect::<Vec<_>>();
                let setter_places = source_places
                    .iter()
                    .enumerate()
                    .map(|(position, place)| {
                        Self::checked_call_source_place(
                            setter,
                            crate::checked::CheckedCallArgumentSource::Positional(position),
                            place,
                        )
                    })
                    .collect::<Vec<_>>();
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(source.len())
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let mut setter_sources = setter_args
                    .iter()
                    .zip(&source_spans)
                    .filter_map(|(argument, source)| match (argument, source) {
                        (MirSubscriptArg::Index(register), Some(source)) => {
                            Some((source.clone(), *register))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                setter_sources.push((
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                    value,
                ));
                let setter_call = self.mir_subscript_call_contract(setter.clone(), &setter_sources);
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: setter_args,
                    arg_places: setter_places,
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            _ => false,
        }
    }

    /// Lower a slice or multidimensional assignment through the checker-selected
    /// `__setitem__` implementation. Unlike an ordinary `MirPlace` projection,
    /// every slice remains a first-class descriptor argument and the receiver
    /// place is retained for `mut self` write-back.
    fn lower_subscript_set(&mut self, target: &Expr, value_expression: &Expr) -> bool {
        if self.checked_call_contract(target).is_none() {
            return false;
        }
        if let ExprKind::Index { object, index } = &target.kind {
            let Some(value_keyword) = self.checked_adjustments(target).into_iter().find_map(
                |adjustment| match adjustment {
                    crate::SemanticAdjustment::SliceDescriptors {
                        descriptors,
                        set_value_keyword,
                    } if descriptors.as_slice() == [None] => Some(set_value_keyword),
                    _ => None,
                },
            ) else {
                return false;
            };
            let (receiver, receiver_place) = self.lower_call_receiver(object);
            let (argument_register, argument_place) = self.lower_call_argument(index);
            let argument = MirSubscriptArg::Index(argument_register);
            let (value, value_place) = self.lower_assignment_value(target, value_expression);
            let call = self
                .subscript_call_contract(
                    target,
                    &[
                        (index.source_span(), argument_register),
                        (value_expression.source_span(), value),
                    ],
                )
                .expect("checked nominal subscript setter has a call contract");
            self.emit_interior_invalidations(index, None);
            self.emit_interior_invalidations(value_expression, None);
            self.emit_interior_invalidations(target, None);
            self.emit(MirInstr::MultiSet {
                receiver,
                receiver_place,
                args: vec![argument],
                arg_places: vec![argument_place],
                value,
                value_place,
                value_keyword,
                call,
            });
            return true;
        }
        let (object, source_arguments): (&Expr, Option<&[crate::ast::SubscriptArg]>) =
            match &target.kind {
                ExprKind::Slice { object, .. } => (object, None),
                ExprKind::MultiIndex { object, args } => (object, Some(args)),
                _ => return false,
            };
        let Some((descriptors, value_keyword)) = self
            .checked_adjustments(target)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    set_value_keyword,
                } => Some((descriptors, set_value_keyword)),
                _ => None,
            })
        else {
            self.emit(MirInstr::Unsupported(
                "checked subscript assignment lacks descriptor metadata".to_string(),
            ));
            return true;
        };

        // Current Mojo evaluates the nominal receiver first, then indices and
        // bounds from left to right, and finally the assignment RHS.
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let mut arg_places = Vec::with_capacity(descriptors.len());
        let mut parameter_sources = Vec::new();
        let args = if let Some(arguments) = source_arguments {
            arguments
                .iter()
                .zip(descriptors)
                .map(|(argument, descriptor)| match argument {
                    crate::ast::SubscriptArg::Index(value) => {
                        debug_assert!(descriptor.is_none());
                        let (register, place) = self.lower_call_argument(value);
                        arg_places.push(place);
                        parameter_sources.push((value.source_span(), register));
                        MirSubscriptArg::Index(register)
                    }
                    crate::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    } => {
                        arg_places.push(None);
                        MirSubscriptArg::Slice {
                            kind: descriptor
                                .expect("slice assignment argument has descriptor kind"),
                            lower: lower.as_ref().map(|bound| self.expr(bound)),
                            upper: upper.as_ref().map(|bound| self.expr(bound)),
                            step: step.as_ref().map(|bound| self.expr(bound)),
                        }
                    }
                })
                .collect()
        } else {
            arg_places.push(None);
            let ExprKind::Slice {
                lower, upper, step, ..
            } = &target.kind
            else {
                unreachable!("single descriptor assignment is a Slice")
            };
            vec![MirSubscriptArg::Slice {
                kind: descriptors
                    .first()
                    .copied()
                    .flatten()
                    .expect("slice assignment has descriptor kind"),
                lower: lower.as_ref().map(|bound| self.expr(bound)),
                upper: upper.as_ref().map(|bound| self.expr(bound)),
                step: step.as_ref().map(|bound| self.expr(bound)),
            }]
        };
        let (value, value_place) = self.lower_assignment_value(target, value_expression);
        parameter_sources.push((value_expression.source_span(), value));
        let call = self
            .subscript_call_contract(target, &parameter_sources)
            .expect("checked nominal subscript setter has a call contract");
        if let Some(arguments) = source_arguments {
            for argument in arguments {
                if let crate::ast::SubscriptArg::Index(argument) = argument {
                    self.emit_interior_invalidations(argument, None);
                }
            }
        }
        self.emit_interior_invalidations(value_expression, None);
        self.emit_interior_invalidations(target, None);
        self.emit(MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            value_keyword,
            call,
        });
        true
    }

    fn lower_assignment_value(
        &mut self,
        target: &Expr,
        value_expression: &Expr,
    ) -> (Reg, Option<MirPlace>) {
        let (mut value, value_place) = self.lower_call_argument(value_expression);
        if let Some(target_ty) = self.checked_ty(target) {
            value = self.materialize_register(value, &target_ty, value_expression.source_span());
        }
        (value, value_place)
    }

    /// Like [`place`](Self::place), but returns `None` for a non-place expression
    /// (a call result, a literal, …) instead of panicking — used at a method-call
    /// receiver, which may be a temporary. Only evaluates subscript indices when
    /// the whole chain is a place.
    /// Lower a `try` sub-region (`body`/`except`/`else`/`finally`) into a
    /// self-contained mini-CFG (block ids local, entry = 0) that **shares this
    /// function's register, variable, and span space** — so it addresses the same
    /// slots. The region's own control flow (`if`/`while`/`for`) becomes local
    /// blocks; the VM runs it recursively.
    fn lower_region(
        &mut self,
        body: &[Stmt],
        ext_loops: &[(hir::BlockId, hir::BlockId, Vec<VarId>)],
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) -> Vec<MirBlock> {
        let checked: Vec<_> = self.checked_expressions.values().cloned().collect();
        let region_cfg = hir::Cfg::build_seeded_checked_with_declarations(
            self.vars.clone(),
            body,
            ext_loops,
            &checked,
            &self.checked_declarations,
        );
        let mut region = MirFunction {
            blocks: Vec::new(),
            n_regs: 0,
            n_vars: 0,
            var_names: Vec::new(),
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: self.returns_reference,
            var_tys: HashMap::new(),
            ret_ty: self.f.ret_ty.clone(),
            raises: self.f.raises,
            error_ty: self.f.error_ty.clone(),
            spans: std::mem::take(&mut self.f.spans), // accumulate into the shared table
            reg_types: std::mem::take(&mut self.f.reg_types),
        };
        let mut map: HashMap<hir::BlockId, MirBlockId> = HashMap::new();
        for hb in region_cfg.g.node_indices() {
            map.insert(hb, region.blocks.len());
            region.blocks.push(MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Return(None),
            });
        }
        // Region-local inference can discover new slots, but it must not
        // replace exact types already established in the enclosing frame. In
        // particular, same-spelled exception targets and outer locals occupy
        // different owner slots even though name-based region seeding sees both.
        let mut region_var_types = region_cfg.var_types.clone();
        region_var_types.extend(self.var_types.clone());
        {
            let mut fl = Flatten {
                f: &mut region,
                cur: 0,
                next_reg: self.next_reg,
                vars: region_cfg.vars.clone(),
                var_types: region_var_types,
                owner_vars: self.owner_vars.clone(),
                nested: self.nested.clone(), // a `try` region may call a nested `def`
                overloads: self.overloads.clone(),
                checked_expressions: self.checked_expressions.clone(),
                checked_declarations: self.checked_declarations.clone(),
                active_semantics: Vec::new(),
                aliases: self.aliases.clone(),
                runtime_aliases: self.runtime_aliases.clone(),
                aggregate_loans: self.aggregate_loans.clone(),
                reassigned_names: self.reassigned_names.clone(),
                returns_reference: self.returns_reference,
            };
            for hb in region_cfg.g.node_indices() {
                fl.cur = map[&hb];
                for instr in &region_cfg.g[hb].instrs {
                    fl.lower_instr(instr, outer_map);
                }
                let fallback = Terminator::FallOff;
                let term = region_cfg.g[hb].term.as_ref().unwrap_or(&fallback);
                // Region terminators resolve local jumps via the region's own `map`;
                // an `EscapeJump` resolves its outer-loop target via `outer_map`.
                let mterm = fl.lower_term(term, &map, outer_map);
                fl.f.blocks[fl.cur].term = mterm;
            }
            self.next_reg = fl.next_reg;
            self.vars = fl.vars.clone();
            self.var_types = fl.var_types.clone();
            self.owner_vars = fl.owner_vars.clone();
        }
        self.f.spans = std::mem::take(&mut region.spans);
        self.f.reg_types = std::mem::take(&mut region.reg_types);
        region.blocks
    }

    /// Lower a `try`'s sub-regions and emit the [`MirInstr::Try`]. `ext_loops` are
    /// the enclosing function loops (HIR block ids) a `break`/`continue` may escape
    /// to; `outer_map` resolves them to MIR blocks. Shared by the primary
    /// (`HirInstr::Try`) and fallback (`lower_stmt`) paths.
    fn emit_try(
        &mut self,
        regions: TryRegions<'_>,
        ext_loops: &[(hir::BlockId, hir::BlockId, Vec<VarId>)],
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        let TryRegions {
            body,
            except,
            orelse,
            finalbody,
            handler_binding,
        } = regions;
        let body_blocks = self.lower_region(body, ext_loops, outer_map);
        let handler = match except {
            Some((name, ex_body)) => {
                let slot = name.as_ref().map(|name| {
                    handler_binding
                        .map(|binding| self.declare_binding_var(binding, name))
                        .unwrap_or_else(|| self.var(name))
                });
                if let Some(slot) = slot {
                    // The checker rejects a try whose body can raise more than
                    // one error type, so copying the first propagating raising
                    // fact types the handler binding without re-inference.
                    let error =
                        region_error_type(&body_blocks, &self.f.reg_types).unwrap_or(Ty::Error);
                    self.var_types.entry(slot).or_insert(error);
                }
                let blocks = self.lower_region(ex_body, ext_loops, outer_map);
                Some((slot, blocks))
            }
            None => None,
        };
        let orelse_blocks = orelse
            .as_ref()
            .map(|b| self.lower_region(b, ext_loops, outer_map));
        let finalbody_blocks = finalbody
            .as_ref()
            .map(|b| self.lower_region(b, ext_loops, outer_map));
        self.emit(MirInstr::Try {
            body: body_blocks,
            handler,
            orelse: orelse_blocks,
            finalbody: finalbody_blocks,
            cleanup: Vec::new(),
        });
    }

    /// A place for a call argument's *write-back* — a variable or a field chain,
    /// **without** any dynamic index (so building it emits nothing and avoids
    /// re-evaluating an index that the argument's value already consumed). Returns
    /// `None` for a temporary or an indexed place (write-back to those is refused by
    /// the VM). Distinct from [`Self::try_place`], which emits index evaluations.
    fn simple_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => Some(self.expression_place_root(name, e)),
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.simple_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })?;
                let mut p = self.resolved_place(name);
                let ty = self.checked_place_ty(e).or_else(|| self.checked_ty(e))?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            _ => None,
        }
    }

    /// Decompose `e` into a place **iff** it is a variable or a *pure field
    /// chain* rooted at one (`x`, `p.a`, `p.a.b`) — no dynamic index. Used to
    /// distinguish a place read (`LoadPlace`) from a temporary/indexed read, and
    /// a partial move (`p.a^`) from an untracked indexed transfer. Emits nothing.
    fn pure_field_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => {
                // `Self.<name>` (a reified value-parameter read, e.g. `Self.size`)
                // resolves off the receiver `self`: `Self` in expression position is
                // an alias for `self`, and the backend's field navigation also
                // searches a struct's `value_params`. `Self` never appears bare in an
                // expression (only `Self.field`), so this alias is safe.
                let root = if name == "Self" { "self" } else { name };
                Some(self.expression_place_root(root, e))
            }
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.pure_field_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            _ => None,
        }
    }

    fn try_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => Some(self.expression_place_root(name, e)),
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.try_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            ExprKind::Index { object, index } => {
                let mut p = self.try_place(object)?;
                // A literal index into compiler-private heterogeneous Tuple
                // storage is part of the place's static identity. Keeping it
                // out of a register lets ownership distinguish element 0 from
                // element 1 while every dynamic/nominal subscript retains the
                // ordinary single-evaluation Index(Reg) path.
                let projection = match self.checked_ty(object) {
                    Some(Ty::Tuple(_)) => exact_nonnegative_index(index)
                        .map(Proj::ConstIndex)
                        .unwrap_or_else(|| Proj::Index(self.expr(index))),
                    _ => Proj::Index(self.expr(index)),
                };
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(projection, ty);
                } else {
                    p.proj.push(projection);
                }
                Some(p)
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })?;
                let mut p = self.resolved_place(name);
                let ty = self.checked_place_ty(e).or_else(|| self.checked_ty(e))?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            _ => None,
        }
    }

    /// Lower the "catch-all" straight-line statements. Every reachable case is
    /// handled; the categorization decisions are documented per arm. `outer_map`
    /// threads the enclosing function's block map for a fallback-path `try`.
    fn lower_stmt(
        &mut self,
        s: &Stmt,
        statement_binding: Option<crate::origin::OwnerId>,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        match &s.kind {
            StmtKind::RefDecl { name, value } => {
                let reference = self.var(name);
                if let Some(binding) = statement_binding {
                    self.owner_vars.insert(binding, reference);
                }
                let mutable = self.checked_borrow_mutability(value).unwrap_or(true);
                if self.reference_result(value).is_some()
                    || !matches!(
                        value.kind,
                        ExprKind::Identifier(_)
                            | ExprKind::Member { .. }
                            | ExprKind::Index { .. }
                            | ExprKind::TypeApply { .. }
                    )
                {
                    let source = self.expr(value);
                    self.runtime_aliases.insert(reference);
                    // A reference-producing expression stores a runtime handle
                    // in this local slot. Carry its checked `Ty::Ref` onto the
                    // slot immediately: later aggregate construction may need
                    // to forward the handle before any ordinary read has had a
                    // chance to seed `var_types` incidentally.
                    let binding_ty = self
                        .f
                        .reg_types
                        .get(&source.0)
                        .filter(|ty| matches!(ty, Ty::Ref(_)))
                        .cloned()
                        .or_else(|| self.reference_result(value).map(Ty::Ref))
                        .or_else(|| self.checked_ty(value));
                    if let Some(ty) = binding_ty.clone() {
                        self.var_types.insert(reference, ty);
                    }
                    self.emit(MirInstr::DefVar {
                        var: reference,
                        src: source,
                        binding_ty,
                    });
                    let candidates: Vec<&Expr> = match &value.kind {
                        ExprKind::Call { args, kwargs, .. } => args
                            .iter()
                            .chain(kwargs.iter().map(|argument| &argument.value))
                            .collect(),
                        ExprKind::MethodCall {
                            object,
                            args,
                            kwargs,
                            ..
                        } => std::iter::once(object.as_ref())
                            .chain(args.iter())
                            .chain(kwargs.iter().map(|argument| &argument.value))
                            .collect(),
                        _ => Vec::new(),
                    };
                    let candidate_places: Vec<_> = candidates
                        .into_iter()
                        .filter_map(|candidate| {
                            self.simple_place(candidate)
                                .map(|place| (self.checked_owner(candidate), place))
                        })
                        .collect();
                    let checked_places = self.checked_reference_places(value);
                    let mut loans = Vec::new();
                    for origin in checked_places {
                        let fallback = candidate_places
                            .iter()
                            .find(|(owner, _)| *owner == Some(origin.root))
                            .map(|(_, place)| place.root);
                        let Some(canonical) = self.mir_interior_origin(&origin, fallback) else {
                            continue;
                        };
                        // The canonical output origin is also the physical
                        // lifetime dependency.  A candidate may itself be a
                        // runtime reference handle, whose slot is not the
                        // ultimate owner; in that case retain the canonical
                        // owner root directly instead of arbitrarily choosing
                        // the first argument handle.
                        let place = candidate_places
                            .iter()
                            .find(|(_, place)| place.root == canonical.root)
                            .map(|(_, place)| place.clone())
                            .unwrap_or_else(|| {
                                MirPlace::root(
                                    canonical.root,
                                    self.var_types.get(&canonical.root).cloned(),
                                )
                            });
                        let interior = canonical
                            .path
                            .iter()
                            .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                            .then_some(canonical);
                        loans.push(MirLoan {
                            place,
                            mutable,
                            interior,
                        });
                    }
                    if let Some(first) = loans.first() {
                        let marker =
                            self.fresh_typed(s.source_span(), Some(first.place.root), Ty::None);
                        self.aggregate_loans.insert(reference, loans.clone());
                        self.emit(MirInstr::EstablishLoans {
                            reference,
                            loans,
                            marker,
                        });
                    }
                    return;
                }
                // A projection below a nominal reference-returning accessor is
                // rooted in that runtime handle, not in raw nominal storage.
                // Materializing the checked accessor here also ensures the
                // subscript and its index are evaluated exactly once at the
                // reference declaration.
                let projected_reference = self.lower_projected_reference_place(value);
                let place = projected_reference
                    .clone()
                    .unwrap_or_else(|| self.place(value));
                // Some reference-producing places (currently Dict lookup)
                // define a new interior generation as part of locating the
                // storage. Invalidate the previous generation before installing
                // this reference's fresh one.
                if projected_reference.is_none() {
                    self.emit_interior_invalidations(value, None);
                }
                let checked_places = self.checked_reference_places(value);
                let mut loans = Vec::new();
                for origin in checked_places {
                    let Some(canonical) = self.mir_interior_origin(&origin, Some(place.root))
                    else {
                        continue;
                    };
                    let interior = canonical
                        .path
                        .iter()
                        .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                        .then_some(canonical);
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior,
                    });
                }
                if loans.is_empty() {
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior: None,
                    });
                }
                // A substituted local alias has no runtime handle value, but
                // its slot is still the checked capability through which every
                // derived place is accessed. Retain that `ref T` declaration
                // type so MIR verification can prove `place.through` instead of
                // treating the analytical alias slot as untyped storage.
                if let Some(reference_ty) = statement_binding.and_then(|binding| {
                    self.checked_declarations
                        .iter()
                        .find(|declaration| declaration.binding == Some(binding))
                        .and_then(|declaration| declaration.ty.clone())
                        .filter(|ty| matches!(ty, Ty::Ref(_)))
                }) {
                    self.var_types.insert(reference, reference_ty);
                }
                self.aliases.insert(reference, loans[0].clone());
                self.aggregate_loans.insert(reference, loans.clone());
                let marker = self.fresh_typed(s.source_span(), Some(place.root), Ty::None);
                self.emit(MirInstr::EstablishLoans {
                    reference,
                    loans,
                    marker,
                });
            }
            // --- Writes through a place (any nesting) --------------------------
            StmtKind::SetPlace { place, value } => {
                if self.lower_subscript_set(place, value) {
                    return;
                }
                let (src, _) = self.lower_assignment_value(place, value);
                self.emit_interior_invalidations(place, None);
                // A store through an origin-bearing pointer writes its source
                // place; the checker fixed the offset to 0 and required
                // mutable provenance. A stably bound pointer substitutes the
                // owner place; otherwise the store goes through the handle.
                if let ExprKind::Index { object, .. } = &place.kind {
                    if let Some(target) = self.pointer_deref_place(object) {
                        self.emit(MirInstr::Store { place: target, src });
                        return;
                    }
                    if self.is_origin_bearing_pointer(object) {
                        let reference = self.expr(object);
                        self.emit(MirInstr::WriteRef {
                            reference,
                            value: src,
                        });
                        return;
                    }
                }
                let p = self.place(place);
                let stores_reference = matches!(p.ty, Some(Ty::Ref(_)))
                    && self.checked_adjustments(value).iter().any(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::BorrowShared
                                | crate::SemanticAdjustment::BorrowMutable
                        )
                    });
                if stores_reference {
                    self.emit(MirInstr::StoreRef {
                        place: p,
                        reference: src,
                    });
                } else {
                    self.emit(MirInstr::Store { place: p, src });
                }
            }
            StmtKind::AugAssign { place, op, value } => {
                if self.lower_augmented_subscript(place, *op, value) {
                    return;
                }
                // `place OP= e` — read the place, apply the op, write it back. A bare
                // variable uses the `UseVar`/`DefVar` fast path (what move-analysis
                // reads for a var); a projected place uses `LoadPlace`/`Store`, with
                // the place flattened once so its indices are evaluated once.
                if let ExprKind::Identifier(name) = &place.kind {
                    // Opaque structured statements retain the source spelling,
                    // while HIR may give a same-spelled sibling declaration a
                    // distinct runtime slot. Resolve the checked owner for the
                    // write half just as `self.expr(place)` does for the read.
                    let var = self.expression_var(name, place);
                    let cur = self.expr(place);
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    if self.runtime_aliases.contains(&var) {
                        let handle = self.fresh(place.source_span(), Some(var));
                        self.emit(MirInstr::MakeRef {
                            dest: handle,
                            place: {
                                let mut place =
                                    MirPlace::root(var, self.var_types.get(&var).cloned());
                                place.through = Some(var);
                                place
                            },
                        });
                        self.emit(MirInstr::WriteRef {
                            reference: handle,
                            value: res,
                        });
                    } else if let Some(loan) = self.aliases.get(&var).cloned() {
                        let mut target = loan.place;
                        target.through = Some(var);
                        self.emit(MirInstr::Store {
                            place: target,
                            src: res,
                        });
                    } else {
                        self.emit(MirInstr::DefVar {
                            var,
                            src: res,
                            binding_ty: None,
                        });
                    }
                } else if let ExprKind::Index { object, .. } = &place.kind
                    && let Some(target) = self.pointer_deref_place(object)
                {
                    // `p[0] OP= e` through a stably bound pointer: owner-place
                    // load and store, exactly like an alias write-back.
                    let cur = self.fresh(span(place), Some(target.root));
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: target.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store {
                        place: target,
                        src: res,
                    });
                } else if let ExprKind::Index { object, .. } = &place.kind
                    && self.is_origin_bearing_pointer(object)
                {
                    // `p[0] OP= e` through an origin-bearing pointer: read and
                    // write through the handle, evaluated once.
                    let reference = self.expr(object);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else if matches!(self.checked_place_ty(place), Some(Ty::Ref(_))) {
                    // A projected ref-valued slot (for example an element of
                    // `List[ref T]`) is two distinct places: the container slot
                    // stores the handle, while augmented assignment reads and
                    // writes the referent. Preserve the handle explicitly so a
                    // nominal container's index dunder cannot feed `ref` itself
                    // into the arithmetic operation.
                    let reference = self.reference_handle(place);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else {
                    let p = self.place(place);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: p.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store { place: p, src: res });
                }
            }

            // --- Simple effectful statements -----------------------------------
            StmtKind::Raise(e) => {
                let src = self.expr(e);
                self.emit(MirInstr::Raise { src });
            }
            // `comptime N = e` is an ordinary `Int` binding at runtime.
            StmtKind::Comptime { name, value } => {
                let src = self.expr(value);
                let var = statement_binding
                    .map(|binding| self.declare_binding_var(binding, name))
                    .unwrap_or_else(|| self.var(name));
                // A comptime statement remains a fallback HIR statement rather
                // than `HirInstr::Bind`; copy its checked expression type when
                // present, or the already-typed initializer register for
                // synthetic/compatibility paths. This makes closure capture
                // places typed without relying on an unrelated later use.
                let binding_ty = self
                    .checked_ty(value)
                    .or_else(|| self.f.reg_types.get(&src.0).cloned());
                if let Some(ty) = binding_ty.clone() {
                    self.var_types.insert(var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var,
                    src,
                    binding_ty,
                });
            }
            // `pass` has no runtime effect. Imports were consumed by linking and
            // are no-ops in a lowered module body.
            StmtKind::Pass | StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}

            // `try`/`except`/`else`/`finally` — each part lowers to a mini-CFG that
            // shares this function's slots; the VM runs them with `exec_try`
            // semantics. `cleanup` (the exceptional-edge drops) is filled by the
            // drop-elaboration pass.
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                // A `break`/`continue` that leaves the `try` (targeting an enclosing
                // loop) needs the outer loop's target block, which the self-contained
                // mini-CFG region can't name — refuse cleanly rather than build an
                // ill-formed region. (A `return` crossing out is fine: it surfaces as
                // a `Flow::Return` the block driver handles.)
                let crosses = region_crosses_control(body)
                    || except
                        .as_ref()
                        .is_some_and(|(_, b)| region_crosses_control(b))
                    || orelse.as_ref().is_some_and(|b| region_crosses_control(b))
                    || finalbody
                        .as_ref()
                        .is_some_and(|b| region_crosses_control(b));
                if crosses {
                    self.emit(MirInstr::Unsupported(
                        "try with break/continue crossing the try boundary".into(),
                    ));
                    return;
                }
                // Fallback path (a `try` whose enclosing loops are region-local, so
                // the HIR left it as an opaque `Stmt`): no escapable loops.
                self.emit_try(
                    TryRegions {
                        body,
                        except,
                        orelse,
                        finalbody,
                        handler_binding: statement_binding,
                    },
                    &[],
                    outer_map,
                );
            }
            // A direct nested declaration creates its closure exactly here. Copy
            // and move captures therefore snapshot/transfer before any following
            // statement can mutate or use the source binding. Later calls and
            // first-class uses load this internal closure slot.
            StmtKind::Def { name, .. } => {
                let info = statement_binding
                    .and_then(|binding| self.nested.get(&binding))
                    .filter(|info| info.materialized_here)
                    .cloned();
                if let Some(info) = info {
                    let src = self.emit_nested_closure(&info, s.source_span(), false);
                    let var = self.declare_binding_var(info.binding, name);
                    if let Some(ty) = &info.callable_ty {
                        self.var_types.entry(var).or_insert_with(|| ty.clone());
                    }
                    self.emit(MirInstr::DefVar {
                        var,
                        src,
                        binding_ty: info.callable_ty,
                    });
                } else {
                    self.emit(MirInstr::Unsupported(
                        "nested def/struct/trait declaration".into(),
                    ));
                }
            }
            // A nested `def` we couldn't lift because it nests another declaration,
            // or a nested `struct`/`trait`, stays a clean `Unsupported`.
            StmtKind::Struct { .. } | StmtKind::Trait { .. } => self.emit(MirInstr::Unsupported(
                "nested def/struct/trait declaration".into(),
            )),

            // Tuple unpacking `a, b = t`: evaluate the tuple once, then bind each
            // target from its element (a NAME → `DefVar`; a place → `Store`).
            StmtKind::Unpack { targets, value } => {
                let plan = self
                    .tuple_unpack_plan(value)
                    .expect("checked tuple unpack carries an extraction plan");
                assert_eq!(
                    plan.len(),
                    targets.len(),
                    "checked tuple unpack arity matches its targets"
                );
                let base_place = self.simple_place(value);
                let tuple = self.expr(value);
                for (i, (target, extraction)) in targets.iter().zip(plan).enumerate() {
                    let idx = self.fresh_typed(span(target), None, Ty::Int);
                    self.emit(MirInstr::Const {
                        dest: idx,
                        k: Const::Int(i as i64),
                    });
                    let raw_ty = extraction
                        .reference
                        .clone()
                        .map(Ty::Ref)
                        .unwrap_or_else(|| extraction.ty.clone());
                    let raw = self.fresh_typed(span(target), None, raw_ty.clone());
                    let call = extraction.accessor.clone().map(|target| MirSubscriptCall {
                        target,
                        raises: None,
                        result_ty: raw_ty.clone(),
                        receiver_requires_place: extraction.reference.is_some(),
                        receiver_convention: extraction
                            .reference
                            .as_ref()
                            .map(|_| crate::ast::ArgConvention::Ref),
                        arguments: Vec::new(),
                        capture_accesses: Vec::new(),
                        reference_result: extraction.reference.clone(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                    });
                    let intrinsic = call
                        .is_none()
                        .then(|| self.intrinsic_index_dispatch(value))
                        .flatten();
                    self.emit(MirInstr::Index {
                        dest: raw,
                        base: tuple,
                        index: idx,
                        base_place: base_place.clone(),
                        index_place: None,
                        call,
                        intrinsic,
                    });
                    let elem = if extraction.reference.is_some() {
                        let value = self.fresh_typed(
                            span(target),
                            base_place.as_ref().map(|place| place.root),
                            extraction.ty,
                        );
                        self.emit(MirInstr::ReadRef {
                            dest: value,
                            reference: raw,
                        });
                        value
                    } else {
                        raw
                    };
                    match &target.kind {
                        ExprKind::Identifier(name) => {
                            let var = self.expression_var(name, target);
                            self.emit_interior_invalidations(target, Some(var));
                            let binding_ty = self
                                .checked_place_ty(target)
                                .or_else(|| self.checked_ty(target));
                            if let Some(ty) = binding_ty.clone() {
                                self.var_types.insert(var, ty);
                            }
                            self.emit(MirInstr::DefVar {
                                var,
                                src: elem,
                                binding_ty,
                            });
                        }
                        _ => {
                            let place = self.place(target);
                            self.emit_interior_invalidations(target, Some(place.root));
                            self.emit(MirInstr::Store { place, src: elem });
                        }
                    }
                }
            }

            // --- Unreachable after the checker ---------------------------------
            // Parse-only statements are flagged `Unsupported`, so a checked program
            // never reaches MIR with them.
            StmtKind::With { .. } | StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. } => {
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked statement reached MIR lowering: {:?}",
                    s.kind
                )));
            }
            // These are lowered by `hir::Lower` directly (to instrs/terminators), so
            // they never arrive here wrapped in a `HirInstr::Stmt`.
            StmtKind::If { .. }
            | StmtKind::While { .. }
            | StmtKind::For { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Return(_)
            | StmtKind::VarDecl { .. }
            | StmtKind::Assign { .. }
            | StmtKind::Expr(_) => {
                self.emit(MirInstr::Unsupported(format!(
                    "malformed HIR statement instruction: {:?}",
                    s.kind
                )));
            }
        }
    }

    fn lower_hir_stmt(
        &mut self,
        statement: &crate::hir::HirStmt,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        let mut index = HashMap::new();
        for (syntax, expression) in statement_expression_roots(&statement.syntax)
            .into_iter()
            .zip(&statement.expressions)
        {
            index_hir_expression(syntax, expression, &mut index);
        }
        self.active_semantics.push(index);
        self.lower_stmt(&statement.syntax, statement.binding, outer_map);
        self.active_semantics.pop();
    }

    fn lower_return_value(&mut self, expression: &hir::HirExpr) -> Reg {
        if self.returns_reference {
            // Returning an ordinary place borrows that place. Returning a place
            // whose *storage* is already `ref T`, or forwarding another
            // reference-producing expression, instead returns the existing
            // handle. Borrowing the ref-valued slot would manufacture `ref ref T`
            // at runtime.
            let forwards_handle = matches!(
                expression.place.as_ref().map(|place| &place.ty),
                Some(Ty::Ref(_))
            ) || expression.adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::ReferenceResult { .. }
                )
            });
            if forwards_handle {
                self.reference_handle_hir(expression)
            } else {
                let place = self
                    .projected_reference_place_hir(expression)
                    .unwrap_or_else(|| self.place_hir(expression));
                let dest = self.fresh(expression.source_span(), Some(place.root));
                self.emit(MirInstr::MakeRef { dest, place });
                dest
            }
        } else {
            let value = self.expr_hir(expression);
            match self.f.ret_ty.clone() {
                Some(target) => self.materialize_register(value, &target, expression.source_span()),
                None => value,
            }
        }
    }

    /// Lower a HIR block terminator; the branch/return operands are flattened into
    /// `self.cur` first, then the `MirTerm` references their result registers.
    fn lower_term(
        &mut self,
        t: &Terminator,
        map: &HashMap<hir::BlockId, MirBlockId>,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) -> MirTerm {
        match t {
            Terminator::Jump(b) => MirTerm::Jump(map[b]),
            Terminator::Branch {
                cond,
                then_b,
                else_b,
            } => {
                let c = self.expr_hir(cond); // evaluated at the end of this block
                MirTerm::Branch {
                    cond: c,
                    then_b: map[then_b],
                    else_b: map[else_b],
                }
            }
            Terminator::Return(expression) => {
                MirTerm::Return(expression.as_ref().map(|e| self.lower_return_value(e)))
            }
            Terminator::ReturnWithCleanup { value, cleanup } => {
                // Preserve source evaluation order: materialize the return value
                // before destroying loop-owned iterators. In particular, `return
                // item^` must transfer the yielded element before the iterator's
                // residual storage is released.
                let value = value.as_ref().map(|e| self.lower_return_value(e));
                for var in cleanup {
                    // Keep the cleanup root live into this return block. Without
                    // this marker, edge-based last-use elaboration can destroy a
                    // loop iterator at block entry (before the return value and
                    // the current yielded element have been handled).
                    self.emit(MirInstr::KeepAlive { var: *var });
                }
                MirTerm::ReturnWithCleanup {
                    value,
                    cleanup: cleanup.clone(),
                }
            }
            Terminator::FallOff => MirTerm::FallOff,
            // An outward `break`/`continue`: the target is an enclosing-function
            // block, resolved via `outer_map` (`cleanup` filled by drop elaboration).
            Terminator::EscapeJump(b) => MirTerm::EscapeJump {
                target: outer_map[b],
                cleanup: Vec::new(),
            },
        }
    }
}

/// Lower a whole HIR control-flow graph (one function body) into a `MirFunction`.
/// Each HIR block becomes a MIR block (same order); a single [`Flatten`] threads
/// the register counter, the variable interner (seeded from `cfg.vars` so IDs
/// agree with the HIR), and the span table across the whole function.
pub fn lower_cfg(cfg: &Cfg) -> MirFunction {
    lower_cfg_nested(
        cfg,
        &HashMap::new(),
        &crate::symbol::OverloadSets::default(),
        false,
        &[],
        &[],
    )
}

/// [`lower_cfg`] with a nested-`def` registry in scope: a call to a registered
/// nested `def` is rewritten to its lifted function (captures prepended) and the
/// nested `def` statement lowers to nothing.
fn lower_cfg_nested(
    cfg: &Cfg,
    nested: &HashMap<crate::origin::OwnerId, NestedInfo>,
    overloads: &crate::symbol::OverloadSets,
    returns_reference: bool,
    reference_parameters: &[bool],
    capture_bindings: &[crate::origin::OwnerId],
) -> MirFunction {
    let mut mir = MirFunction {
        blocks: Vec::new(),
        n_regs: 0,
        n_vars: cfg.vars.len(),
        var_names: cfg.vars.clone(),
        n_params: cfg.n_params,
        param_types: Vec::new(),
        owned_params: Vec::new(),
        deinit_params: Vec::new(),
        ref_params: Vec::new(),
        returns_reference,
        var_tys: HashMap::new(),
        ret_ty: None,
        raises: false,
        error_ty: None,
        spans: SpanTable::default(),
        reg_types: HashMap::new(),
    };

    // One empty MIR block per HIR block; record the HIR→MIR index mapping so
    // terminators can translate their jump targets.
    let mut map: HashMap<hir::BlockId, MirBlockId> = HashMap::new();
    for hb in cfg.g.node_indices() {
        map.insert(hb, mir.blocks.len());
        mir.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Return(None),
        }); // placeholder term
    }

    {
        let mut fl = Flatten {
            f: &mut mir,
            cur: 0,
            next_reg: 0,
            vars: cfg.vars.clone(),
            var_types: cfg.var_types.clone(),
            owner_vars: capture_bindings
                .iter()
                .copied()
                .enumerate()
                .map(|(slot, binding)| (binding, slot as VarId))
                .collect(),
            nested: nested.clone(),
            overloads: overloads.clone(),
            checked_expressions: cfg.checked_expressions.clone(),
            checked_declarations: cfg.checked_declarations.clone(),
            active_semantics: Vec::new(),
            aliases: HashMap::new(),
            runtime_aliases: reference_parameters
                .iter()
                .take(cfg.n_params)
                .enumerate()
                .filter_map(|(slot, reference)| reference.then_some(slot as VarId))
                .collect(),
            aggregate_loans: HashMap::new(),
            reassigned_names: reassigned_names(cfg, nested),
            returns_reference,
        };
        for hb in cfg.g.node_indices() {
            fl.cur = map[&hb];
            for instr in &cfg.g[hb].instrs {
                // At the function level the "outer" map is this function's own map
                // (a `try`'s escape targets are this function's loop blocks).
                fl.lower_instr(instr, &map);
            }
            let fallback = Terminator::FallOff;
            let term = cfg.g[hb].term.as_ref().unwrap_or(&fallback);
            let mterm = fl.lower_term(term, &map, &map);
            fl.f.blocks[fl.cur].term = mterm;
        }
        fl.f.n_regs = fl.next_reg;
        // The MIR flattener may intern additional locals beyond the HIR's set
        // (short-circuit / iterator temporaries), so take the final interner.
        fl.f.n_vars = fl.vars.len();
        fl.f.var_names = fl.vars.clone();
        fl.f.var_tys = fl.var_types.clone();
    } // `fl` (the &mut borrow of `mir`) ends here

    mir
}

/// A whole program's worth of lowered functions, keyed by name. The synthetic
/// `__toplevel__` holds module initialization and explicit legacy test snippets.
/// Production compilation rejects executable file-scope source statements.
#[derive(Debug)]
pub struct MirProgram {
    pub functions: Vec<(String, MirFunction)>,
    /// Declaration facts needed by execution, normalized once while lowering.
    /// Backends consume this instead of rescanning the source AST.
    pub declarations: MirDeclarations,
    /// Violations of the checked-program contract discovered while lowering.
    /// Backends must reject a program with any entry rather than executing a
    /// guessed fallback representation.
    pub invariant_errors: Vec<String>,
}

/// The error type that can propagate out of a lowered `try`-body region: the
/// first raising fact that would reach this region's handler. Nested regions
/// with their own handler consume their body's raises, so only their
/// handler/orelse/finalbody blocks (and handlerless bodies) propagate.
fn region_error_type(blocks: &[MirBlock], reg_types: &HashMap<u32, Ty>) -> Option<Ty> {
    for block in blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::Raise { src } => {
                    if let Some(ty) = reg_types.get(&src.0) {
                        return Some(ty.clone());
                    }
                }
                MirInstr::Call {
                    raises: Some(ty), ..
                }
                | MirInstr::CallIndirect {
                    raises: Some(ty), ..
                }
                | MirInstr::MethodCall {
                    raises: Some(ty), ..
                } => {
                    return Some(ty.clone());
                }
                MirInstr::Index {
                    call: Some(call), ..
                }
                | MirInstr::Slice {
                    call: Some(call), ..
                }
                | MirInstr::MultiIndex {
                    call: Some(call), ..
                } => {
                    if let Some(ty) = &call.raises {
                        return Some(ty.clone());
                    }
                }
                MirInstr::MultiSet { call, .. } => {
                    if let Some(ty) = &call.raises {
                        return Some(ty.clone());
                    }
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    let mut nested = Vec::new();
                    if handler.is_none() {
                        nested.push(body.as_slice());
                    }
                    if let Some((_, handler_blocks)) = handler {
                        nested.push(handler_blocks.as_slice());
                    }
                    nested.extend(orelse.as_deref());
                    nested.extend(finalbody.as_deref());
                    for region in nested {
                        if let Some(ty) = region_error_type(region, reg_types) {
                            return Some(ty);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Fill remaining register types by copying facts already present in the
/// instruction stream: operand register types, typed places, inline element
/// types, slot types, and declaration returns. This pass never re-implements
/// checker inference — a register whose type cannot be copied from an existing
/// fact is reported as an invariant error. Synthetic loan/consumption markers
/// are typed `Ty::None`. Reference-handle permission follows the place root: a
/// capability root is forwarded (and projections cannot recover stronger
/// permission), while borrowing ordinary storage adds an outer reference layer
/// even when the stored value is itself a reference. Handle loan bookkeeping
/// lives in `EstablishLoans`/`through` metadata, not in the synthetic register
/// origin.
fn close_register_types(
    name: &str,
    f: &mut MirFunction,
    declarations: &MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    fn deref(ty: Ty) -> Ty {
        match ty {
            Ty::Ref(reference) => *reference.referent,
            other => other,
        }
    }
    fn declared_return(declarations: &MirDeclarations, callee: &str) -> Option<Ty> {
        declarations
            .functions
            .iter()
            .find(|declaration| declaration.lowered_name == callee)
            .map(|declaration| declaration.ret_ty.clone())
    }
    fn struct_field(declarations: &MirDeclarations, base: &Ty, field: &str) -> Option<Ty> {
        let Ty::Struct(name, _) = base else {
            return None;
        };
        declarations
            .structs
            .iter()
            .find(|declaration| &declaration.name == name)?
            .fields
            .iter()
            .find(|(candidate, _)| candidate == field)
            .map(|(_, ty)| ty.clone())
    }
    fn close_blocks(
        blocks: &[MirBlock],
        reg_types: &mut HashMap<u32, Ty>,
        var_tys: &mut HashMap<VarId, Ty>,
        declarations: &MirDeclarations,
    ) -> bool {
        let mut changed = false;
        // Compile-time integer registers within this region, so a tuple index
        // through a constant register can copy the selected element type.
        let mut const_ints: HashMap<u32, i64> = HashMap::new();
        let record = |reg_types: &mut HashMap<u32, Ty>, reg: &Reg, ty: Option<Ty>| {
            if let Some(ty) = ty
                && !reg_types.contains_key(&reg.0)
            {
                reg_types.insert(reg.0, ty);
                true
            } else {
                false
            }
        };
        for block in blocks {
            for instr in block.instrs.iter() {
                if let MirInstr::TryNext { yielded, .. } = instr {
                    changed |= record(reg_types, yielded, Some(Ty::Bool));
                }
                let derived: Option<(&Reg, Option<Ty>)> = match instr {
                    MirInstr::Const { dest, k } => {
                        let ty = match k {
                            Const::Int(value) => {
                                const_ints.insert(dest.0, *value);
                                Some(Ty::Int)
                            }
                            Const::Float(_) => Some(Ty::Float64),
                            Const::IntLiteral(_) => Some(Ty::IntLiteral),
                            Const::FloatLiteral(_) => Some(Ty::FloatLiteral),
                            Const::Bool(_) => Some(Ty::Bool),
                            Const::Str(_) => Some(Ty::String),
                            Const::None => Some(Ty::None),
                            // A callable constant's type needs the checked
                            // expression; report rather than reconstruct.
                            Const::Function(_) => None,
                        };
                        Some((dest, ty))
                    }
                    MirInstr::MaterializeLiteral { dest, target, .. } => {
                        Some((dest, Some(target.clone())))
                    }
                    MirInstr::UseVar { dest, var, .. } => {
                        Some((dest, var_tys.get(var).cloned().map(deref)))
                    }
                    MirInstr::DefVar {
                        var,
                        src,
                        binding_ty,
                    } => {
                        // A synthetic binding (iterator, short-circuit carrier)
                        // has no checked binding type; its slot type is the
                        // initializing register's.
                        if !var_tys.contains_key(var)
                            && let Some(ty) = binding_ty
                                .clone()
                                .or_else(|| reg_types.get(&src.0).cloned())
                        {
                            var_tys.insert(*var, ty);
                            changed = true;
                        }
                        None
                    }
                    MirInstr::LoadPlace { dest, place } => {
                        Some((dest, place.ty.clone().map(deref)))
                    }
                    MirInstr::MovePlace { dest, place } => Some((dest, place.ty.clone())),
                    MirInstr::MakeRef { dest, place } => {
                        Some((dest, mir_place_handle_ty(place, None)))
                    }
                    MirInstr::ReadRef { dest, reference } => Some((
                        dest,
                        reg_types.get(&reference.0).cloned().map(|ty| match ty {
                            Ty::Ref(reference) => *reference.referent,
                            Ty::Pointer { element, .. } => *element,
                            other => other,
                        }),
                    )),
                    MirInstr::CopyValue { dest, value } => {
                        Some((dest, reg_types.get(&value.0).cloned()))
                    }
                    MirInstr::UnOp { op, dest, a } => Some((
                        dest,
                        match op {
                            PrefixOp::Not => Some(Ty::Bool),
                            _ => reg_types.get(&a.0).cloned(),
                        },
                    )),
                    MirInstr::BinOp { op, dest, a, b, .. } => Some((
                        dest,
                        match op {
                            InfixOp::Eq
                            | InfixOp::Ne
                            | InfixOp::Lt
                            | InfixOp::Gt
                            | InfixOp::Le
                            | InfixOp::Ge
                            | InfixOp::And
                            | InfixOp::Or
                            | InfixOp::In
                            | InfixOp::NotIn => Some(Ty::Bool),
                            _ => reg_types.get(&a.0).or_else(|| reg_types.get(&b.0)).cloned(),
                        },
                    )),
                    MirInstr::GetField { dest, base, field } => Some((
                        dest,
                        reg_types
                            .get(&base.0)
                            .and_then(|base| struct_field(declarations, base, field))
                            .map(deref),
                    )),
                    MirInstr::Index {
                        dest, base, index, ..
                    } => Some((
                        dest,
                        reg_types.get(&base.0).and_then(|base| match base {
                            Ty::Pointer { element, .. } => Some((**element).clone()),
                            Ty::Simd { dtype, .. } => Some(Ty::Simd {
                                dtype: *dtype,
                                width: 1,
                            }),
                            // A tuple element type needs the compile-time
                            // index the checker already validated.
                            Ty::Tuple(elements) => const_ints
                                .get(&index.0)
                                .and_then(|value| usize::try_from(*value).ok())
                                .and_then(|value| elements.get(value).cloned())
                                .map(deref),
                            _ => None,
                        }),
                    )),
                    MirInstr::Call {
                        dest,
                        func: FuncRef(callee),
                        ..
                    } => Some((dest, declared_return(declarations, callee))),
                    MirInstr::MethodCall {
                        dest,
                        resolved: Some(callee),
                        ..
                    } => Some((dest, declared_return(declarations, callee))),
                    MirInstr::CallIndirect { dest, callee, .. } => Some((
                        dest,
                        reg_types
                            .get(&callee.0)
                            .and_then(crate::checker::callable_contract_ty)
                            .and_then(|ty| match ty {
                                Ty::Func { ret, .. } => Some((**ret).clone()),
                                _ => None,
                            }),
                    )),
                    MirInstr::MakeTuple {
                        dest,
                        element_types: Some(elements),
                        ..
                    } => Some((dest, Some(Ty::Tuple(elements.clone())))),
                    MirInstr::MakeTuple {
                        dest,
                        elems,
                        element_types: None,
                    } => {
                        // A reference-carrying tuple has no inline element
                        // types; its shape is its element registers' types.
                        let elements: Option<Vec<Ty>> = elems
                            .iter()
                            .map(|element| reg_types.get(&element.0).cloned())
                            .collect();
                        Some((dest, elements.map(Ty::Tuple)))
                    }
                    MirInstr::MakeVariant {
                        dest, alternatives, ..
                    } => Some((dest, Some(Ty::Variant(alternatives.clone())))),
                    MirInstr::MakeSimd {
                        dest, dtype, width, ..
                    } => Some((
                        dest,
                        Some(Ty::Simd {
                            dtype: *dtype,
                            width: *width as i64,
                        }),
                    )),
                    MirInstr::VariantIs { dest, .. } => Some((dest, Some(Ty::Bool))),
                    MirInstr::VariantGet {
                        dest,
                        variant,
                        index,
                    }
                    | MirInstr::VariantTake {
                        dest,
                        variant,
                        index,
                        ..
                    } => Some((
                        dest,
                        reg_types.get(&variant.0).and_then(|ty| match ty {
                            Ty::Variant(alternatives) => alternatives.get(*index).cloned(),
                            _ => None,
                        }),
                    )),
                    MirInstr::VariantReplace {
                        dest,
                        place,
                        output_index,
                        ..
                    } => Some((
                        dest,
                        place.ty.as_ref().and_then(|ty| match ty {
                            Ty::Variant(alternatives) => alternatives.get(*output_index).cloned(),
                            _ => None,
                        }),
                    )),
                    MirInstr::HasNext { dest, .. } => Some((dest, Some(Ty::Bool))),
                    MirInstr::Next { dest, iter, .. } | MirInstr::TryNext { dest, iter, .. } => {
                        // The element register's type is the loop variable's
                        // checked binding type (its definition may sit in the
                        // loop's body block), or falls back to the iterator
                        // slot's element type.
                        let consumer = blocks
                            .iter()
                            .flat_map(|candidate| candidate.instrs.iter())
                            .find_map(|candidate| match candidate {
                                MirInstr::DefVar {
                                    src,
                                    binding_ty: Some(ty),
                                    ..
                                } if src == dest => Some(ty.clone()),
                                MirInstr::DefVar { src, var, .. } if src == dest => {
                                    var_tys.get(var).cloned()
                                }
                                MirInstr::Store { place, src } if src == dest => place.ty.clone(),
                                _ => None,
                            })
                            .or_else(|| {
                                var_tys.get(iter).and_then(|ty| match ty {
                                    Ty::Struct(name, _) => {
                                        declared_return(declarations, &format!("{name}.__next__"))
                                    }
                                    _ => None,
                                })
                            });
                        Some((dest, consumer))
                    }
                    MirInstr::EstablishLoans { marker, .. }
                    | MirInstr::InvalidateInteriors { marker, .. }
                    | MirInstr::ConsumePlace { marker, .. } => Some((marker, Some(Ty::None))),
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        for region in std::iter::once(body)
                            .chain(handler.iter().map(|(_, blocks)| blocks))
                            .chain(orelse.iter())
                            .chain(finalbody.iter())
                        {
                            changed |= close_blocks(region, reg_types, var_tys, declarations);
                        }
                        None
                    }
                    _ => None,
                };
                if let Some((dest, ty)) = derived {
                    changed |= record(reg_types, dest, ty);
                }
            }
        }
        changed
    }

    fn describe_defining_instr(blocks: &[MirBlock], reg: u32, out: &mut Option<String>) {
        for block in blocks {
            for instr in &block.instrs {
                let mut regs = Vec::new();
                verify::instruction_result_regs(instr, &mut regs);
                if regs.iter().any(|candidate| candidate.0 == reg) {
                    let debug = format!("{instr:?}");
                    *out = Some(debug.chars().take(80).collect());
                    return;
                }
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instr
                {
                    for region in std::iter::once(body)
                        .chain(handler.iter().map(|(_, blocks)| blocks))
                        .chain(orelse.iter())
                        .chain(finalbody.iter())
                    {
                        describe_defining_instr(region, reg, out);
                        if out.is_some() {
                            return;
                        }
                    }
                }
            }
        }
    }

    let mut reg_types = std::mem::take(&mut f.reg_types);
    let mut var_tys = std::mem::take(&mut f.var_tys);
    // Chains (a handle read of a fresh handle, a call through a just-typed
    // callable) settle in a few passes; the bound keeps lowering total.
    for _ in 0..4 {
        if !close_blocks(&f.blocks, &mut reg_types, &mut var_tys, declarations) {
            break;
        }
    }
    for reg in 0..f.n_regs {
        if !reg_types.contains_key(&reg) {
            let mut producer = None;
            describe_defining_instr(&f.blocks, reg, &mut producer);
            invariant_errors.push(format!(
                "fn '{name}': register r{reg} has no checked type ({})",
                producer.as_deref().unwrap_or("no defining instruction")
            ));
        }
    }
    f.reg_types = reg_types;
    f.var_tys = var_tys;
}

fn checked_type_or_record(
    checked: &CheckedProgram,
    site: AnnotationSite,
    description: &str,
    invariant_errors: &mut Vec<String>,
) -> Ty {
    match checked.checked_type_at(&site) {
        Some(ty) => ty.clone(),
        None => {
            invariant_errors.push(format!("missing checked type for {description}"));
            Ty::None
        }
    }
}

#[derive(Debug, Default)]
pub struct MirDeclarations {
    pub structs: Vec<MirStructDeclaration>,
    pub functions: Vec<MirFunctionDeclaration>,
}

#[derive(Debug)]
pub struct MirStructDeclaration {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    pub mut_self_methods: HashSet<String>,
    pub fieldwise_init: bool,
    pub param_decls: Vec<ParamDecl>,
    pub explicit_destroy_message: Option<String>,
    pub explicit_destructors: HashMap<String, bool>,
}

#[derive(Debug)]
pub struct MirFunctionDeclaration {
    pub lowered_name: String,
    pub param_names: Vec<String>,
    pub param_types: Vec<Ty>,
    pub defaults: Vec<Option<CheckedConst>>,
    pub required: Vec<bool>,
    pub variadic: Option<Ty>,
    pub variadic_index: Option<usize>,
    pub kw_variadic: Option<Ty>,
    pub kw_variadic_index: Option<usize>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub param_decls: Vec<ParamDecl>,
    /// Whether this declaration has an implicit method receiver. This keeps a
    /// plain `self` (whose convention is `None`) distinct from a static/free
    /// callable with no receiver.
    pub has_receiver: bool,
    /// Declared convention of that implicit method receiver.
    pub receiver_convention: Option<ArgConvention>,
    /// Declared conventions of the explicit runtime parameters, aligned with
    /// `param_types`. Effective per-call `ref` access may narrow to `Read`.
    pub param_conventions: Vec<Option<ArgConvention>>,
    /// Checked return type of the callable.
    pub ret_ty: Ty,
    /// Whether the callable returns a reference handle to `ret_ty` rather than
    /// an owned value. The selected call carries the instantiated origin and
    /// mutability; this declaration fact verifies the ABI family.
    pub returns_reference: bool,
    /// Checked raising contract and its declared error type.
    pub raises: bool,
    pub error_ty: Option<Ty>,
    /// Whether each runtime parameter is a `mut`/`ref` reference whose final
    /// value writes back through a caller place. Same order as `param_types`.
    pub ref_params: Vec<bool>,
}

/// Translate a source parameter marker into the runtime frame layout. Named
/// `out` results are callee-local slots, so they do not consume an incoming
/// argument position; variadic collectors do consume one frame position.
fn runtime_parameter_index(params: &[FnParam], marker: Option<usize>) -> Option<usize> {
    marker.map(|index| {
        params[..index]
            .iter()
            .filter(|parameter| {
                !matches!(parameter.convention, Some(ArgConvention::Out))
                    && parameter.kind != ParamKind::KwVariadic
            })
            .count()
    })
}

/// `*args` is inserted among regular incoming arguments before the collector is
/// materialized, so only preceding non-`out` regular parameters determine its
/// insertion point.
fn runtime_variadic_index(params: &[FnParam], marker: Option<usize>) -> Option<usize> {
    marker.map(|index| {
        params[..index]
            .iter()
            .filter(|parameter| {
                parameter.kind == ParamKind::Regular
                    && !matches!(parameter.convention, Some(ArgConvention::Out))
            })
            .count()
    })
}

/// Translate a declaration-side parameter type into the type of the single
/// runtime slot visible in the function body. Variadic element types are ABI
/// descriptors: the call matcher uses them per supplied argument, then binds a
/// single aggregate in the callee. Positional packs use private pack storage;
/// keyword packs use the public owning `StringDict`. Keeping the positional
/// discriminator out of `MirFunction` prevents ordinary tuple operations,
/// ownership, and place typing from having to understand an artificial
/// source-inexpressible aggregate kind.
fn body_parameter_ty(parameter: &FnParam, ty: Ty) -> Ty {
    match (parameter.kind, ty) {
        (ParamKind::Variadic, Ty::RuntimePack(elements)) => Ty::Tuple(elements),
        (ParamKind::Variadic, element) => Ty::VariadicPack(Box::new(element)),
        (ParamKind::KwVariadic, element) => Ty::Struct(
            "StringDict".to_string(),
            vec![crate::types::TyArg::Ty(element)],
        ),
        (_, ty) => ty,
    }
}

/// Compile-time value parameters have runtime storage in the callee frame even
/// though they are not part of the ordinary argument ABI. The VM reifies each
/// supplied parameter-argument register into these named slots before execution.
fn value_parameter_locals(decls: &[ParamDecl]) -> Vec<(String, Ty)> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name, ty, variadic, ..
            } if matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) => Some((
                name.trim_start_matches('*').to_string(),
                if *variadic {
                    Ty::VariadicPack(ty.clone())
                } else {
                    (**ty).clone()
                },
            )),
            ParamDecl::Type { .. } | ParamDecl::Value { .. } => None,
        })
        .collect()
}

mod nested;
use nested::*;

/// Lower a whole program (a top-level statement list) into per-function MIR.
///
/// Decision — **declarations are handled here, not inside a function body**: each
/// top-level `def` becomes its own `MirFunction`; each `struct` method becomes
/// `Struct.method`; a `trait`'s bodiless requirements produce nothing (default
/// methods are deferred). Remaining statements form `__toplevel__`; production
/// compilation has already rejected executable file-scope source statements.
/// (Nested `def`s inside a body are still deferred — see `lower_stmt`.)
pub fn lower_program(program: &[Stmt]) -> Result<MirProgram, crate::error::TypeError> {
    let checked = crate::checker::check_program(program)?;
    Ok(lower_checked_program(&checked))
}

pub fn lower_checked_program(checked: &CheckedProgram) -> MirProgram {
    let program = checked.statements();
    let mut functions = Vec::new();
    let mut declarations = MirDeclarations::default();
    let mut invariant_errors = Vec::new();
    let mut toplevel: Vec<Stmt> = Vec::new();
    let overloads = crate::symbol::OverloadSets::scan(program);

    for s in program {
        match &s.kind {
            StmtKind::Def {
                name,
                type_params,
                params,
                positional_only,
                keyword_only,
                body,
                ..
            } => {
                let named_result = params
                    .iter()
                    .find(|p| matches!(p.convention, Some(ArgConvention::Out)));
                // ABI parameters lead the variable table; the named result is a
                // callee-local uninitialized slot and is never passed by callers.
                let caller_params: Vec<_> = params
                    .iter()
                    .filter(|p| !matches!(p.convention, Some(ArgConvention::Out)))
                    .collect();
                let mut names: Vec<String> = caller_params.iter().map(|p| p.name.clone()).collect();
                if let Some(result) = named_result {
                    names.push(result.name.clone());
                }
                let generic_site = GenericSite::Function {
                    module: s.module.clone(),
                    declaration: s.span,
                    syntax: s.syntax_id,
                };
                let param_decls = checked
                    .generic_parameters_at(&generic_site)
                    .unwrap_or(&[])
                    .to_vec();
                let value_parameter_locals = value_parameter_locals(&param_decls);
                names.extend(value_parameter_locals.iter().map(|(name, _)| name.clone()));
                let ptys = caller_params
                    .iter()
                    .map(|p| {
                        let param = params
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, *p))
                            .expect("caller parameter belongs to declaration");
                        body_parameter_ty(
                            p,
                            checked_type_or_record(
                                checked,
                                AnnotationSite::FunctionParam {
                                    module: s.module.clone(),
                                    declaration: s.span,
                                    syntax: s.syntax_id,
                                    param,
                                },
                                &format!("parameter '{}' of function '{name}'", p.name),
                                &mut invariant_errors,
                            ),
                        )
                    })
                    .collect();
                let owned = caller_params
                    .iter()
                    .map(|p| is_owned(&p.convention))
                    .collect();
                let deinit = caller_params
                    .iter()
                    .map(|p| is_deinit(&p.convention))
                    .collect();
                let refp = caller_params
                    .iter()
                    .map(|p| is_ref(&p.convention))
                    .collect();
                let lowered_name =
                    crate::symbol::lowered_def_name(name, type_params, params, &overloads);
                let variadic_idx = params.iter().position(|p| p.kind == ParamKind::Variadic);
                let kw_variadic_idx = params.iter().position(|p| p.kind == ParamKind::KwVariadic);
                let regular: Vec<_> = params
                    .iter()
                    .filter(|p| {
                        p.kind == ParamKind::Regular
                            && !matches!(p.convention, Some(ArgConvention::Out))
                    })
                    .collect();
                let return_site = AnnotationSite::FunctionReturn {
                    module: s.module.clone(),
                    declaration: s.span,
                    syntax: s.syntax_id,
                };
                let ret_ty = checked_type_or_record(
                    checked,
                    return_site.clone(),
                    &format!("return type of function '{name}'"),
                    &mut invariant_errors,
                );
                let effect = checked
                    .declaration_effect_at(&return_site)
                    .cloned()
                    .unwrap_or_else(|| {
                        invariant_errors
                            .push(format!("missing checked effect for function '{name}'"));
                        crate::checked::DeclarationEffect {
                            raises: false,
                            error: None,
                            returns_reference: false,
                        }
                    });
                declarations.functions.push(MirFunctionDeclaration {
                    lowered_name: lowered_name.clone(),
                    param_names: regular.iter().map(|p| p.name.clone()).collect(),
                    param_types: regular
                        .iter()
                        .map(|p| {
                            checked_type_or_record(
                                checked,
                                AnnotationSite::FunctionParam {
                                    module: s.module.clone(),
                                    declaration: s.span,
                                    syntax: s.syntax_id,
                                    param: params
                                        .iter()
                                        .position(|candidate| std::ptr::eq(candidate, *p))
                                        .unwrap_or(params.len()),
                                },
                                &format!("parameter '{}' of function '{name}'", p.name),
                                &mut invariant_errors,
                            )
                        })
                        .collect(),
                    defaults: regular
                        .iter()
                        .map(|p| p.default.as_ref().and_then(CheckedConst::from_expr))
                        .collect(),
                    required: regular.iter().map(|p| p.default.is_none()).collect(),
                    variadic: variadic_idx.map(|i| {
                        checked_type_or_record(
                            checked,
                            AnnotationSite::FunctionParam {
                                module: s.module.clone(),
                                declaration: s.span,
                                syntax: s.syntax_id,
                                param: i,
                            },
                            &format!("variadic parameter of function '{name}'"),
                            &mut invariant_errors,
                        )
                    }),
                    variadic_index: runtime_variadic_index(params, variadic_idx),
                    kw_variadic: kw_variadic_idx.map(|i| {
                        checked_type_or_record(
                            checked,
                            AnnotationSite::FunctionParam {
                                module: s.module.clone(),
                                declaration: s.span,
                                syntax: s.syntax_id,
                                param: i,
                            },
                            &format!("keyword variadic parameter of function '{name}'"),
                            &mut invariant_errors,
                        )
                    }),
                    kw_variadic_index: runtime_parameter_index(params, kw_variadic_idx),
                    positional_only: regular_marker_index(params, *positional_only),
                    keyword_only: effective_keyword_only_index(params, *keyword_only, variadic_idx),
                    param_decls,
                    has_receiver: false,
                    receiver_convention: None,
                    param_conventions: regular.iter().map(|p| p.convention).collect(),
                    ret_ty: ret_ty.clone(),
                    returns_reference: effect.returns_reference,
                    raises: effect.raises,
                    error_ty: effect.error.clone(),
                    ref_params: regular.iter().map(|p| is_ref(&p.convention)).collect(),
                });
                lower_fn_nested(
                    FunctionLowering {
                        checked,
                        name: &lowered_name,
                        parameter_names: &names,
                        parameter_types: ptys,
                        value_parameter_locals,
                        owned_parameters: owned,
                        deinit_parameters: deinit,
                        reference_parameters: refp,
                        returns_reference: effect.returns_reference,
                        ret_ty,
                        raises: effect.raises,
                        error_ty: effect.error,
                        named_result: named_result.map(|p| p.name.as_str()),
                        body,
                        overloads: &overloads,
                    },
                    &mut functions,
                    &mut declarations,
                );
            }
            StmtKind::Struct {
                name,
                type_params,
                fields,
                methods,
                fieldwise_init,
                ..
            } => {
                let mut_self_methods = methods
                    .iter()
                    .filter(|m| {
                        matches!(
                            m.self_convention,
                            Some(ArgConvention::Mut | ArgConvention::Ref)
                        )
                    })
                    .map(|m| {
                        let method_name = crate::symbol::lifecycle_method_name(m);
                        let source = format!("{name}.{method_name}");
                        let lowered = crate::symbol::lowered_method_name(
                            &source,
                            type_params,
                            &m.params,
                            m.self_convention,
                            &overloads,
                        );
                        if lowered == source {
                            method_name.to_string()
                        } else {
                            lowered
                        }
                    })
                    .collect();
                declarations.structs.push(MirStructDeclaration {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .enumerate()
                        .map(|(field_index, field)| {
                            (
                                field.name.clone(),
                                checked_type_or_record(
                                    checked,
                                    AnnotationSite::StructField {
                                        module: s.module.clone(),
                                        declaration: name.clone(),
                                        field: field_index,
                                    },
                                    &format!("field '{}' of struct '{name}'", field.name),
                                    &mut invariant_errors,
                                ),
                            )
                        })
                        .collect(),
                    mut_self_methods,
                    fieldwise_init: *fieldwise_init,
                    param_decls: checked
                        .generic_parameters_at(&GenericSite::Struct {
                            module: s.module.clone(),
                            declaration: name.clone(),
                        })
                        .unwrap_or(&[])
                        .to_vec(),
                    explicit_destroy_message: checked
                        .explicit_destroy_types()
                        .get(name)
                        .map(|info| info.message.clone()),
                    explicit_destructors: checked
                        .explicit_destroy_types()
                        .get(name)
                        .map(|info| info.destructors.clone())
                        .unwrap_or_default(),
                });
                for (method_index, m) in methods.iter().enumerate() {
                    let method_name = crate::symbol::lifecycle_method_name(m);
                    let source_mangled = format!("{name}.{method_name}");
                    let mangled = crate::symbol::lowered_method_name(
                        &source_mangled,
                        type_params,
                        &m.params,
                        m.self_convention,
                        &overloads,
                    );
                    let variadic_idx = m
                        .params
                        .iter()
                        .position(|param| param.kind == ParamKind::Variadic);
                    let kw_variadic_idx = m
                        .params
                        .iter()
                        .position(|param| param.kind == ParamKind::KwVariadic);
                    let regular: Vec<_> = m
                        .params
                        .iter()
                        .filter(|param| {
                            param.kind == ParamKind::Regular
                                && !matches!(param.convention, Some(ArgConvention::Out))
                        })
                        .collect();
                    let return_site = AnnotationSite::MethodReturn {
                        module: s.module.clone(),
                        declaration: name.clone(),
                        method: method_index,
                    };
                    let ret_ty = checked_type_or_record(
                        checked,
                        return_site.clone(),
                        &format!("return type of method '{source_mangled}'"),
                        &mut invariant_errors,
                    );
                    let effect = checked
                        .declaration_effect_at(&return_site)
                        .cloned()
                        .unwrap_or_else(|| {
                            invariant_errors.push(format!(
                                "missing checked effect for method '{source_mangled}'"
                            ));
                            crate::checked::DeclarationEffect {
                                raises: false,
                                error: None,
                                returns_reference: false,
                            }
                        });
                    let generic_site = GenericSite::Method {
                        module: s.module.clone(),
                        declaration: name.clone(),
                        method: method_index,
                    };
                    let param_decls = checked
                        .generic_parameters_at(&generic_site)
                        .unwrap_or(&[])
                        .to_vec();
                    let value_parameter_locals = value_parameter_locals(&param_decls);
                    declarations.functions.push(MirFunctionDeclaration {
                        lowered_name: mangled.clone(),
                        param_names: regular.iter().map(|param| param.name.clone()).collect(),
                        param_types: regular
                            .iter()
                            .map(|param| {
                                checked_type_or_record(
                                    checked,
                                    AnnotationSite::MethodParam {
                                        module: s.module.clone(),
                                        declaration: name.clone(),
                                        method: method_index,
                                        param: m
                                            .params
                                            .iter()
                                            .position(|candidate| std::ptr::eq(candidate, *param))
                                            .unwrap_or(m.params.len()),
                                    },
                                    &format!(
                                        "parameter '{}' of method '{source_mangled}'",
                                        param.name
                                    ),
                                    &mut invariant_errors,
                                )
                            })
                            .collect(),
                        defaults: regular
                            .iter()
                            .map(|param| param.default.as_ref().and_then(CheckedConst::from_expr))
                            .collect(),
                        required: regular
                            .iter()
                            .map(|param| param.default.is_none())
                            .collect(),
                        variadic: variadic_idx.map(|index| {
                            checked_type_or_record(
                                checked,
                                AnnotationSite::MethodParam {
                                    module: s.module.clone(),
                                    declaration: name.clone(),
                                    method: method_index,
                                    param: index,
                                },
                                &format!("variadic parameter of method '{source_mangled}'"),
                                &mut invariant_errors,
                            )
                        }),
                        variadic_index: runtime_variadic_index(&m.params, variadic_idx),
                        kw_variadic: kw_variadic_idx.map(|index| {
                            checked_type_or_record(
                                checked,
                                AnnotationSite::MethodParam {
                                    module: s.module.clone(),
                                    declaration: name.clone(),
                                    method: method_index,
                                    param: index,
                                },
                                &format!("keyword variadic parameter of method '{source_mangled}'"),
                                &mut invariant_errors,
                            )
                        }),
                        kw_variadic_index: runtime_parameter_index(&m.params, kw_variadic_idx),
                        positional_only: regular_marker_index(&m.params, m.positional_only),
                        keyword_only: effective_keyword_only_index(
                            &m.params,
                            m.keyword_only,
                            variadic_idx,
                        ),
                        param_decls,
                        has_receiver: m.has_self,
                        receiver_convention: m.self_convention,
                        param_conventions: regular
                            .iter()
                            .map(|parameter| parameter.convention)
                            .collect(),
                        ret_ty: ret_ty.clone(),
                        returns_reference: effect.returns_reference,
                        raises: effect.raises,
                        error_ty: effect.error.clone(),
                        ref_params: regular
                            .iter()
                            .map(|param| is_ref(&param.convention))
                            .collect(),
                    });
                    // A method's receiver `self` is the implicit first parameter,
                    // followed by the declared params.
                    let mut names: Vec<String> = Vec::new();
                    let mut ptys: Vec<Ty> = Vec::new();
                    let mut owned: Vec<bool> = Vec::new();
                    let mut deinit: Vec<bool> = Vec::new();
                    let mut refp: Vec<bool> = Vec::new();
                    if m.has_self {
                        names.push("self".to_string());
                        ptys.push(Ty::Struct(name.clone(), Vec::new()));
                        owned.push(is_owned(&m.self_convention));
                        deinit.push(is_deinit(&m.self_convention));
                        refp.push(is_ref(&m.self_convention));
                    }
                    names.extend(m.params.iter().map(|p| p.name.clone()));
                    names.extend(value_parameter_locals.iter().map(|(name, _)| name.clone()));
                    ptys.extend(m.params.iter().enumerate().map(|(param, p)| {
                        body_parameter_ty(
                            p,
                            checked_type_or_record(
                                checked,
                                AnnotationSite::MethodParam {
                                    module: s.module.clone(),
                                    declaration: name.clone(),
                                    method: method_index,
                                    param,
                                },
                                &format!("parameter '{}' of method '{source_mangled}'", p.name),
                                &mut invariant_errors,
                            ),
                        )
                    }));
                    owned.extend(m.params.iter().map(|p| is_owned(&p.convention)));
                    deinit.extend(m.params.iter().map(|p| is_deinit(&p.convention)));
                    refp.extend(m.params.iter().map(|p| is_ref(&p.convention)));
                    lower_fn_nested(
                        FunctionLowering {
                            checked,
                            name: &mangled,
                            parameter_names: &names,
                            parameter_types: ptys,
                            value_parameter_locals,
                            owned_parameters: owned,
                            deinit_parameters: deinit,
                            reference_parameters: refp,
                            returns_reference: effect.returns_reference,
                            ret_ty,
                            raises: effect.raises,
                            error_ty: effect.error,
                            named_result: None,
                            body: &m.body,
                            overloads: &overloads,
                        },
                        &mut functions,
                        &mut declarations,
                    );
                }
            }
            // A `trait`'s requirements have no body (`...`); nothing to lower yet.
            StmtKind::Trait { .. } => {}
            _ => toplevel.push(s.clone()),
        }
    }

    let mut toplevel_fn = lower_cfg_nested(
        &Cfg::build_checked_fn(checked, &[], &toplevel),
        &HashMap::new(),
        &overloads,
        false,
        &[],
        &[],
    );
    // The synthetic module initializer returns nothing and never raises.
    toplevel_fn.ret_ty = Some(Ty::None);
    functions.push(("__toplevel__".to_string(), toplevel_fn));
    for (name, function) in &mut functions {
        close_register_types(name, function, &declarations, &mut invariant_errors);
    }
    let mut result = MirProgram {
        functions,
        declarations,
        invariant_errors,
    };
    result.invariant_errors.extend(verify::verify(&result));
    result
}
