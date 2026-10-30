//! Declaration collection, signature construction, and body-checking support.

use super::decorators::DecoratorSite;
use super::patterns::StmtReach;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::DeclaredParam;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};

/// A method body as its check binds and walks it, whether the method was
/// spelled or is a struct's wrapper of a trait default it inherits.
pub(super) struct MethodBody<'a> {
    pub(super) name: &'a str,
    /// Every declared parameter but the receiver, in order.
    pub(super) params: Vec<BodyParam<'a>>,
    pub(super) self_writable: bool,
    /// Whether the body initializes its `out self` receiver.
    pub(super) initializing: bool,
    /// The callable identity its transfer effects are committed under.
    pub(super) effect_key: String,
    pub(super) method_decls: &'a [ParamDecl],
    pub(super) ref_return: Option<mojito_types::origin::RefSig>,
    /// The return annotation a `return` re-resolves
    /// ([`Checker::body_return_annotation`]).
    pub(super) return_annotation: Option<(SourceType, bool)>,
    /// Whether the method declares compile-time parameters of its own.
    pub(super) generic: bool,
    pub(super) ret_ty: &'a Ty,
    pub(super) body: &'a [Stmt],
}

/// A method parameter as its body binds it.
#[derive(Clone, Copy)]
pub(super) struct BodyParam<'a> {
    pub(super) name: &'a str,
    pub(super) kind: mojito_ast::ast::ParamKind,
    pub(super) convention: Option<ArgConvention>,
}

impl BodyParam<'_> {
    fn is_named_result(&self) -> bool {
        self.kind == mojito_ast::ast::ParamKind::Regular
            && self.convention == Some(ArgConvention::Out)
    }
}

pub(super) fn definitely_returns(body: &[Stmt], reach: &dyn Fn(&Stmt) -> StmtReach) -> bool {
    body.iter().any(|stmt| stmt_returns(stmt, reach))
}

/// How many compile-time parameters a per-call clone of a declaration bakes,
/// which is what the overload rank charges a candidate for its signature
/// length. A callable-bounded parameter (`F: def() -> Int`) is not one: it
/// may remain on a concrete clone.
pub(super) fn baked_decl_count(decls: &[ParamDecl]) -> usize {
    decls
        .iter()
        .filter(|decl| {
            !matches!(
                decl,
                ParamDecl::Type {
                    callable_bound: Some(_),
                    ..
                }
            )
        })
        .count()
}

/// Whether a declaration bakes no compile-time parameter.
pub(super) fn decls_are_concrete(decls: &[ParamDecl]) -> bool {
    baked_decl_count(decls) == 0
}

/// Solve the type parameters a callable-bounded parameter's contract names
/// from the callable actually bound to it: `[T: AnyType, //, F: def() -> T]`
/// with `init_with: F` binds `F` to the argument's function type, and its
/// declared contract `def() -> T` then unifies against that type to bind
/// `T` (upstream's `Variant(init_with=factory)` inference).
pub(super) fn unify_through_callable_bounds(
    decls: &[ParamDecl],
    subst: &mut TySubst,
) -> Result<(), TypeError> {
    for decl in decls {
        let ParamDecl::Type {
            id,
            callable_bound: Some(bound),
            ..
        } = decl
        else {
            continue;
        };
        let Some(actual) = subst.get(id).cloned() else {
            continue;
        };
        // The contract's environment class is validated separately
        // (`validate_callable_parameter_bounds`); solving reads only the
        // parameter and result shapes.
        if let (
            Some(FuncTy {
                params: bound_params,
                ret: bound_ret,
                error: bound_error,
                ..
            }),
            Some(FuncTy {
                params: actual_params,
                ret: actual_ret,
                error: actual_error,
                ..
            }),
        ) = (bound.as_func(), actual.as_func())
        {
            for (pattern, actual) in bound_params.iter().zip(actual_params) {
                unify(pattern, actual, subst)?;
            }
            unify(bound_ret, actual_ret, subst)?;
            if let (Some(pattern), Some(actual)) = (bound_error, actual_error) {
                unify(pattern, actual, subst)?;
            }
        }
    }
    Ok(())
}

/// Whether `stmt` never completes normally. `reach` gives a `match`
/// statement's coverage facts: whether its cases cover every value of its
/// subject, and which cases earlier ones already cover; a case the subject
/// cannot miss concludes it regardless.
pub(super) fn stmt_returns(stmt: &Stmt, reach: &dyn Fn(&Stmt) -> StmtReach) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) => true,
        // A `raise` diverges (it never falls through to the end), so for
        // reachability it behaves like a `return`.
        StmtKind::Raise(_) => true,
        // The runtime trap never returns, so a body ending in it has no
        // fall-through path either (the shape of an unspecialized generic
        // template method's stub body).
        StmtKind::Expr(Expr {
            kind: ExprKind::Call { name, .. },
            ..
        }) if is_diverging_intrinsic(name) => true,
        // A `comptime assert` whose condition folded to `False` fails in
        // every instance that reaches it.
        StmtKind::ComptimeAssert { .. } => reach(stmt).fails,
        // A `comptime if` reaches here only under source validation, where
        // no arm is assumed selected: every arm must return.
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            orelse
                .as_ref()
                .is_some_and(|e| definitely_returns(e, reach))
                && branches.iter().all(|(_, b)| definitely_returns(b, reach))
        }
        // A `match` falls through unless its cases cover the subject or a
        // case the subject cannot miss concludes it; then every case a value
        // reaches must return.
        StmtKind::Match { cases, .. } | StmtKind::ComptimeMatch { cases, .. } => {
            let reached = match_reach(stmt, cases, reach);
            reached.concluded
                && reached
                    .reachable(cases)
                    .all(|case| definitely_returns(&case.body, reach))
        }
        // A `comptime for` reaches here only under source validation. How
        // often it unrolls, and which of its `comptime if` arms survive, is
        // each instantiation's fact, so a loop holding a `return` leaves the
        // fall-through verdict to the executable check of the unrolled clone.
        StmtKind::ComptimeFor { body, .. } => block_holds_return(body),
        // A `with` body runs exactly once (its desugar is a `try`/`finally`
        // whose cleanup cannot divert control), so it diverges iff the body does.
        StmtKind::With { body, .. } | StmtKind::Scope(body) => definitely_returns(body, reach),
        // A `try` definitely diverges when: a `finally` does (it overrides every
        // path); or the **normal-completion** path diverges (the body — or, if the
        // body may complete, the `else`) *and* the **exceptional** path does (every
        // `except` handler diverges; with no handler, an uncaught raise itself
        // exits, so only the normal path can fall through).
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            if finalbody
                .as_ref()
                .is_some_and(|fb| definitely_returns(fb, reach))
            {
                return true;
            }
            let normal = match orelse {
                Some(else_) => definitely_returns(body, reach) || definitely_returns(else_, reach),
                None => definitely_returns(body, reach),
            };
            let exceptional = match except {
                Some((_, handler)) => definitely_returns(handler, reach),
                None => true,
            };
            normal && exceptional
        }
        _ => false,
    }
}

/// The coverage facts of the `match` statement `stmt` with `cases`: it
/// never falls through when the check proved its cases cover the subject,
/// or when an unguarded case the subject cannot miss concludes it.
pub(super) fn match_reach(
    stmt: &Stmt,
    cases: &[mojito_ast::ast::MatchCase],
    reach: &dyn Fn(&Stmt) -> StmtReach,
) -> StmtReach {
    let mut reached = reach(stmt);
    reached.concluded |= cases
        .iter()
        .any(|case| case.guard.is_none() && super::patterns::irrefutable(&case.pattern));
    reached
}

/// Conservative definite-initialization check for a named `out` result. A
/// value-returning path supplies the result directly; a fallthrough or bare
/// return path must assign the named result first.
pub(super) fn definitely_initializes_named_result(
    body: &[Stmt],
    name: &str,
    reach: &dyn Fn(&Stmt) -> StmtReach,
) -> bool {
    let mut initialized = false;
    for stmt in body {
        match &stmt.kind {
            StmtKind::Assign { name: target, .. } if target == name => {
                initialized = true;
            }
            // `__mlir_op.`lit.ownership.mark_initialized`(
            // __get_mvalue_as_litref(result))`: the result is defined from
            // here, its storage written through pointers.
            StmtKind::Expr(expr)
                if marked_place(expr).is_some_and(
                    |place| matches!(&place.kind, ExprKind::Identifier(marked) if marked == name),
                ) =>
            {
                initialized = true;
            }
            StmtKind::Return(Some(_)) | StmtKind::Raise(_) => return true,
            // The runtime trap of an unspecialized template's stub never
            // completes, so it produces no result to initialize.
            StmtKind::Expr(Expr {
                kind: ExprKind::Call { name, .. },
                ..
            }) if is_diverging_intrinsic(name) => return true,
            StmtKind::ComptimeAssert { .. } if reach(stmt).fails => return true,
            StmtKind::Return(None) => return initialized,
            StmtKind::Scope(body) => {
                initialized |= definitely_initializes_named_result(body, name, reach);
            }
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                let Some(orelse) = orelse else { continue };
                if branches
                    .iter()
                    .all(|(_, branch)| definitely_initializes_named_result(branch, name, reach))
                    && definitely_initializes_named_result(orelse, name, reach)
                {
                    return true;
                }
            }
            StmtKind::Match { cases, .. } | StmtKind::ComptimeMatch { cases, .. } => {
                let reached = match_reach(stmt, cases, reach);
                if reached.concluded
                    && reached
                        .reachable(cases)
                        .all(|case| definitely_initializes_named_result(&case.body, name, reach))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    initialized
}

/// Whether every normally completing path initializes `self.field`. A raised
/// path does not produce a value and therefore need not initialize the field;
/// every explicit return and every fallthrough path does. Loops are treated as
/// possibly executing zero times.
pub(super) fn definitely_initializes_self_field(
    body: &[Stmt],
    field: &str,
    reach: &dyn Fn(&Stmt) -> StmtReach,
) -> bool {
    let flow = init_field_flow(body, field, false, reach);
    flow.valid && flow.normal.is_none_or(|initialized| initialized)
}

/// Whether a parameter annotation is exactly the bare `StringLiteral`, or a
/// specialized `*args: *Ts` pack (`$pack[...]`, or the `__RuntimeTuple[...]`
/// a forwarded nested pack is stamped with) whose elements may be that
/// per-call inferred literal type.
pub(super) fn is_string_literal_annotation(annotation: &SourceType) -> bool {
    match annotation {
        SourceType::StringLiteral | SourceType::ClosedStringLiteral => true,
        SourceType::Named(name, args) => {
            matches!(name.as_str(), "$pack" | "__RuntimeTuple")
                || (name == "StringLiteral" && args.is_empty())
        }
        _ => false,
    }
}

/// Whether a parameter annotation is a bare value-parameterized literal type
/// ([`is_string_literal_annotation`], or exactly `IntLiteral`), whose value
/// the parameter infers per call.
pub(super) fn is_literal_annotation(annotation: &SourceType) -> bool {
    is_string_literal_annotation(annotation) || is_int_literal_annotation(annotation)
}

/// Whether a parameter annotation is exactly the bare `IntLiteral`.
pub(super) fn is_int_literal_annotation(annotation: &SourceType) -> bool {
    matches!(annotation, SourceType::Named(name, args) if name == "IntLiteral" && args.is_empty())
}

/// The collector part of a method signature — its `*args` and `**kwargs`
/// collectors and the keyword-only boundary a `*args` collector sets — built
/// alike for a struct method and a trait requirement, so a witness compares
/// against its requirement field for field.
pub(super) struct Collectors {
    pub(super) variadic: Option<Ty>,
    pub(super) variadic_index: Option<usize>,
    pub(super) variadic_convention: Option<ArgConvention>,
    pub(super) variadic_name: Option<String>,
    pub(super) kw_variadic: Option<Ty>,
    pub(super) kw_variadic_index: Option<usize>,
    pub(super) keyword_only: Option<usize>,
}

impl Collectors {
    /// `keyword_only` is the declared `*` marker; `all_types` the resolved
    /// type of every parameter in `params`.
    pub(super) fn of(params: &[FnParam], keyword_only: Option<usize>, all_types: &[Ty]) -> Self {
        let position = |kind| params.iter().position(|p| p.kind == kind);
        let variadic_idx = position(mojito_ast::ast::ParamKind::Variadic);
        let kw_variadic_idx = position(mojito_ast::ast::ParamKind::KwVariadic);
        Self {
            variadic: variadic_idx.map(|index| all_types[index].clone()),
            variadic_index: regular_marker_index(params, variadic_idx),
            variadic_convention: variadic_idx.and_then(|index| params[index].convention),
            variadic_name: variadic_idx.map(|index| params[index].name.clone()),
            kw_variadic: kw_variadic_idx.map(|index| all_types[index].clone()),
            kw_variadic_index: kw_variadic_idx,
            keyword_only: effective_keyword_only_index(params, keyword_only, variadic_idx),
        }
    }
}

/// Whether the call `name` is a compiler-private intrinsic that never
/// returns: the runtime trap.
fn is_diverging_intrinsic(name: &str) -> bool {
    name == "_mojito_abort"
}

/// Why a construction selected no constructor: the availability clause of a
/// candidate the arguments would have selected, a candidate's failed
/// binding, or a bare miss.
fn constructor_miss(
    name: &str,
    kind: OverloadSelect,
    binding_failure: Option<&TypeError>,
    unavailable: Option<TypeError>,
) -> TypeError {
    const NO_MATCH: &str = "no constructor overload matches the supplied arguments";
    match (kind, binding_failure, unavailable) {
        (OverloadSelect::Ambiguous, ..) => TypeError::BadCall {
            func: name.to_string(),
            reason: "ambiguous overloaded constructor call".to_string(),
        },
        (OverloadSelect::NoMatch, None, Some(unavailable)) => unavailable,
        (OverloadSelect::NoMatch, Some(error), _) => TypeError::BadCall {
            func: name.to_string(),
            reason: format!("{NO_MATCH} ({error})"),
        },
        (OverloadSelect::NoMatch, None, None) => TypeError::BadCall {
            func: name.to_string(),
            reason: NO_MATCH.to_string(),
        },
    }
}

/// The place the `__mlir_op` statement `lit.ownership.mark_initialized` marks
/// (`__get_mvalue_as_litref(place)`), when `expr` is that statement.
fn marked_place(expr: &Expr) -> Option<&Expr> {
    let ExprKind::MethodCall {
        object,
        method,
        args,
        kwargs,
    } = &expr.kind
    else {
        return None;
    };
    if !matches!(&object.kind, ExprKind::Identifier(name) if name == "__mlir_op")
        || method != "lit.ownership.mark_initialized"
        || !kwargs.is_empty()
    {
        return None;
    }
    match args.as_slice() {
        [
            Expr {
                kind: ExprKind::Call { name, args, .. },
                ..
            },
        ] if name == "__get_mvalue_as_litref" => match args.as_slice() {
            [place] => Some(place),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `expr` marks `self.<field>`, or `self` whole, initialized.
fn marks_initialized(expr: &Expr, field: &str) -> bool {
    let is_self = |expr: &Expr| matches!(&expr.kind, ExprKind::Identifier(name) if name == "self");
    marked_place(expr).is_some_and(|place| {
        is_self(place)
            || matches!(&place.kind, ExprKind::Member { object, field: marked }
                if marked == field && is_self(object))
    })
}

#[derive(Clone, Copy)]
struct InitFieldFlow {
    /// Initialization state on normal fallthrough, or `None` when no path falls
    /// through (all paths returned or raised).
    normal: Option<bool>,
    /// Every value-producing exit seen so far was initialized.
    valid: bool,
}

fn init_field_flow(
    body: &[Stmt],
    field: &str,
    mut initialized: bool,
    reach: &dyn Fn(&Stmt) -> StmtReach,
) -> InitFieldFlow {
    let mut valid = true;
    for stmt in body {
        match &stmt.kind {
            StmtKind::SetPlace { place, .. }
                if matches!(
                    &place.kind,
                    ExprKind::Member { object, field: assigned }
                        if assigned == field
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
                ) =>
            {
                initialized = true;
            }
            // `self = value` initializes every field, as upstream's
            // `__init__` may assign its whole `out self`.
            StmtKind::Assign { name, .. } if name == "self" => {
                initialized = true;
            }
            // `__mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(
            // self.field))`, or over `self` whole, initializes the field.
            StmtKind::Expr(expr) if marks_initialized(expr, field) => {
                initialized = true;
            }
            // `return value` initializes the whole `out self` with the value.
            StmtKind::Return(value) => {
                return InitFieldFlow {
                    normal: None,
                    valid: valid && (initialized || value.is_some()),
                };
            }
            StmtKind::Raise(_) => {
                return InitFieldFlow {
                    normal: None,
                    valid,
                };
            }
            StmtKind::ComptimeAssert { .. } if reach(stmt).fails => {
                return InitFieldFlow {
                    normal: None,
                    valid,
                };
            }
            // A kept `comptime if` is checked as its runtime counterpart: the
            // condition is opaque, so every arm must initialize.
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                let mut flows: Vec<_> = branches
                    .iter()
                    .map(|(_, branch)| init_field_flow(branch, field, initialized, reach))
                    .collect();
                flows.push(match orelse {
                    Some(orelse) => init_field_flow(orelse, field, initialized, reach),
                    None => InitFieldFlow {
                        normal: Some(initialized),
                        valid: true,
                    },
                });
                valid &= flows.iter().all(|flow| flow.valid);
                let mut normal_paths = flows.iter().filter_map(|flow| flow.normal);
                initialized = match normal_paths.next() {
                    Some(first) => normal_paths.fold(first, |all, state| all && state),
                    None => {
                        return InitFieldFlow {
                            normal: None,
                            valid,
                        };
                    }
                };
            }
            // A `match` is a branch per case a value reaches, plus the
            // fall-through when no case concludes it.
            StmtKind::Match { cases, .. } | StmtKind::ComptimeMatch { cases, .. } => {
                let reached = match_reach(stmt, cases, reach);
                let mut flows: Vec<_> = reached
                    .reachable(cases)
                    .map(|case| init_field_flow(&case.body, field, initialized, reach))
                    .collect();
                if !reached.concluded {
                    flows.push(InitFieldFlow {
                        normal: Some(initialized),
                        valid: true,
                    });
                }
                valid &= flows.iter().all(|flow| flow.valid);
                let mut normal_paths = flows.iter().filter_map(|flow| flow.normal);
                initialized = match normal_paths.next() {
                    Some(first) => normal_paths.fold(first, |all, state| all && state),
                    None => {
                        return InitFieldFlow {
                            normal: None,
                            valid,
                        };
                    }
                };
            }
            // A loop may execute zero times, so assignments in its body cannot
            // establish initialization after the loop. Returns inside it still
            // have to be safe.
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                valid &= init_field_flow(body, field, initialized, reach).valid;
            }
            // A `with` body, like a kept compile-time block, runs exactly
            // once, straight through.
            StmtKind::With { body, .. } | StmtKind::Scope(body) => {
                let flow = init_field_flow(body, field, initialized, reach);
                valid &= flow.valid;
                match flow.normal {
                    Some(state) => initialized = state,
                    None => {
                        return InitFieldFlow {
                            normal: None,
                            valid,
                        };
                    }
                }
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let body_flow = init_field_flow(body, field, initialized, reach);
                valid &= body_flow.valid;
                let normal_flow = body_flow.normal.map(|state| match orelse {
                    Some(orelse) => init_field_flow(orelse, field, state, reach),
                    None => InitFieldFlow {
                        normal: Some(state),
                        valid: true,
                    },
                });
                let exceptional_flow = except
                    .as_ref()
                    .map(|(_, handler)| init_field_flow(handler, field, initialized, reach));
                valid &= normal_flow.is_none_or(|flow| flow.valid)
                    && exceptional_flow.is_none_or(|flow| flow.valid);

                let mut exits: Vec<bool> = normal_flow
                    .and_then(|flow| flow.normal)
                    .into_iter()
                    .chain(exceptional_flow.and_then(|flow| flow.normal))
                    .collect();
                if except.is_none() && body_flow.normal.is_none() {
                    exits.clear();
                }
                if let Some(finalbody) = finalbody {
                    let starts = if exits.is_empty() {
                        vec![initialized]
                    } else {
                        exits
                    };
                    let final_flows: Vec<_> = starts
                        .into_iter()
                        .map(|state| init_field_flow(finalbody, field, state, reach))
                        .collect();
                    valid &= final_flows.iter().all(|flow| flow.valid);
                    exits = final_flows.iter().filter_map(|flow| flow.normal).collect();
                }
                if exits.is_empty() {
                    return InitFieldFlow {
                        normal: None,
                        valid,
                    };
                }
                initialized = exits.into_iter().all(|state| state);
            }
            _ => {}
        }
    }
    InitFieldFlow {
        normal: Some(initialized),
        valid,
    }
}

/// Method/function signature classification and body checking moved from `checker.rs`.
impl Checker {
    pub(super) fn method_sig(
        &self,
        method: &Method,
        decls: Vec<ParamDecl>,
        all_types: &[Ty],
    ) -> Result<MethodSig, TypeError> {
        let error = self.declared_error(method.raises, method.raises_type.as_ref())?;
        let collectors = Collectors::of(&method.params, method.keyword_only, all_types);
        let regular: Vec<_> = method
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == mojito_ast::ast::ParamKind::Regular && !p.is_named_result())
            .collect();
        let regular_params: Vec<&FnParam> = regular.iter().map(|(_, param)| *param).collect();
        let named_result = mojito_ast::ast::named_result(&method.params);
        Ok(MethodSig {
            availability: method
                .where_clauses
                .iter()
                .map(|condition| {
                    self.compile_where_clause(condition)
                        .map(|constraint| self.bind_constraint(&constraint, &decls))
                })
                .collect::<Result<_, _>>()?,
            decls,
            has_self: method.has_self,
            params: regular
                .iter()
                .map(|(index, _)| all_types[*index].clone())
                .collect(),
            names: regular.iter().map(|(_, p)| p.name.clone()).collect(),
            required: required_mask(
                &regular.iter().map(|(_, p)| *p).collect::<Vec<_>>(),
                collectors.keyword_only,
            )?,
            defaults: regular
                .iter()
                .map(|(_, p)| {
                    p.default.as_ref().map(|default| {
                        mojito_checked::checked::CheckedConst::from_expr(default)
                            .map_or(SignatureDefault::Evaluated, SignatureDefault::Constant)
                    })
                })
                .collect(),
            variadic: collectors.variadic,
            variadic_index: collectors.variadic_index,
            variadic_convention: collectors.variadic_convention,
            variadic_name: collectors.variadic_name,
            kw_variadic: collectors.kw_variadic,
            kw_variadic_index: collectors.kw_variadic_index,
            positional_only: regular_marker_index(&method.params, method.positional_only),
            keyword_only: collectors.keyword_only,
            conventions: regular.iter().map(|(_, p)| p.convention).collect(),
            ret: match &method.ret {
                Some(SourceType::Ref { referent, .. }) => self.ty_from_anno(referent)?,
                // `$`-mangled generated methods rebind already-checked
                // annotations with origins legitimately erased.
                Some(ret) if method.name.contains('$') => {
                    self.resolve_generated_return_annotation(ret)?
                }
                Some(ret) => self.resolve_return_annotation(ret)?,
                None => match named_result {
                    Some(result) => self.ty_from_anno(&result.ty)?,
                    None => Ty::none(),
                },
            },
            raises: error.as_ref().is_some_and(|ty| *ty != TyKind::Never),
            error,
            self_convention: method.self_convention,
            refined_self: self.refined_receiver(method)?,
            ref_params: lower_ref_param_sigs(
                &self.enclosing_type_params,
                &regular_params,
                self.enclosing_struct_type_params.get(),
            )?,
            ref_return: match &method.ret {
                Some(SourceType::Ref { origin, .. }) => Some(self.lower_ref_sig_resolved(
                    origin.as_ref().ok_or_else(|| {
                        TypeError::Unsupported("reference return requires an origin".to_string())
                    })?,
                    &self.enclosing_type_params,
                    &regular_params,
                    self.enclosing_struct_type_params.get(),
                )?),
                _ => None,
            },
            view_return_interior: view_return_interior_tags(method.ret.as_ref()),
            view_return: self.view_return_origins(
                method.ret.as_ref(),
                &self.enclosing_type_params,
                &regular_params,
                self.enclosing_struct_type_params.get(),
                method.self_convention,
            ),
            implicit: method
                .decorators
                .iter()
                .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit"),
            // Populated by struct signature registration, which has the
            // enclosing struct's resolved field types in hand.
            parametric_origin_writes: Vec::new(),
            origin_binders: regular_params
                .iter()
                .map(|param| self.reference_parameter_struct_binder(param.origin.as_deref()))
                .collect(),
            synthesized_default: method.provenance
                == mojito_ast::ast::MethodProvenance::SynthesizedDefault,
            nested_origins: NestedOrigins::of(&method.decorators),
            overload: None,
        })
    }

    /// The refined result type an initializer's `out self` annotation
    /// declares (`out self: W[2]`), checked against the pin's receiver
    /// rules: an instance of the enclosing struct whose argument at each
    /// position is that position's own binder or names none of the struct's.
    /// `None` when the receiver is unannotated or spelled at `Self`.
    fn refined_receiver(&self, method: &Method) -> Result<Option<Ty>, TypeError> {
        let (Some(annotation), Some(self_ty)) = (&method.self_type, &self.self_ty) else {
            return Ok(None);
        };
        let refined = self.ty_from_anno(annotation)?;
        if refined == *self_ty {
            return Ok(None);
        }
        if method.self_convention != Some(ArgConvention::Out) {
            return Err(TypeError::InvalidReceiver(
                "'self' argument must have type 'Self'; use a 'where' clause to constrain the \
                 'Self' type instead"
                    .to_string(),
            ));
        }
        let not_an_instance = || {
            TypeError::InvalidReceiver(format!(
                "'self' argument must have type '{self_ty}', but actually has type '{refined}'"
            ))
        };
        let (Some((name, own)), Some((refined_name, arguments))) = (
            mojito_types::types::nominal_parts(self_ty),
            mojito_types::types::nominal_parts(&refined),
        ) else {
            return Err(not_an_instance());
        };
        if name != refined_name {
            return Err(not_an_instance());
        }
        let decls = self
            .structs
            .get(name)
            .map(|info| info.decls.as_slice())
            .unwrap_or_default();
        for ((binder, argument), decl) in own.iter().zip(arguments.iter()).zip(decls) {
            if argument == binder {
                continue;
            }
            if mentioned_binder(argument, decls).is_some() {
                return Err(TypeError::InvalidReceiver(format!(
                    "cannot use Self parameter '{argument}' in constructor whose result defines \
                     it to '{}'",
                    decl.name()
                )));
            }
        }
        Ok(Some(refined))
    }

    /// Classify a `[...]` parameter list into type and value parameters, and
    /// validate them: names must be distinct; a single bound naming a type
    /// (a scalar, a registered struct, an alias of a type, or an earlier type
    /// binder of the same list) is a **value** parameter; otherwise the bounds
    /// must all name traits (built-in or user), giving a **type** parameter. The
    /// parser guarantees each parameter carries at least one `: bound` (Mojo has
    /// no unconstrained parameters).
    /// `owner` names the declaration whose binders these are
    /// ([`binder_owner`]/[`Self::method_binder_owner`]); a default may reference the
    /// value parameters declared before it.
    pub(super) fn classify_params(
        &mut self,
        owner: &str,
        tps: &[mojito_ast::ast::TypeParam],
    ) -> Result<Vec<ParamDecl>, TypeError> {
        self.push_param_scope(&[]);
        let classified = self.classify_params_in_scope(owner, tps);
        self.tparams.pop();
        classified.map(|decls| self.bind_declared_constraints(&decls))
    }

    fn classify_params_in_scope(
        &mut self,
        owner: &str,
        tps: &[mojito_ast::ast::TypeParam],
    ) -> Result<Vec<ParamDecl>, TypeError> {
        let mut decls = Vec::new();
        let mut seen = HashSet::new();
        // A binder is owned by the template and numbered by its classified
        // slot; every reference to it carries this identity.
        let binder_owner = binder_owner(owner);
        let binder_id = |slot: usize| ParamId::new(&binder_owner, slot);
        for tp in tps {
            if let Some(scope) = self.vparams.last_mut() {
                *scope = value_scope(&decls);
            }
            if let Some(scope) = self.pack_params.last_mut() {
                *scope = pack_scope(&decls);
            }
            if let Some(scope) = self.tparams.last_mut() {
                *scope = type_scope(&decls);
            }
            if !seen.insert(tp.name.clone()) {
                return Err(TypeError::Redeclaration(tp.name.clone()));
            }
            // Origin and OriginSet parameters are semantic-only and erased before
            // runtime generic argument binding. `Origin` participates in ref
            // signatures; `OriginSet` names a capturing callable's environment.
            // Both are inferred from places/callable values rather than occupying
            // a source-visible value-parameter slot. An infer-only `Bool` that
            // binds a sibling origin parameter's `mut=` erases with it.
            if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
                continue;
            }
            if tp.is_origin_mutability_binder(tps) {
                continue;
            }
            if let Some(value_type) = &tp.value_type {
                let ty = self.ty_from_anno(value_type)?;
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), ty)?);
                continue;
            }
            // The removed `SIMDSize` width spelling (upstream hard removal,
            // 2026-08) gets a targeted migration diagnostic.
            if let [only] = tp.bounds.as_slice()
                && only == "SIMDSize"
            {
                return Err(TypeError::Unsupported(
                    "'SIMDSize' was removed; use 'SIMDLength'".to_string(),
                ));
            }
            // A lone bound that names a scalar type marks a value parameter.
            if let [only] = tp.bounds.as_slice()
                && let Some(vty) = scalar_type_name(only)
            {
                if !matches!(
                    vty.kind(),
                    TyKind::INT
                        | TyKind::UINT
                        | TyKind::Bool
                        | TyKind::StringLiteral(_)
                        | TyKind::FLOAT64
                        | TyKind::Dtype
                ) {
                    return Err(TypeError::BadValueParamType {
                        name: tp.name.clone(),
                        ty: only.clone(),
                    });
                }
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), vty)?);
                continue;
            }
            // A lone bound naming a registered struct is a struct-typed
            // value parameter (`[e: Extent]`, `[w: SIMDLength]`).
            if let [only] = tp.bounds.as_slice()
                && self.structs.contains_key(only)
            {
                let ty = TyKind::Struct(only.clone(), Vec::new());
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), ty.into())?);
                continue;
            }
            // A lone bound naming an alias of a generator type (`G: Gen` over
            // `comptime Gen = __generator_type[Idx: Int] Int`) is a value
            // parameter of that type.
            if let [only] = tp.bounds.as_slice()
                && let Some(ty) = self.generator_type_alias(only)
            {
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), ty)?);
                continue;
            }
            // A lone bound naming an earlier type binder of this list is a
            // value parameter of that type (`[T: AnyType, //, v: T]`): the
            // list is a telescope, each binder in scope for the ones after it.
            if let [only] = tp.bounds.as_slice()
                && let Some(ty) = sibling_type_binder(&decls, only)
            {
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), ty)?);
                continue;
            }
            // Any other lone annotation is resolved like any type expression,
            // as upstream's `emitExprType` resolves it: a trait makes a type
            // parameter, and a type (`Int16`, an alias of `Int`) a value
            // parameter of that type.
            if let Some(ty) = self.value_binder_type(tp, &decls) {
                decls.push(self.value_param_decl(tp, binder_id(decls.len()), ty)?);
                continue;
            }
            // A bound naming an earlier trait-valued binder (`T: Tr`, `T: Tr
            // & Hashable` over `Tr: type_of(Copyable)`) is upstream's
            // symbolic `ParamType`: the body sees the binder's declared
            // bound beside the other traits, and each use site checks the
            // trait the binder receives.
            let trait_param = tp
                .bounds
                .iter()
                .find_map(|bound| sibling_trait_binder(&decls, bound));
            let mut trait_bounds = trait_param
                .as_ref()
                .map(|(_, declared)| declared.clone())
                .unwrap_or_default();
            for bound in &tp.bounds {
                if bound == "<function type>"
                    || trait_param
                        .as_ref()
                        .is_some_and(|(binder, _)| *binder.name == **bound)
                {
                    continue;
                }
                self.check_trait_name(bound)?;
                if !trait_bounds.contains(bound) {
                    trait_bounds.push(bound.clone());
                }
            }
            if let Some((binder, _)) = &trait_param {
                self.trait_bounded_binders
                    .insert(binder_id(decls.len()), binder.clone());
            }
            decls.push(ParamDecl::Type {
                id: binder_id(decls.len()),
                name: tp.name.clone(),
                bounds: trait_bounds,
                trait_param: trait_param.map(|(binder, _)| binder),
                callable_bound: None,
                // A callable RHS is initially represented by this temporary
                // type declaration, but its default is a function value rather
                // than a type. Compile it only after the callable contract has
                // been lowered below.
                default: if tp.callable_bound.is_some() {
                    None
                } else {
                    tp.default
                        .as_ref()
                        .map(|value| self.type_default_from_expr(value))
                        .transpose()?
                },
                infer_only: tp.infer_only,
                variadic: tp.name.starts_with('*'),
                constraints: tp
                    .constraints
                    .iter()
                    .map(|condition| self.compile_where_clause(condition))
                    .collect::<Result<_, _>>()?,
            });
        }

        // Callable constraints may depend on any type parameter in this list
        // (`F: def(T) -> T`), so lower them only after the complete preliminary
        // parameter scope exists. An explicit `thin`/`capturing[...]` spelling is
        // instead a compile-time callable-value parameter in current Mojo.
        self.push_param_scope(&decls);
        let result = (|| {
            for source in tps {
                let Some(callable) = &source.callable_bound else {
                    continue;
                };
                let SourceType::Func {
                    thin, capturing, ..
                } = callable
                else {
                    return Err(TypeError::InvariantViolation(
                        "retained callable parameter bound is not a function type".to_string(),
                    ));
                };
                let checked = self.lower_anonymous_callable_type(callable, tps)?;
                let Some(index) = decls.iter().position(|decl| decl.name() == source.name) else {
                    return Err(TypeError::InvariantViolation(format!(
                        "callable constraint parameter '{}' was not classified",
                        source.name
                    )));
                };
                let ParamDecl::Type {
                    constraints,
                    infer_only,
                    variadic,
                    ..
                } = &decls[index]
                else {
                    return Err(TypeError::InvariantViolation(
                        "callable constraint was classified as a value parameter".to_string(),
                    ));
                };
                let constraints = constraints.clone();
                let infer_only = *infer_only;
                let variadic = *variadic;
                if *thin || capturing.is_some() {
                    let callable_default = source
                        .default
                        .as_ref()
                        .map(|default| {
                            self.compile_callable_default(default, &checked, &decls[..index])
                        })
                        .transpose()?;
                    decls[index] = ParamDecl::Value {
                        id: binder_id(index),
                        name: source.name.clone(),
                        ty: ParamDecl::value_list_ty(checked, variadic),
                        default: None,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    };
                } else {
                    let ParamDecl::Type { callable_bound, .. } = &mut decls[index] else {
                        unreachable!("callable type parameter changed classification")
                    };
                    *callable_bound = Some(checked);
                }
            }
            Ok(decls)
        })();
        self.tparams.pop();
        result
    }

    /// The generator type an unparameterized alias names (`comptime Gen =
    /// __generator_type[Idx: Int] Int`). `None` for any other name.
    fn generator_type_alias(&self, name: &str) -> Option<Ty> {
        let alias = self.comptime_aliases.get(name)?;
        match &alias.body {
            AliasBody::Type(ty)
                if alias.decls.is_empty() && matches!(ty.kind(), TyKind::Generator(_)) =>
            {
                Some(ty.clone())
            }
            _ => None,
        }
    }

    /// The type `tp`'s lone annotation denotes when it names neither a
    /// trait nor a trait-valued binder among `earlier`: the value type of a
    /// value parameter. `None` when it names a trait or resolves to no type.
    fn value_binder_type(
        &self,
        tp: &mojito_ast::ast::TypeParam,
        earlier: &[ParamDecl],
    ) -> Option<Ty> {
        let [only] = tp.bounds.as_slice() else {
            return None;
        };
        if self.check_trait_name(only).is_ok()
            || tp.callable_bound.is_some()
            || sibling_trait_binder(earlier, only).is_some()
        {
            return None;
        }
        self.ty_from_anno(&SourceType::Named(only.clone(), Vec::new()))
            .ok()
            .filter(|ty| !matches!(ty.kind(), TyKind::Trait(_) | TyKind::AnyTrait(_)))
    }

    /// The value declaration `tp` classifies to once its type is `ty`.
    fn value_param_decl(
        &self,
        tp: &mojito_ast::ast::TypeParam,
        id: ParamId,
        ty: Ty,
    ) -> Result<ParamDecl, TypeError> {
        let default = match (&tp.default, ty.kind()) {
            (Some(expr), TyKind::AnyTrait(bound)) => {
                Some(self.trait_param_default(tp, &id, bound, expr)?)
            }
            // A default that calls a function or constructs a struct
            // (`e: Extent = Extent(2, 3)`) is that application, and one a
            // struct's `@implicit` constructor converts (`w: W = 3`) is
            // that constructor's.
            (default, _) => default
                .as_ref()
                .map(|expr| {
                    let _lifting = self.lifting_position();
                    let compiled = self.compile_dependent_ct_expr(expr)?;
                    Ok::<_, TypeError>(
                        self.converted_parameter_value(expr, &compiled, &ty)
                            .unwrap_or(compiled),
                    )
                })
                .transpose()?,
        };
        let variadic = tp.name.starts_with('*');
        Ok(ParamDecl::Value {
            id,
            name: tp.name.clone(),
            ty: ParamDecl::value_list_ty(ty, variadic),
            default,
            callable_default: None,
            infer_only: tp.infer_only,
            variadic,
            constraints: tp
                .constraints
                .iter()
                .map(|condition| self.compile_where_clause(condition))
                .collect::<Result<_, _>>()?,
        })
    }

    /// The default of a trait-valued parameter (`Tr: type_of(AnyType) =
    /// Copyable`): a trait or a composition its bound accepts.
    fn trait_param_default(
        &self,
        tp: &mojito_ast::ast::TypeParam,
        id: &ParamId,
        bound: &[String],
        expr: &Expr,
    ) -> Result<ParamExpr, TypeError> {
        let expected = TyKind::AnyTrait(bound.to_vec());
        let mismatch = |found: String| TypeError::ParamValueType {
            callee: id.owner.to_string(),
            param: tp.name.clone(),
            expected: expected.to_string(),
            found,
        };
        let Some(traits) = self.trait_value(expr) else {
            let value = self.compile_dependent_ct_expr(expr)?;
            return Err(mismatch(value.meta().to_string()));
        };
        let traits = traits?;
        if !self.traits_satisfy(&traits, bound) {
            return Err(mismatch(TyKind::AnyTrait(traits).to_string()));
        }
        self.param_context
            .constant_as(CtValue::Trait(traits), &expected.into())
            .map_err(param_error)
    }

    /// Lower a callable contract with its own `def[...]` binders. The
    /// anonymous scope is nested inside the surrounding declaration's scope,
    /// so alpha-renamed binders retain their own identity while a signature may
    /// still depend on an outer type. Origin declarations stay in the source
    /// context used by reference/capture lowering, but are erased from the
    /// ordinary `GenericFuncTy::decls` just like named defs.
    pub(super) fn lower_anonymous_callable_type(
        &mut self,
        callable: &SourceType,
        outer_type_params: &[mojito_ast::ast::TypeParam],
    ) -> Result<Ty, TypeError> {
        let SourceType::Func {
            type_params,
            where_clauses,
            ..
        } = callable
        else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable lowering received a non-function type".to_string(),
            ));
        };
        let mut decls = self.classify_params("$callable", type_params)?;
        // A trailing `where` clause constrains the contract's own `def[...]`
        // parameters; with none declared there is nothing to constrain.
        if !where_clauses.is_empty() && decls.is_empty() {
            return Err(TypeError::Unsupported(
                "a function-type 'where' clause requires a def[...] parameter list".to_string(),
            ));
        }
        self.push_param_scope(&decls);

        let mut contextual_callable = callable.clone();
        let SourceType::Func {
            type_params: callable_context,
            where_clauses: contextual_clauses,
            ..
        } = &mut contextual_callable
        else {
            unreachable!("callable source was matched above")
        };
        // The clauses are compiled onto the parameter declarations below;
        // strip them so `ty_from_anno`'s plain-annotation rejection does not
        // fire on the contract path.
        let clauses = std::mem::take(contextual_clauses);
        // Own declarations remain first so any own Origin/OriginSet indexes
        // have the same source-relative positions they do on a named generic
        // def. The appended outer declarations are only a lookup context.
        callable_context.extend_from_slice(outer_type_params);
        let checked = self.ty_from_anno(&contextual_callable);
        // Compile the clauses inside the pushed scope so the contract's own
        // binders resolve, exactly like a named def's trailing clauses.
        let compiled: Result<Vec<_>, TypeError> = clauses
            .iter()
            .map(|condition| {
                self.compile_where_clause(condition)
                    .map(|constraint| self.bind_constraint(&constraint, &decls))
            })
            .collect();
        self.tparams.pop();
        let checked = checked?;
        for constraint in compiled? {
            match decls
                .last_mut()
                .expect("non-empty: clauses with empty decls rejected above")
            {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.push(constraint);
                }
            }
        }

        if decls.is_empty() {
            return Ok(checked);
        }
        let TyKind::Func(func) = checked.kind() else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable signature did not lower to a function type".to_string(),
            ));
        };
        Ok(TyKind::GenericFunc(Box::new(GenericFuncTy {
            decls,
            func: (**func).clone(),
        }))
        .into())
    }

    pub(super) fn type_default_from_expr(&self, value: &Expr) -> Result<Ty, TypeError> {
        match &value.kind {
            ExprKind::Identifier(name) => {
                if let Some(ty) = scalar_type_name(name) {
                    Ok(ty)
                } else {
                    self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))
                }
            }
            ExprKind::TypeApply { name, args } => {
                let (ty, attributes) = self.applied_type(name, args)?;
                self.record_bracket_attributes(value.source_span(), attributes);
                Ok(ty)
            }
            ExprKind::TypeValue(ty) => self.type_value(value, ty),
            _ => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: "type parameter default".to_string(),
            }),
        }
    }

    pub(super) fn compile_callable_default(
        &self,
        expression: &Expr,
        expected: &Ty,
        earlier: &[ParamDecl],
    ) -> Result<CallableDefault, TypeError> {
        if let ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } = &expression.kind
        {
            let condition = self.compile_dependent_ct_expr(cond)?;
            if let ExprKind::Identifier(name) = &cond.kind {
                let is_bool_parameter = earlier.iter().any(|declaration| {
                    matches!(declaration,
                        ParamDecl::Value { name: parameter, ty, .. }
                            if parameter == name && ty == &TyKind::Bool)
                });
                if !is_bool_parameter && !self.comptimes.contains_key(name) {
                    return Err(TypeError::TypeMismatch {
                        expected: TyKind::Bool.to_string(),
                        found: format!("compile-time parameter '{name}'"),
                        context: "callable default condition".to_string(),
                    });
                }
            }
            return Ok(CallableDefault::If {
                condition,
                then_value: Box::new(self.compile_callable_default(
                    then_branch,
                    expected,
                    earlier,
                )?),
                else_value: Box::new(self.compile_callable_default(
                    else_branch,
                    expected,
                    earlier,
                )?),
            });
        }

        if let ExprKind::Identifier(name) = &expression.kind
            && let Some(ParamDecl::Value { ty, .. }) = earlier
                .iter()
                .find(|declaration| declaration.name() == name)
        {
            if !self.value_coerces(ty, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: ty.to_string(),
                    context: format!("default for callable parameter '{name}'"),
                });
            }
            return Ok(CallableDefault::Parameter(ParamRef::unbound(name)));
        }

        let (name, arguments) = match &expression.kind {
            ExprKind::Identifier(name) => (name.as_str(), &[][..]),
            ExprKind::TypeApply { name, args } => (name.as_str(), args.as_slice()),
            _ => {
                return Err(TypeError::Unsupported(
                    "a callable default must be a function, an earlier callable parameter, or a conditional of those values"
                        .to_string(),
                ));
            }
        };
        let actual = self
            .infer_specialized_callable_value(
                expression.source_span(),
                name,
                arguments,
                Some(expected),
                true,
            )?
            .ok_or_else(|| TypeError::NotCallable {
                name: name.to_string(),
                ty: self
                    .lookup(name)
                    .map_or_else(|| "undefined".to_string(), ToString::to_string),
            })?;
        if !self.value_coerces(&actual, expected) {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: actual.to_string(),
                context: "callable parameter default".to_string(),
            });
        }
        let symbol = self
            .fact(&expression.source_span(), |facts| {
                facts.overload_target.as_ref()
            })
            .unwrap_or_else(|| name.to_string());
        Ok(CallableDefault::Symbol(symbol))
    }

    /// The conformance atoms a method's `where` clauses guarantee, assumed
    /// while checking its body and, in `register_struct_method_signatures`,
    /// while resolving its own signature (`-> Need[Self.T] where
    /// conforms_to(Self.T, Copyable)` on a `T: Movable` struct).
    pub(super) fn method_where_assumptions(
        &self,
        where_clauses: &[Expr],
        method_decls: &[ParamDecl],
    ) -> Result<Vec<ParamExpr>, TypeError> {
        let mut facts = Vec::new();
        for condition in where_clauses {
            let constraint = self.compile_where_clause(condition)?;
            self.guaranteed_facts(&self.bind_constraint(&constraint, method_decls), &mut facts);
        }
        Ok(facts)
    }

    /// The callable identity the `overload_index`-th registered overload of
    /// method `declaration.method_name` commits its transfer effects under:
    /// its symbol, since same-name overloads may differ in loan behavior and
    /// a bare method key would merge them by declaration order.
    pub(super) fn method_effect_key(
        &self,
        declaration: &str,
        method_name: &str,
        overload_index: usize,
    ) -> Result<String, TypeError> {
        let (overloaded, signature) = self
            .structs
            .get(declaration)
            .and_then(|info| info.methods.get(method_name))
            .and_then(|methods| {
                methods
                    .get(overload_index)
                    .map(|signature| (methods.len() > 1, signature))
            })
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "method signature for '{declaration}.{method_name}' is not registered"
                ))
            })?;
        Ok(if overloaded {
            method_lowered_name(
                declaration,
                method_name,
                signature,
                self.self_instance_ty(declaration).as_ref(),
            )
        } else {
            format!("{declaration}.{method_name}")
        })
    }

    pub(super) fn check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        module: Option<&String>,
        declaration: &str,
        method_index: usize,
        overload_index: usize,
    ) -> Result<(), TypeError> {
        self.parametric_write_frames.borrow_mut().push(Vec::new());
        let decls =
            self.classify_params(&self.method_binder_owner(declaration, m), &m.type_params)?;
        let saved_site = self
            .method_site
            .replace((module.cloned(), declaration.to_string()));
        self.push_param_scope(&decls);
        let saved = self.enclosing_type_params.clone();
        let saved_struct_count = self.enclosing_struct_type_params.replace(saved.len());
        self.enclosing_type_params.extend(m.type_params.clone());
        let assumptions = self.method_where_assumptions(&m.where_clauses, &decls);
        let result = match assumptions {
            Ok(mut assumptions) => {
                assumptions.extend(self.method_propositions(&m.where_clauses, &decls));
                self.push_assumptions(assumptions);
                let result = (|| {
                    for param in 0..m.params.len() {
                        let site = mojito_checked::checked::AnnotationSite::MethodParam {
                            module: module.cloned(),
                            declaration: declaration.to_string(),
                            method: method_index,
                            param,
                        };
                        let ty = self
                            .declaration_types
                            .borrow()
                            .get(&site)
                            .cloned()
                            .ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "method parameter {} for '{}.{}' has no checked type",
                                    param, declaration, m.name
                                ))
                            })?;
                        // A positional variadic pack (`var *values: T`) is
                        // never linear: upstream destroys its elements
                        // implicitly even for a non-`Deinitable` `T`.
                        let verdict = (m.params[param].kind
                            != mojito_ast::ast::ParamKind::Variadic)
                            .then(|| self.destruction_verdict(&ty))
                            .flatten();
                        self.record_declaration_verdict(site, verdict);
                    }
                    // Transfer summaries belong to the same callable identity
                    // as dispatch. Same-name overloads may have different loan
                    // behavior, so a bare method key would merge unrelated
                    // effects according to declaration order.
                    let effect_key = self.method_effect_key(
                        declaration,
                        lifecycle_method_name(m),
                        overload_index,
                    )?;
                    self.check_method_inner(self_ty, m, effect_key)
                })();
                self.pop_assumptions();
                result
            }
            Err(error) => Err(error),
        };
        self.enclosing_type_params = saved;
        self.enclosing_struct_type_params.set(saved_struct_count);
        self.tparams.pop();
        self.method_site = saved_site;
        let propagated = self
            .parametric_write_frames
            .borrow_mut()
            .pop()
            .expect("method write-requirement frame");
        if result.is_ok() && !propagated.is_empty() {
            let method_name = lifecycle_method_name(m);
            let signature = self
                .structs
                .get_mut(declaration)
                .and_then(|info| info.methods.get_mut(method_name))
                .and_then(|methods| methods.get_mut(overload_index))
                .expect("checked method signature remains registered");
            for id in propagated {
                if !signature.parametric_origin_writes.contains(&id) {
                    signature.parametric_origin_writes.push(id);
                }
            }
        }
        result
    }

    pub(super) fn check_method_inner(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        effect_key: String,
    ) -> Result<(), TypeError> {
        self.check_decorators(
            DecoratorSite::Method {
                init: m.name == "__init__",
            },
            &m.decorators,
        )?;
        let is_implicit = m
            .decorators
            .iter()
            .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit");
        // The single argument is ordinarily a plain or owned (`var`) value —
        // upstream's `Optional.__init__(out self, var value: T)` — and a
        // `ref`-convention parameter with an origin clause is also accepted
        // (a view constructor — current Mojo's origin-refining implicit
        // conversion).
        let implicit_convention_ok = match &m.params.first().and_then(|p| p.convention) {
            None | Some(ArgConvention::Var) => true,
            Some(ArgConvention::Ref) => m
                .params
                .first()
                .is_some_and(|parameter| parameter.origin.is_some()),
            Some(_) => false,
        };
        if is_implicit
            && (m.name != "__init__"
                || !m.has_self
                || m.self_convention != Some(ArgConvention::Out)
                || m.params.len() != 1
                || m.params[0].kind != mojito_ast::ast::ParamKind::Regular
                || m.params[0].default.is_some()
                || !implicit_convention_ok
                || m.ret.is_some())
        {
            return Err(TypeError::Unsupported(
                "@implicit requires a single-argument '__init__(out self, value: T)' \
                 (the argument may be a 'var' or 'ref [origin]' parameter)"
                    .to_string(),
            ));
        }
        self.validate_origin_signature(
            &self.enclosing_type_params,
            &m.params,
            m.self_origin.as_ref(),
        )?;
        if is_legacy_bare_move_constructor(m) {
            return Err(TypeError::Unsupported(
                "the move initializer requires the consuming 'deinit' convention: write \
                 '__init__(out self, *, deinit move: Self)'; a bare 'move:' parameter is \
                 not accepted"
                    .to_string(),
            ));
        }
        // `out self` initializes the receiver: it is allowed on the **`__init__`**
        // lifecycle method (a hand-written constructor), where `self`'s fields are
        // assigned in the body. `ref self` (parametric-mutability references), and
        // `out self` on any other method, still need semantics we don't model, so
        // they stay flagged. A plain `self`, `imm self`, `mut self`, or `var
        // self` consuming method is fine.
        // `out self` initializes the receiver — allowed on the lifecycle methods
        // `__init__` (constructor), `__copyinit__` (copy), and `__moveinit__` (move),
        // whose bodies assign `self`'s fields. `ref self`, and `out self` elsewhere,
        // stay flagged.
        let is_lifecycle_init = matches!(
            m.name.as_str(),
            "__init__" | "__copyinit__" | "__moveinit__"
        );
        let out_init = matches!(m.self_convention, Some(mojito_ast::ast::ArgConvention::Out))
            && is_lifecycle_init;
        if matches!(m.self_convention, Some(mojito_ast::ast::ArgConvention::Out)) && !out_init {
            return Err(TypeError::Unsupported(
                "'out self' receiver outside a lifecycle initializer".to_string(),
            ));
        }
        let named_result = mojito_ast::ast::named_result(&m.params);
        if m.params
            .iter()
            .filter(|p| DeclaredParam::is_named_result(*p))
            .count()
            > 1
        {
            return Err(TypeError::Unsupported(
                "multiple named 'out' results".to_string(),
            ));
        }
        if named_result.is_some() && m.ret.is_some() {
            return Err(TypeError::Unsupported(
                "a function cannot declare both a named result and '->' return type".to_string(),
            ));
        }
        let ret_ty = self.in_signature_scope(m.has_self.then_some(self_ty), |this| {
            let generated = this.generated_declaration.get();
            this.generated_declaration
                .set(generated || m.name.contains('$'));
            let params = this.param_tys(&m.params);
            this.generated_declaration.set(generated);
            params?;
            match &m.ret {
                Some(SourceType::Ref { referent, .. }) => this.ty_from_anno(referent),
                // `$`-mangled generated methods rebind already-checked
                // annotations with origins legitimately erased.
                Some(t) if m.name.contains('$') => this.resolve_generated_return_annotation(t),
                Some(t) => this.resolve_return_annotation(t),
                None => named_result
                    .map_or_else(|| Ok(Ty::none()), |result| this.ty_from_anno(&result.ty)),
            }
        })?;
        let regular: Vec<&FnParam> = m
            .params
            .iter()
            .filter(|param| {
                param.kind == mojito_ast::ast::ParamKind::Regular && !param.is_named_result()
            })
            .collect();
        let ref_return = match &m.ret {
            Some(SourceType::Ref { origin, .. }) => Some(self.lower_ref_sig_resolved(
                origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported("reference return requires an origin".to_string())
                })?,
                &self.enclosing_type_params,
                &regular,
                self.enclosing_struct_type_params.get(),
            )?),
            _ => None,
        };
        // A default may name the method's own value parameters
        // (`x: Int = Self.n + j`), bound around the defaults as in the body.
        let method_decls = self.classify_params(&self.method_owner(self_ty, m), &m.type_params)?;
        self.push_scope();
        let defaults = self
            .declare_value_parameters(&method_decls)
            .and_then(|()| self.check_method_defaults(m));
        self.pop_scope();
        defaults?;
        self.push_scope();
        self.raising_context
            .push(self.declared_error(m.raises, m.raises_type.as_ref())?);
        // A method body is a carry site (`body_carry`): keyed by its first
        // statement, which the final re-key made unique, and named as the
        // template mechanism names it (`Owner.method`, overloads alike).
        let site = m
            .body
            .first()
            .filter(|_| self.carries_bodies())
            .map(|first| {
                let display = self.method_site.as_ref().map_or_else(
                    || effect_key.clone(),
                    |(_, owner)| format!("{owner}.{}", m.name),
                );
                (first.source_span(), display)
            });
        let carried = site
            .as_ref()
            .is_some_and(|(key, display)| self.carry_body(key, display));
        let mut result = if carried {
            Ok(())
        } else {
            let start = site.as_ref().map(|(key, _)| self.enter_body_site(key));
            let enclosing = self.lifting_owner.replace(effect_key.clone());
            let checked = self.bind_and_check_method(self_ty, m, &ret_ty, ref_return, effect_key);
            self.lifting_owner = enclosing;
            if let (Some((key, display)), Some(start)) = (site, start) {
                self.leave_body_site(key, &display, &start);
            }
            checked
        };
        // Definite initialization (conservative, flow-insensitive first pass): an
        // `__init__` must assign every declared field somewhere in its body, so a
        // constructed value has no unset fields. Path-sensitive DI (assign exactly
        // once, before any read, on every path) is left for a later refinement.
        if result.is_ok()
            && out_init
            && let TyKind::Struct(sname, _) = self_ty.kind()
        {
            result = self.check_definite_init(sname, &m.name, &m.body);
        }
        self.raising_context.pop();
        self.pop_scope();
        result
    }

    /// Verify an `out self` lifecycle method (`method`) assigns every declared field
    /// of `sname` (flow-insensitive: assigned *somewhere*). Reports the first missing
    /// field.
    pub(super) fn check_definite_init(
        &self,
        sname: &str,
        method: &str,
        body: &[Stmt],
    ) -> Result<(), TypeError> {
        let info = self.structs.get(sname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
        })?;
        // A constructor whose body is the runtime trap (an unspecialized
        // generic template's stub) never completes, so it initializes nothing.
        if body.first().is_some_and(|statement| {
            matches!(&statement.kind, StmtKind::Expr(Expr { kind: ExprKind::Call { name, .. }, .. }) if is_diverging_intrinsic(name))
        }) {
            return Ok(());
        }
        for (field, _) in &info.fields {
            if !definitely_initializes_self_field(body, field, &|stmt| self.stmt_reach(stmt)) {
                return Err(TypeError::UninitializedField {
                    struct_name: sname.to_string(),
                    method: method.to_string(),
                    field: field.clone(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn bind_and_check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        ret_ty: &Ty,
        ref_return: Option<mojito_types::origin::RefSig>,
        effect_key: String,
    ) -> Result<(), TypeError> {
        // Compile-time callable/scalar value parameters occupy named runtime
        // slots in a method body, just as they do in a generic free function.
        // Type parameters remain type-only and are available through `tparams`.
        let method_decls = self.classify_params(&self.method_owner(self_ty, m), &m.type_params)?;
        self.declare_value_parameters(&method_decls)?;
        let mut reference_type_params = self.enclosing_type_params.clone();
        reference_type_params.extend(m.type_params.iter().cloned());
        let self_writable = ref_binding_is_writable(
            m.self_convention,
            m.self_origin.as_deref(),
            &reference_type_params,
        );
        if m.has_self {
            // A refined initializer's `self` has its refined type, while
            // `Self` stays the struct at its own binders, as at the pin.
            let receiver = match &m.self_type {
                Some(annotation) if m.self_convention == Some(ArgConvention::Out) => {
                    self.ty_from_anno(annotation)?
                }
                _ => self_ty.clone(),
            };
            self.bind_receiver(&receiver, self_writable)?;
        }
        for p in &m.params {
            // Parameter annotations follow the initialized-local rule
            // (pin-attested): a bare origin-slotted generic infers per call,
            // a partial application that omits an origin slot rejects, and a
            // placeholder marks the slot explicitly inferred (a bare
            // `Pointer[T, _]` is concrete here only).
            self.resolving_parameter_annotation.set(true);
            self.bare_literal_parameter
                .set(is_literal_annotation(&p.ty));
            let generated = self.generated_declaration.get();
            self.generated_declaration
                .set(generated || m.name.contains('$'));
            let resolved =
                self.resolve_storage_annotation(&p.ty, super::StorageStrictness::AllowBare);
            self.generated_declaration.set(generated);
            self.bare_literal_parameter.set(false);
            self.resolving_parameter_annotation.set(false);
            let mut pty = resolved?;
            pty = match p.kind {
                mojito_ast::ast::ParamKind::Variadic => {
                    mojito_types::types::variadic_collector_type(pty, p.convention)
                }
                mojito_ast::ast::ParamKind::KwVariadic => {
                    self.kwargs_collector_ty(pty, &format!("keyword collector '{}'", p.name))?
                }
                mojito_ast::ast::ParamKind::Regular => pty,
            };
            let writable = ref_parameter_is_writable(p, &reference_type_params);
            let binder = matches!(p.convention, Some(mojito_ast::ast::ArgConvention::Ref))
                .then(|| self.reference_parameter_struct_binder(p.origin.as_deref()))
                .flatten();
            self.bind_parameter(
                BodyParam {
                    name: &p.name,
                    kind: p.kind,
                    convention: p.convention,
                },
                &pty,
                writable,
                binder,
            )?;
        }
        self.check_method_body(MethodBody {
            name: &m.name,
            params: m
                .params
                .iter()
                .map(|param| BodyParam {
                    name: &param.name,
                    kind: param.kind,
                    convention: param.convention,
                })
                .collect(),
            self_writable,
            initializing: matches!(m.self_convention, Some(mojito_ast::ast::ArgConvention::Out))
                && matches!(
                    lifecycle_method_name(m),
                    "__init__" | "__copyinit__" | "__moveinit__"
                ),
            effect_key,
            method_decls: &method_decls,
            ref_return,
            return_annotation: Self::body_return_annotation(m.ret.as_ref(), &m.name),
            generic: !m.type_params.is_empty(),
            ret_ty,
            body: &m.body,
        })
    }

    /// Declare a method's receiver `self` at `self_ty`, rooting the loans
    /// its value carries at its own place.
    pub(super) fn bind_receiver(&mut self, self_ty: &Ty, writable: bool) -> Result<(), TypeError> {
        self.declare_with_mutability("self", self_ty.clone(), writable)?;
        if self.type_carries_loans(self_ty)
            && let Some(owner) = self.lookup_owner("self")
        {
            self.set_aggregate_origins(
                "self",
                vec![mojito_types::origin::Origin::Place(
                    mojito_types::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    },
                )],
            );
        }
        Ok(())
    }

    /// Declare a method's parameter `param` at its body type `pty`: a `ref`
    /// one as a reference parameter (`binder` the enclosing struct's origin
    /// binder its clause names), and every one rooting the loans its value
    /// carries at its own place.
    pub(super) fn bind_parameter(
        &mut self,
        param: BodyParam<'_>,
        pty: &Ty,
        writable: bool,
        binder: Option<mojito_types::origin::PointerOrigin>,
    ) -> Result<(), TypeError> {
        let BodyParam {
            name,
            kind,
            convention,
        } = param;
        self.declare_with_mutability(
            name,
            pty.clone(),
            kind == mojito_ast::ast::ParamKind::KwVariadic || param.is_named_result() || writable,
        )?;
        self.record_owned_pack(name, kind, convention);
        if matches!(convention, Some(mojito_ast::ast::ArgConvention::Ref)) {
            self.register_reference_parameter(name, pty.clone(), writable, binder);
        }
        if !matches!(pty.kind(), TyKind::Ref(_))
            && self.type_may_carry_loans(pty)
            && let Some(owner) = self.lookup_owner(name)
        {
            self.set_aggregate_origins(
                name,
                vec![mojito_types::origin::Origin::Place(
                    mojito_types::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    },
                )],
            );
        }
        Ok(())
    }

    /// Check a method's body over its bound receiver and parameters, in the
    /// transfer frame its effect key names, and commit the effects it
    /// records.
    pub(super) fn check_method_body(&mut self, method: MethodBody<'_>) -> Result<(), TypeError> {
        let MethodBody {
            name,
            params,
            self_writable,
            initializing,
            effect_key,
            method_decls,
            ref_return,
            return_annotation,
            generic,
            ret_ty,
            body,
        } = method;
        // `self` is writable in a `mut self` method, or an `out self` `__init__`
        // (which assigns its fields). Restored after the body.
        let saved = std::mem::replace(&mut self.self_mutable, self_writable);
        let saved_initializing = std::mem::replace(&mut self.self_initializing, initializing);
        let regular = || {
            params.iter().filter(|param| {
                param.kind == mojito_ast::ast::ParamKind::Regular && !param.is_named_result()
            })
        };
        let owners: Vec<_> = regular()
            .map(|param| {
                self.lookup_owner(param.name)
                    .expect("bound method parameter")
            })
            .collect();
        let self_owner = self.lookup_owner("self");
        let mut allowed: HashSet<_> = owners.iter().copied().collect();
        allowed.extend(self_owner);
        // Variadic collectors are parameters too: a loan rooted at the
        // callee-owned pack moves outward with the stored value (the
        // bundled Tuple constructor stores its `*args` collector into
        // `self.storage`).
        allowed.extend(
            params
                .iter()
                .filter(|param| param.kind != mojito_ast::ast::ParamKind::Regular)
                .filter_map(|param| self.lookup_owner(param.name)),
        );
        self.aggregate_escape_contexts
            .push((self.scopes.len().saturating_sub(1), allowed));
        self.transfer_frames.borrow_mut().push(TransferFrame {
            callable: effect_key,
            param_owners: owners.clone(),
            param_borrowed: regular()
                .map(|param| {
                    matches!(
                        param.convention,
                        Some(
                            mojito_ast::ast::ArgConvention::Mut
                                | mojito_ast::ast::ArgConvention::Ref
                        )
                    )
                })
                .collect(),
            self_owner,
            value_callables: method_decls
                .iter()
                .filter_map(|decl| match decl {
                    ParamDecl::Value { name, .. } if decl.binds_callables() => {
                        Some(name.trim_start_matches('*').to_string())
                    }
                    _ => None,
                })
                .collect(),
            effects: Vec::new(),
            recorded: Vec::new(),
            latent_escapes: false,
            call_throughs: Vec::new(),
        });
        self.raise_observation_frames
            .borrow_mut()
            .push((self.handled_raise_depth, false));
        self.return_ref_contracts.push(ref_return.map(|signature| {
            (
                signature,
                owners,
                self_owner.map(|root| mojito_types::origin::OriginPlace {
                    root,
                    path: Vec::new(),
                }),
            )
        }));
        self.return_annotations.push(return_annotation);
        // A method body is a function scope for nested closures just as a
        // top-level `def` body is. In particular, an explicit capture list on a
        // method-local function may name `self`, parameters, and method locals.
        self.function_bases.push(self.scopes.len() - 1);
        let named_result = params.iter().find(|param| param.is_named_result());
        self.named_result_context
            .push(match (initializing, named_result, self.lookup("self")) {
                (true, _, Some(receiver)) => super::BodyResult::Receiver(receiver.clone()),
                (_, Some(_), _) => super::BodyResult::Named,
                _ => super::BodyResult::Returned,
            });
        self.deferred_result_bodies
            .push(matches!(ret_ty.kind(), TyKind::KgenDeferred));
        count_body_inference(!self.self_decls.is_empty() || generic, || {
            self.method_site
                .as_ref()
                .map_or_else(|| name.to_string(), |(_, owner)| format!("{owner}.{name}"))
        });
        let result = self.check_block(body, Some(ret_ty), false);
        self.named_result_context.pop();
        self.deferred_result_bodies.pop();
        self.function_bases.pop();
        self.return_annotations.pop();
        self.return_ref_contracts.pop();
        self.raise_observation_frames.borrow_mut().pop();
        if let Some(frame) = self.transfer_frames.borrow_mut().pop() {
            self.top_level_callables
                .borrow_mut()
                .insert(frame.callable.clone());
            if !frame.effects.is_empty() {
                // A bundled seed is never inferred from the body, so what
                // the body records joins it.
                let mut effects = seeded_transfer_effects()
                    .remove(&frame.callable)
                    .unwrap_or_default();
                for effect in frame.effects {
                    if !effects.contains(&effect) {
                        effects.push(effect);
                    }
                }
                self.transfer_effects
                    .borrow_mut()
                    .insert(frame.callable.clone(), effects);
            }
            if !frame.call_throughs.is_empty() {
                self.call_through_effects
                    .borrow_mut()
                    .insert(frame.callable, frame.call_throughs);
            }
        }
        self.aggregate_escape_contexts.pop();
        self.self_mutable = saved;
        self.self_initializing = saved_initializing;
        result?;
        let reach = |stmt: &Stmt| self.stmt_reach(stmt);
        let completes = match named_result {
            Some(result) => definitely_initializes_named_result(body, result.name, &reach),
            None => *ret_ty == TyKind::None || definitely_returns(body, &reach),
        };
        if !completes {
            return Err(TypeError::MissingReturn(name.to_string()));
        }
        Ok(())
    }

    /// Type a struct construction `Name[param_args](args)` (the fieldwise
    /// constructor). Type parameters are supplied explicitly or inferred from the
    /// field arguments; value parameters must be supplied explicitly.
    /// Record the `ref [origin]` constructor-argument borrows for a
    /// construction call: the constructed aggregate keeps each lent place
    /// alive through the binding's loans (a view constructor's contract).
    /// Positional slots only; keyword-bound reference parameters have no
    /// current stdlib surface.
    pub(in crate::checker) fn record_constructor_reference_borrows(
        &self,
        span: &SourceSpan,
        ref_params: &[Option<mojito_types::origin::RefSig>],
        slots: &[ArgSlot],
    ) {
        let mut arguments = Vec::new();
        for (parameter, signature) in ref_params.iter().enumerate() {
            let Some(signature) = signature else { continue };
            let Some(ArgSlot::Positional(index)) = slots.get(parameter) else {
                continue;
            };
            arguments.push((
                *index,
                signature.mutability == mojito_types::origin::SigMutability::Mutable,
            ));
        }
        if !arguments.is_empty() {
            // A re-inference of the same construction keeps the owner a
            // temporary-receiver materialization already minted on it.
            let mut record = self.record_at(span.clone());
            let materialized = match record.adjustment.as_deref() {
                Some(mojito_checked::checked::SemanticAdjustment::BorrowRefArguments {
                    materialized,
                    ..
                }) => *materialized,
                _ => None,
            };
            record.adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::BorrowRefArguments {
                    arguments,
                    materialized,
                },
            ));
        }
    }

    /// A construction is its `__init__`'s call: its arguments bind and
    /// alias-check as a call's do, and a raising initializer makes it a
    /// raising call.
    fn record_constructor_call(
        &self,
        span: &SourceSpan,
        name: &str,
        binding: &ConstructorBinding<'_>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        let (effective, _) = self.solve_call_origins(
            binding.slots,
            binding.conventions,
            binding.ref_params,
            None,
            args,
            kwargs,
        )?;
        let exclusivity = &binding.exclusivity;
        let copied_reads = binding
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let argument = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let Some(parameter) = exclusivity.bound.get(index) else {
                    return Ok(false);
                };
                let ty = self.infer_with_expected(argument, parameter, true)?;
                let convention = effective.get(index).copied().flatten();
                Ok(self.argument_is_independent_copy(convention, argument, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        self.check_free_call_aliasing(
            &ExclusivityCallee {
                name: &exclusivity.callee,
                parameter_names: exclusivity.parameter_names,
                declared: &exclusivity.declared,
                bound: &exclusivity.bound,
                nested_origins: exclusivity.nested_origins,
                collected: Some(&exclusivity.gathered),
                initializer: true,
            },
            &effective,
            &copied_reads,
            binding.slots,
            args,
            kwargs,
        )?;
        self.record_argument_borrows(
            binding.slots,
            &effective,
            binding.collected,
            args,
            kwargs,
            None,
        );
        self.record_constructor_reference_borrows(span, binding.ref_params, binding.slots);
        if binding.raises {
            let error = binding.error.cloned().unwrap_or_else(Ty::error);
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising constructor '{name}'"), error)?;
        }
        Ok(())
    }

    /// Each method parameter default fits its parameter's type and names no
    /// runtime binding.
    fn check_method_defaults(&self, m: &Method) -> Result<(), TypeError> {
        super::statements::reject_default_lambdas(&m.params)?;
        for param in &m.params {
            let Some(default) = &param.default else {
                continue;
            };
            self.check_parameter_default(&param.name, default, &self.ty_from_anno(&param.ty)?)?;
        }
        Ok(())
    }

    /// A method parameter `name`'s default fits its type `expected` and
    /// names no runtime binding.
    pub(super) fn check_parameter_default(
        &self,
        name: &str,
        default: &Expr,
        expected: &Ty,
    ) -> Result<(), TypeError> {
        if let Some(name) = self.dynamic_default_reference(default) {
            return Err(TypeError::DynamicDefault(name));
        }
        let found = self.infer_with_expected(default, expected, true)?;
        // Fall back to an `@implicit` converting constructor (records the
        // ctor target for omitted-arg materialization), matching the
        // free-function default path and the binding/argument positions.
        if !coerces(&found, expected)
            && !self.record_constructor_conversion(default, &found, expected)?
        {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: found.to_string(),
                context: format!("default value of method parameter '{name}'"),
            });
        }
        Ok(())
    }

    /// The immutable origin binders a construction's struct-typed arguments
    /// recorded, each under the field it initializes (`Wrap(Cell[ImmOrigin(o)](…))`
    /// gives `(["cell"], o)`): a fieldwise initializer's arguments land in
    /// declaration order, and a handwritten `__init__` conventionally forwards
    /// a same-named parameter into each field, as `aggregate_field_origins`
    /// assumes too.
    fn nested_immutable_binders(
        &self,
        info: &StructInfo,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Vec<ImmutableOriginBinder> {
        let field_argument = |field: &str, position: usize| {
            args.get(position).or_else(|| {
                kwargs
                    .iter()
                    .find(|argument| argument.name == field)
                    .map(|argument| &argument.value)
            })
        };
        let mut nested = Vec::new();
        for (field, ty) in &info.fields {
            if !matches!(ty.kind(), TyKind::Struct(..)) {
                continue;
            }
            let position = if info.fieldwise_init {
                info.fields.iter().position(|(name, _)| name == field)
            } else {
                info.methods.get("__init__").and_then(|signatures| {
                    signatures
                        .iter()
                        .find(|signature| signature.params.len() >= args.len())
                        .and_then(|signature| signature.names.iter().position(|name| name == field))
                })
            };
            let Some(argument) = position.and_then(|position| field_argument(field, position))
            else {
                continue;
            };
            // The argument's own construction records its casts when it is
            // typed, which the outer construction's argument typing has not
            // yet done at this point.
            if self.infer(argument).is_err() {
                continue;
            }
            nested.extend(
                self.recorded_immutable_origin_binders(argument)
                    .into_iter()
                    .map(|(mut path, id)| {
                        path.insert(0, field.clone());
                        (path, id)
                    }),
            );
        }
        nested
    }

    /// The pack a bare construction of a variadic struct (`Pair((1, True))`,
    /// no `[...]` arguments on a struct keyed by exactly one type pack) must
    /// infer from the constructor it selects; `None` for every other call.
    /// The public `Tuple` is its own request kind and is left out.
    fn inferred_construction_pack<'d>(
        name: &str,
        decls: &'d [ParamDecl],
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Option<&'d str> {
        if !param_args.is_empty()
            || name == mojito_types::types::TUPLE_TYPE_NAME
            || name == mojito_types::types::TSTRING_TYPE_NAME
        {
            return None;
        }
        match decls {
            [
                ParamDecl::Type {
                    name,
                    variadic: true,
                    ..
                },
            ] => Some(name.trim_start_matches('*')),
            _ => None,
        }
    }

    /// A bare construction of a variadic struct infers the pack only from a
    /// constructor parameter that spells it — a spread (`Tuple[*Self.Ts]`) or
    /// the `*args: *Self.Ts` collector. A constructor naming the pack nowhere
    /// (`Variant.__init__[T](out self, var value: T)`) leaves it unknown; it
    /// is not silently the empty pack.
    fn reject_unconstrained_pack(
        name: &str,
        decls: &[ParamDecl],
        param_args: &[mojito_ast::ast::ParamArg],
        constructed: Option<&Ty>,
        patterns: &[Ty],
    ) -> Result<(), TypeError> {
        let Some(pack) = Self::inferred_construction_pack(name, decls, param_args)
            .filter(|_| constructed.is_none())
        else {
            return Ok(());
        };
        let names_pack = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Param { binder, .. }
                if binder.name.starts_with('*') && binder.name.trim_start_matches('*') == pack)
        };
        if patterns
            .iter()
            .any(|pattern| mojito_types::types::mentions(pattern, &names_pack))
        {
            return Ok(());
        }
        Err(TypeError::CannotInferTypeParam {
            name: name.to_string(),
            param: pack.to_string(),
        })
    }

    /// What a bare variadic-struct construction does once its pack is
    /// solved. The pack is recorded keyed by the call occurrence: the
    /// compiler's discovery loop replays a closed recording as the struct
    /// specialization the elaborator then rewrites the call to (the
    /// scalar-range constructor rewrite's shape). When that specialization
    /// is already declared (an earlier round minted it for another
    /// occurrence, or the same round's clone annotations spell it), the call
    /// types as the concrete struct's construction: it is checked against
    /// the constructor it will be rewritten to, and its type agrees with
    /// every annotation already spelling the specialization. `None` leaves
    /// the call to the template's own constructor path — for every other
    /// call, and while the specialization is not declared yet.
    fn finish_variadic_construction(
        &self,
        span: &SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        tyargs: &[TyArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let decls = &self.structs.get(name)?.decls;
        Self::inferred_construction_pack(name, decls, param_args)?;
        self.record_at(span.clone()).generic_instantiation =
            Some(mojito_checked::checked::GenericInstantiation {
                callee: name.to_string(),
                parameter_names: Vec::new(),
                parameter_types: Vec::new(),
                variadic: None,
                arguments: tyargs.to_vec(),
                folded_arguments: Vec::new(),
            });
        let [TyArg::Val(pack @ CtValue::Tuple(_))] = tyargs else {
            return None;
        };
        let specialization =
            mojito_symbol::symbol::mangle(name, std::slice::from_ref(pack)).ok()?;
        if !self.structs.contains_key(&specialization) {
            return None;
        }
        Some(self.infer_construction(span, &specialization, &[], args, kwargs))
    }

    /// The pack a constructor candidate's collector takes whole when the
    /// call forwards one still a parameter (`Tuple(*args^)`): it binds to
    /// the collector as a call's does, which checks its ownership and bounds
    /// and solves the struct's pack to the caller's. `Ok(None)` when the
    /// call forwards no pack; an error when this candidate cannot take the
    /// spread, with the binding failure the miss reports when there is one.
    fn constructor_forwarded_pack(
        &self,
        name: &str,
        sig: &MethodSig,
        args: &[Expr],
        matched: &mojito_ast::call::CallSlots,
    ) -> Result<Option<Ty>, Option<TypeError>> {
        let forwarded = self
            .forwarded_pack_argument(name, args, sig.variadic.is_some())
            .map_err(|_| None)?;
        let (Some((position, pack)), Some(collector)) = (forwarded, sig.variadic.as_ref()) else {
            return Ok(None);
        };
        mojito_ast::call::bind_spread(matched, position).map_err(|_| None)?;
        self.bind_forwarded_pack(
            name,
            &args[position],
            &pack,
            collector,
            Some(sig.variadic_convention == Some(ArgConvention::Var)),
        )
        .map(Some)
        .map_err(Some)
    }

    /// Record a construction's compile-time arguments as a generic `def`
    /// call records its own: the struct's, then the selected constructor's
    /// own, in the layout the constructor's declaration restates them
    /// (`Counter[4](1)` calls `Counter[4].__init__`). A bracket argument
    /// solved to a closed value travels in the record alone, so the call
    /// lowers no run-time form for it.
    fn record_construction_instantiation(
        &self,
        span: &SourceSpan,
        name: &str,
        bracket: (&[ParamDecl], &[mojito_ast::ast::ParamArg]),
        parameter_names: &[String],
        arguments: Vec<TyArg>,
    ) {
        if arguments.is_empty() {
            return;
        }
        let (decls, param_args) = bracket;
        let folded_arguments = crate::checker::call_inference::folded_parameter_arguments(
            decls, param_args, &arguments,
        );
        self.record_at(span.clone()).generic_instantiation =
            Some(mojito_checked::checked::GenericInstantiation {
                callee: name.to_string(),
                parameter_names: parameter_names.to_vec(),
                parameter_types: Vec::new(),
                variadic: None,
                arguments,
                folded_arguments,
            });
    }

    /// The initializer's own list an explicit `T.__init__[ks](…)` spelled
    /// for the construction at `span`, marked bound; empty for any other
    /// construction.
    fn spelled_initializer_args_at(&self, span: &SourceSpan) -> Vec<mojito_ast::ast::ParamArg> {
        self.spelled_initializer_args
            .borrow()
            .get(span)
            .map(|(arguments, bound)| {
                bound.set(true);
                arguments.clone()
            })
            .unwrap_or_default()
    }

    /// The exact lowered `__init__` a zero-argument construction of `target`
    /// selects, as upstream's `emitConstructorCall` resolves `C()` with no
    /// operands: among the overloads no argument binds whose `where` clause
    /// holds under `target`'s arguments, the best ranked.
    pub(super) fn empty_constructor(&self, target: &Ty) -> Result<String, TypeError> {
        let TyKind::Struct(name, arguments) = target.kind() else {
            return Err(TypeError::InvariantViolation(format!(
                "empty construction of non-nominal '{target}'"
            )));
        };
        let no_constructor = || TypeError::BadCall {
            func: name.clone(),
            reason: "no matching constructor for a call with no arguments".to_string(),
        };
        let info = self.structs.get(name).ok_or_else(no_constructor)?;
        let sigs = info.methods.get("__init__").ok_or_else(no_constructor)?;
        let ranked = sigs
            .iter()
            .filter(|sig| {
                mojito_ast::call::match_call_slots(
                    &sig.names,
                    &sig.required,
                    sig.positional_only,
                    sig.keyword_only,
                    0,
                    &[],
                    mojito_ast::call::CallVariadics {
                        positional: sig.variadic.is_some(),
                        keyword: sig.kw_variadic.is_some(),
                    },
                )
                .is_ok()
                    && self
                        .method_constraint_result(sig, &[], &info.decls, arguments)
                        .is_ok()
                    && sig.refined_self.as_ref().is_none_or(|refined| {
                        self.self_instance_ty(name).is_some_and(|instance| {
                            refinement_admits(refined, &instance, &sig.decls, target)
                        })
                    })
            })
            .map(|sig| {
                let rank = overload_rank(
                    0,
                    sig.variadic.is_some(),
                    baked_decl_count(&sig.decls),
                    !decls_are_concrete(&sig.decls),
                );
                (rank, sig)
            })
            .collect::<Vec<_>>();
        let best = ranked.iter().map(|(rank, _)| *rank).min();
        let sig = match ranked
            .iter()
            .filter(|(rank, _)| Some(*rank) == best)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [] => return Err(no_constructor()),
            [(_, sig)] => *sig,
            _ => {
                return Err(TypeError::BadCall {
                    func: name.clone(),
                    reason: "ambiguous overloaded constructor call".to_string(),
                });
            }
        };
        if sig.raises {
            return Err(TypeError::Unsupported(format!(
                "a collection built by '{name}'s raising empty constructor"
            )));
        }
        Ok(constructor_symbol(
            name,
            "__init__",
            sigs.len() > 1,
            sig,
            self.self_instance_ty(name).as_ref(),
        ))
    }

    /// The call diagnostic of a constructor candidate whose availability
    /// clause fails for the constructed type, which is then no candidate.
    /// `method_arguments` bind the constructor's own parameters, which a
    /// struct still solving its own leaves to the per-call clone.
    fn unavailable_constructor(
        &self,
        name: &str,
        sig: &MethodSig,
        method_arguments: &[TyArg],
        struct_decls: &[ParamDecl],
        struct_arguments: &[TyArg],
    ) -> Option<TypeError> {
        if !struct_decls.is_empty() && !sig.decls.is_empty() {
            return None;
        }
        self.method_constraint_result(sig, method_arguments, struct_decls, struct_arguments)
            .err()
            .map(|failure| TypeError::BadCall {
                func: name.to_string(),
                reason: failure.reason(),
            })
    }

    /// The patterns and actuals a construction of `name` solves the
    /// struct's parameters from: its bound arguments', led by the struct at
    /// its own binders against `constructed`, the instance the construction
    /// builds when one is known before the overloads are matched.
    pub(super) fn constructed_unification(
        &self,
        name: &str,
        constructed: Option<&Ty>,
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> (Vec<Ty>, Vec<Ty>) {
        match constructed.zip(self.self_instance_ty(name)) {
            Some((constructed, instance)) => (
                std::iter::once(instance)
                    .chain(patterns.iter().cloned())
                    .collect(),
                std::iter::once(constructed.clone())
                    .chain(actuals.iter().cloned())
                    .collect(),
            ),
            None => (patterns.to_vec(), actuals.to_vec()),
        }
    }

    pub(super) fn infer_construction(
        &self,
        span: &SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        self.infer_construction_of(span, name, None, param_args, args, kwargs)
    }

    /// A construction of `name`, whose instance `constructed` names when an
    /// alias spells it without brackets (`Int32(x)` is `SIMD[DType.int32,
    /// 1](x)`): the instance binds the struct's parameters as an argument
    /// does, before the overloads are matched.
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn infer_construction_of(
        &self,
        span: &SourceSpan,
        name: &str,
        constructed: Option<&Ty>,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("constructor target '{name}' is not registered"))
        })?;
        // Origin slots are partitioned out of the application once, here:
        // every binder below sees origin-free arguments, and the explicit
        // origins are checked against the arguments binding those slots
        // once a constructor is selected.
        let mut partitioned =
            self.partition_struct_origin_args(name, &info.source_params, param_args)?;
        if let Some(instance) = constructed {
            bind_instance_origins(&mut partitioned, &info.source_params, instance);
        }
        let param_args: &[mojito_ast::ast::ParamArg] = &partitioned.forwarded;
        // An `ImmOrigin(o)` argument binds its slot immutably whatever the
        // source allows; the binding site keeps that fact for writes, and a
        // nested construction's casts ride along under their field.
        let mut immutable_binders = partitioned
            .explicit_origins
            .iter()
            .filter(|explicit| {
                explicit.mutability == Some(mojito_types::origin::Mutability::Immutable)
            })
            .map(|explicit| (Vec::new(), explicit.id))
            .collect::<Vec<_>>();
        immutable_binders.extend(self.nested_immutable_binders(info, args, kwargs));
        self.construction_immutable_binders
            .borrow_mut()
            .insert(span.clone(), immutable_binders);
        // The initializer's own list of an explicit `T.__init__[ks](…)`:
        // the construction's bracket is always the type's.
        let own_args = self.spelled_initializer_args_at(span);
        if own_args.is_empty()
            && !kwargs.is_empty()
            && args.is_empty()
            && kwargs.len() == 1
            && kwargs[0].name == "copy"
        {
            return self.infer_copy_construction(
                span,
                name,
                info,
                param_args,
                kwargs,
                &partitioned,
            );
        }
        // A hand-written `def __init__(out self, …)` is the constructor: check the
        // call arguments against its parameters (the `self` receiver is implicit).
        // Takes precedence over `@fieldwise_init`. On a **generic** struct, the type
        // parameters are solved by unifying `__init__`'s parameter types against the
        // argument types — exactly as the fieldwise path unifies field types.
        if let Some(sigs) = info.methods.get("__init__") {
            if info.decls.is_empty() {
                // A bracket on a struct without parameters is the pin's
                // "unexpected parameter".
                self.resolve_use_params(name, &[], param_args, &[], &[])?;
                let mut matches = Vec::new();
                // The reason the last candidate failed its origin binding, so a
                // miss caused by an explicit-origin or pointer-permission
                // mismatch reports that, not a bare miss.
                let mut origin_failure: Option<TypeError> = None;
                // The availability clause of the first candidate the
                // arguments would have selected, which the miss reports.
                let mut availability_failure: Option<TypeError> = None;
                for sig in sigs {
                    // A constructor with its own compile-time parameters
                    // (`__init__[T: Movable](out self, var value: T)`)
                    // instantiates them from the arguments like any generic
                    // method; the resolved arguments feed per-call
                    // specialization.
                    let Ok((
                        params,
                        variadic,
                        kw_variadic,
                        _,
                        method_arguments,
                        bracket_attributes,
                    )) = self.instantiate_method_generics(
                        &format!("{name}.__init__"),
                        sig,
                        &sig.params,
                        sig.variadic.as_ref(),
                        sig.kw_variadic.as_ref(),
                        &own_args,
                        args,
                        kwargs,
                    )
                    else {
                        continue;
                    };
                    // A `Pointer` parameter over the struct's own origin
                    // binder binds that binder from its argument before
                    // scoring.
                    let (params, origin_bindings) = match self.bind_constructor_pointer_params(
                        name,
                        sig,
                        params,
                        &partitioned.explicit_origins,
                        args,
                        kwargs,
                    )? {
                        Ok(bound) => bound,
                        Err(failure) => {
                            origin_failure = failure.or_else(|| origin_failure.take());
                            continue;
                        }
                    };
                    let instantiation = method_instantiation_arguments(sig, &method_arguments);
                    if let Ok(scored) = self.score_method_call(
                        sig,
                        &params,
                        variadic.as_ref(),
                        kw_variadic.as_ref(),
                        args,
                        kwargs,
                    ) {
                        if let Some(unavailable) =
                            self.unavailable_constructor(name, sig, &method_arguments, &[], &[])
                        {
                            availability_failure.get_or_insert(unavailable);
                            continue;
                        }
                        matches.push(MethodCallResolution {
                            conversion_score: scored.rank,
                            simd_erasures: scored.simd_erasures,
                            slots: scored.slots,
                            positional_overflow: scored.positional_overflow,
                            keyword_overflow: scored.keyword_overflow,
                            variadic_element: variadic,
                            variadic_convention: sig.variadic_convention,
                            variadic_name: sig.variadic_name.clone(),
                            keyword_element: kw_variadic,
                            conventions: sig.conventions.clone(),
                            self_convention: sig.self_convention,
                            return_type: self.constructed_type(
                                name,
                                Vec::new(),
                                &partitioned.tail,
                                &origin_bindings,
                            ),
                            result_adapter: None,
                            raises: sig.raises,
                            error: sig.error.clone(),
                            mutates_receiver: false,
                            consumes_receiver: false,
                            lowered_name: Some(constructor_symbol(
                                name,
                                "__init__",
                                sigs.len() > 1,
                                sig,
                                self.self_instance_ty(name).as_ref(),
                            )),
                            ref_params: sig.ref_params.clone(),
                            ref_return: None,
                            param_types: params,
                            param_decls: sig.decls.clone(),
                            parametric_origin_writes: sig.parametric_origin_writes.clone(),
                            instantiation,
                            parameter_names: sig.names.clone(),
                            view_return_interior: sig.view_return_interior.clone(),
                            view_return: Vec::new(),
                            declared_return: None,
                            declared_params: sig.params.clone(),
                            nested_origins: sig.nested_origins,
                            bracket_attributes,
                            refined_bound: None,
                        });
                    }
                }
                let selected =
                    select_method_overload("__init__", matches, None).map_err(|kind| {
                        constructor_miss(name, kind, origin_failure.as_ref(), availability_failure)
                    })?;
                if let Some(target) = &selected.lowered_name {
                    self.record_at(span.clone()).overload_target = Some(target.clone());
                }
                // The own list is no bracket of the construction: its
                // arguments reach MIR in the recorded instantiation alone.
                self.record_bracket_attributes(
                    span.clone(),
                    partitioned.brackets.scatter(Vec::new()),
                );
                self.record_construction_instantiation(
                    span,
                    name,
                    (&selected.param_decls, param_args),
                    &selected.parameter_names,
                    selected.instantiation.clone().unwrap_or_default(),
                );
                self.record_selected_method_conversions("__init__", &selected, args, kwargs)?;
                // Constructor calls use the same reference-parameter handles as
                // ordinary calls. Record their retained caller places after
                // overload selection so MIR does not have to inspect the
                // constructor declaration (and rejected candidates cannot leak
                // facts into the selected call).
                self.record_constructor_call(
                    span,
                    name,
                    &ConstructorBinding {
                        slots: &selected.slots,
                        conventions: &selected.conventions,
                        ref_params: &selected.ref_params,
                        collected: selected.gathered_arguments(),
                        raises: selected.raises,
                        error: selected.error.as_ref(),
                        exclusivity: ConstructorExclusivity {
                            callee: selected.return_type.to_string(),
                            parameter_names: &selected.parameter_names,
                            declared: selected.declared_params.clone(),
                            bound: selected.param_types.clone(),
                            nested_origins: selected.nested_origins,
                            gathered: self.method_collected_arguments(&selected, args),
                        },
                    },
                    args,
                    kwargs,
                )?;
                for (index, slot) in selected.slots.iter().enumerate() {
                    let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                        selected.conventions.get(index)
                    else {
                        continue;
                    };
                    let kind = consume_kind(*convention);
                    let argument = match slot {
                        ArgSlot::Positional(position) => &args[*position],
                        ArgSlot::Keyword(position) => &kwargs[*position].value,
                        ArgSlot::Default => continue,
                    };
                    // Contextual, against the slot this argument fills, as
                    // `record_selected_method_conversions` just typed it.
                    // Inferring bare here re-records the argument's semantic
                    // adjustment from scratch, and an occurrence holds one
                    // adjustment: a display argument that had just resolved
                    // to its parameter's collection (`ConstructCollection` for
                    // a `List[Int]` parameter) was overwritten by the display's
                    // own default (`ConstructArrayLiteral`), so a `var` or
                    // `deinit` parameter received — and a field then stored —
                    // an `Array` where a `List` was declared.
                    let ty = match selected.param_types.get(index) {
                        Some(expected) => self.infer_with_expected(argument, expected, true)?,
                        None => self.infer(argument)?,
                    };
                    self.check_consuming_as(
                        argument,
                        &ty,
                        &format!("argument {} to '{name}'", index + 1),
                        kind,
                    )?;
                }
                self.check_consuming_collected(
                    selected.variadic_convention == Some(ArgConvention::Var),
                    &selected.positional_overflow,
                    args,
                    &format!("'{name}'"),
                )?;
                return Ok(selected.return_type);
            }
            // A surplus positional argument falls through to the slot
            // matcher below, which refuses it; this path zips arguments with
            // parameters and would drop it.
            if sigs.len() == 1
                && kwargs.is_empty()
                && sigs[0].variadic.is_none()
                && sigs[0].refined_self.is_none()
                && args.len() <= sigs[0].params.len()
            {
                let sig = &sigs[0];
                let params = sig.params.clone();
                let decls = info.decls.clone();
                let explicit = self.explicit_type_subst(name, &decls, param_args)?;
                let arg_tys = args
                    .iter()
                    .zip(&params)
                    .map(|(argument, pattern)| {
                        self.solving_argument_ty(argument, pattern, &decls, &explicit)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Self::reject_unconstrained_pack(name, &decls, param_args, constructed, &params)?;
                let (instance_patterns, instance_actuals) =
                    self.constructed_unification(name, constructed, &params, &arg_tys);
                let BoundParameters {
                    mut subst,
                    arguments: tyargs,
                    attributes,
                } = self.resolve_use_params(
                    name,
                    &decls,
                    param_args,
                    &instance_patterns,
                    &instance_actuals,
                )?;
                self.record_bracket_attributes(
                    span.clone(),
                    partitioned.brackets.scatter(attributes),
                );
                if let Some(specialized) =
                    self.finish_variadic_construction(span, name, param_args, &tyargs, args, kwargs)
                {
                    return specialized;
                }
                unify_through_callable_bounds(&sig.decls, &mut subst)?;
                if let Some(unavailable) =
                    self.unavailable_constructor(name, sig, &[], &decls, &tyargs)
                {
                    return Err(unavailable);
                }
                let bound_slots: Vec<(usize, &Expr, &Ty)> = args
                    .iter()
                    .zip(&params)
                    .enumerate()
                    .map(|(index, (expression, pattern))| (index, expression, pattern))
                    .collect();
                let pointer_origins = self.bind_constructor_origins(
                    name,
                    "__init__",
                    &info.source_params,
                    sig,
                    &bound_slots,
                    &arg_tys,
                    &partitioned.explicit_origins,
                )?;
                self.record_at(span.clone()).overload_target = Some(constructor_symbol(
                    name,
                    "__init__",
                    false,
                    sig,
                    self.self_instance_ty(name).as_ref(),
                ));
                let values = solved_value_bindings(&decls, &tyargs);
                let struct_subst = struct_binders_only(&subst, sig);
                // The struct's own origin binders are bound before its type
                // parameters, so an argument type's binder of the same slot
                // number (an enclosing clone's) stays as it is.
                let struct_bound = params
                    .iter()
                    .take(args.len())
                    .map(|pty| {
                        self.constructor_parameter_ty(
                            &pointer_origins.substitute(&expand_bound_pack(pty, &decls, &tyargs)),
                            &struct_subst,
                            &values,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let own = self.solve_constructor_own_params(
                    name,
                    sig,
                    (&own_args, &struct_subst),
                    struct_bound.iter().cloned().zip(arg_tys.iter().cloned()),
                )?;
                self.record_construction_instantiation(
                    span,
                    name,
                    (&decls, param_args),
                    &sig.names,
                    tyargs
                        .iter()
                        .cloned()
                        .chain(own.arguments.clone())
                        .collect(),
                );
                self.record_struct_instantiation(name, &tyargs, span.source.as_deref());
                let mut bound = Vec::with_capacity(args.len());
                for (i, pty) in struct_bound.iter().enumerate() {
                    let expected = own.apply(self, pty)?;
                    // Checked against its solved parameter, last: a display
                    // argument constructs that parameter's collection.
                    let actual = self
                        .infer_with_expected(&args[i], &expected, true)
                        .map_err(|error| {
                            let display = match &args[i].kind {
                                ExprKind::ListLit(_) => "list literal",
                                ExprKind::BraceLit(_) => "dictionary or set literal",
                                _ => return error,
                            };
                            TypeError::TypeMismatch {
                                expected: expected.to_string(),
                                found: display.to_string(),
                                context: format!(
                                    "argument '{}' to '{name}.__init__'",
                                    sig.names[i]
                                ),
                            }
                        })?;
                    if coerces(&actual, &expected) {
                        // A literal argument materializes to the solved
                        // parameter type exactly as it does for a non-generic
                        // constructor (`Box[Float64](2.5)` stores a Float64,
                        // not the exact literal).
                        self.record_literal_materializations(&args[i], &actual, &expected)?;
                    } else if !self.record_constructor_conversion(&args[i], &actual, &expected)? {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: actual.to_string(),
                            context: format!("argument {} to '{}.__init__'", i + 1, name),
                        });
                    }
                    if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                        sig.conventions.get(i)
                    {
                        let kind = consume_kind(*convention);
                        self.check_consuming_as(
                            &args[i],
                            &actual,
                            &format!("argument {} to '{}'", i + 1, name),
                            kind,
                        )?;
                    }
                    bound.push(expected);
                }
                let slots = (0..args.len()).map(ArgSlot::Positional).collect::<Vec<_>>();
                let constructed =
                    self.constructed_type(name, tyargs, &partitioned.tail, &pointer_origins);
                let exclusivity = ConstructorExclusivity {
                    callee: constructed.to_string(),
                    parameter_names: &sig.names,
                    declared: params
                        .iter()
                        .map(|parameter| pointer_origins.substitute(parameter))
                        .collect(),
                    bound,
                    nested_origins: sig.nested_origins,
                    gathered: self.signature_collected_arguments(sig, &[], args, |element| {
                        Ok(element.clone())
                    }),
                };
                self.record_constructor_call(
                    span,
                    name,
                    &ConstructorBinding::of_signature(sig, &slots, &[], exclusivity),
                    args,
                    kwargs,
                )?;
                return Ok(constructed);
            }
            let decls = info.decls.clone();
            let explicit = self.explicit_type_subst(name, &decls, param_args)?;
            let overloaded = sigs.len() > 1;
            let keyword_names: Vec<&str> = kwargs.iter().map(|k| k.name.as_str()).collect();
            // Map arguments (positional and keyword) to each candidate's
            // parameter slots with the shared structural matcher, then solve
            // the struct's generic parameters from the bound slots.
            let mut matches = Vec::new();
            // The reason the last candidate failed its origin binding, so a
            // call that matches no constructor because of an explicit-origin
            // or pointer-permission mismatch reports that, not a bare miss.
            let mut origin_failure: Option<TypeError> = None;
            // The first candidate whose struct-parameter bound the arguments
            // violate (`Dict[Key, Int]` over a non-`Hashable` key), so the
            // miss names the bound instead of a bare "no overload".
            let mut bound_failure: Option<TypeError> = None;
            // The availability clause of the first candidate the arguments
            // would have selected.
            let mut availability_failure: Option<TypeError> = None;
            for sig in sigs {
                let Ok(matched) = mojito_ast::call::match_call_slots(
                    &sig.names,
                    &sig.required,
                    sig.positional_only,
                    sig.keyword_only,
                    args.len(),
                    &keyword_names,
                    mojito_ast::call::CallVariadics {
                        positional: sig.variadic.is_some(),
                        keyword: sig.kw_variadic.is_some(),
                    },
                ) else {
                    continue;
                };
                // Keyword collectors (`**kwargs`) are not an explicit-call
                // surface yet. A positional `*values` collector IS: overflow
                // arguments bind to the variadic element so they type-check and
                // feed generic inference (a bare `Bag(1, 2)` solves `T`).
                if !matched.keyword_overflow.is_empty() {
                    continue;
                }
                let forwarded = match self.constructor_forwarded_pack(name, sig, args, &matched) {
                    Ok(forwarded) => forwarded,
                    Err(failure) => {
                        bound_failure = bound_failure.or(failure);
                        continue;
                    }
                };
                let mut bound: Vec<(&Expr, Ty, Option<ArgConvention>)> = Vec::new();
                let mut bound_slots: Vec<(usize, &Expr, &Ty)> = Vec::new();
                for (index, slot) in matched.slots.iter().enumerate() {
                    let expression = match slot {
                        ArgSlot::Positional(position) => &args[*position],
                        ArgSlot::Keyword(position) => &kwargs[*position].value,
                        ArgSlot::Default => continue,
                    };
                    bound_slots.push((index, expression, &sig.params[index]));
                    bound.push((
                        expression,
                        sig.params[index].clone(),
                        sig.conventions.get(index).copied().flatten(),
                    ));
                }
                if let Some(element) = sig.variadic.as_ref() {
                    // Each overflow argument scores against the variadic element
                    // (its convention is applied at the `^` transfer site, not
                    // here — mirroring the `decls.is_empty()` variadic path).
                    for position in &matched.positional_overflow {
                        bound.push((&args[*position], element.clone(), None));
                    }
                }
                // The forwarded pack is the one overflow argument, so the
                // last bound.
                let forwarded_index = forwarded.as_ref().map(|_| bound.len() - 1);
                let arg_tys = bound
                    .iter()
                    .enumerate()
                    .map(|(index, (expression, pattern, _))| match &forwarded {
                        Some(actual) if Some(index) == forwarded_index => Ok(actual.clone()),
                        _ => self.solving_argument_ty(expression, pattern, &decls, &explicit),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let patterns: Vec<Ty> = bound
                    .iter()
                    .map(|(_, pattern, _)| pattern.clone())
                    .collect();
                let (instance_patterns, instance_actuals) =
                    self.constructed_unification(name, constructed, &patterns, &arg_tys);
                let mut refined_own = None;
                let resolved_use = Self::reject_unconstrained_pack(
                    name,
                    &decls,
                    param_args,
                    constructed,
                    &patterns,
                )
                .and_then(|()| match &sig.refined_self {
                    Some(refined) => {
                        let bound = patterns
                            .iter()
                            .zip(&arg_tys)
                            .enumerate()
                            .filter(|(index, _)| Some(*index) != forwarded_index)
                            .map(|(_, (pattern, actual))| (pattern.clone(), actual.clone()))
                            .collect::<Vec<_>>();
                        self.solve_refined_construction(
                            name,
                            (sig, &own_args),
                            refined,
                            param_args,
                            constructed,
                            &bound,
                        )
                        .map(|(parameters, own)| {
                            refined_own = Some(own);
                            parameters
                        })
                    }
                    None => self.resolve_use_params(
                        name,
                        &decls,
                        param_args,
                        &instance_patterns,
                        &instance_actuals,
                    ),
                });
                // A value spelled where the struct takes a type
                // (`Tuple[Int, Tuple[3]]()`) is wrong whichever constructor
                // the arguments select.
                if let Err(
                    error @ TypeError::TypeMismatch {
                        expected, found, ..
                    },
                ) = &resolved_use
                    && expected == "a type"
                    && found == "a value"
                {
                    return Err(error.clone());
                }
                if let Err(error) = &resolved_use
                    && (sig.refined_self.is_some()
                        || matches!(
                            error,
                            TypeError::TraitNotSatisfied { .. }
                                | TypeError::CannotInferTypeParam { .. }
                        ))
                    && bound_failure.is_none()
                {
                    bound_failure = Some(error.clone());
                }
                if let Ok(BoundParameters {
                    subst,
                    arguments: tyargs,
                    attributes,
                }) = resolved_use
                {
                    self.record_bracket_attributes(
                        span.clone(),
                        partitioned.brackets.scatter(attributes),
                    );
                    let pointer_origins = match self.bind_constructor_origins(
                        name,
                        "__init__",
                        &info.source_params,
                        sig,
                        &bound_slots,
                        &arg_tys[..bound_slots.len()],
                        &partitioned.explicit_origins,
                    ) {
                        Ok(bindings) => bindings,
                        Err(error) => {
                            origin_failure = Some(error);
                            continue;
                        }
                    };
                    // The struct's binders only: the constructor's own are
                    // solved apart, against these.
                    let bindings = AssocBindings {
                        types: struct_binders_only(&subst, sig),
                        values: solved_value_bindings(&decls, &tyargs),
                        ids: mojito_types::param_expr::ParamBindings::new(),
                        origins: HashMap::new(),
                    };
                    let own = match refined_own.take().map_or_else(
                        || {
                            self.solve_constructor_own_params(
                                name,
                                sig,
                                (&own_args, &bindings.types),
                                patterns
                                    .iter()
                                    .zip(&arg_tys)
                                    .enumerate()
                                    .filter(|(index, _)| Some(*index) != forwarded_index)
                                    .map(|(_, (pattern, actual))| {
                                        (substitute_assoc(pattern, &bindings), actual.clone())
                                    }),
                            )
                        },
                        Ok,
                    ) {
                        Ok(own) => own,
                        Err(error) => {
                            bound_failure.get_or_insert(error);
                            continue;
                        }
                    };
                    let mut score = 0;
                    let mut ok = true;
                    let mut expected_tys = Vec::with_capacity(bound.len());
                    // The k-th argument a `*args: *Self.Ts` collector took
                    // is expected at the k-th element of the pack it solved;
                    // a regular slot's pattern expands its spreads of the
                    // solved pack. A collector over an explicitly bound pack
                    // (`Bag[Int, String](1)`) must take exactly its elements.
                    let pack_elements = bound_pack_elements(&decls, &tyargs);
                    if !collector_takes_bound_pack(
                        sig,
                        pack_elements.as_ref(),
                        matched.positional_overflow.len(),
                    ) {
                        continue;
                    }
                    for (index, pty) in patterns
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| Some(*index) != forwarded_index)
                    {
                        let expected = index
                            .checked_sub(bound_slots.len())
                            .filter(|_| {
                                mojito_types::types::pack_spread(std::slice::from_ref(pty))
                                    .is_some()
                            })
                            .and_then(|position| {
                                pack_elements
                                    .as_ref()
                                    .and_then(|(_, elements)| elements.get(position))
                            })
                            .cloned()
                            .map_or_else(
                                || {
                                    own.apply(
                                        self,
                                        &substitute_assoc(
                                            &pointer_origins.substitute(&expand_bound_pack(
                                                pty, &decls, &tyargs,
                                            )),
                                            &bindings,
                                        ),
                                    )
                                },
                                Ok,
                            )?;
                        // Scored against the solved parameter, as the
                        // commit below checks it: a display argument is that
                        // parameter's collection.
                        let Ok(actual) = self.infer_with_expected(bound[index].0, &expected, false)
                        else {
                            ok = false;
                            break;
                        };
                        if coerces(&actual, &expected) {
                            if actual != expected {
                                score += CONVERSION_UNIT;
                            }
                        } else if self
                            .implicit_conversion_target(&actual, &expected)?
                            .is_some()
                        {
                            // An implicit conversion ranks below any direct
                            // coercion so exact overloads keep winning.
                            score += 2 * CONVERSION_UNIT;
                        } else {
                            ok = false;
                            break;
                        }
                        expected_tys.push((index, expected));
                    }
                    if ok
                        && let Some(unavailable) =
                            self.unavailable_constructor(name, sig, &[], &decls, &tyargs)
                    {
                        availability_failure.get_or_insert(unavailable);
                        continue;
                    }
                    if ok {
                        // An initializer's own compile-time parameters rank it
                        // exactly as a free function's do: at equal conversion
                        // cost the shorter parameter signature wins, so
                        // `Int8(Float64(3.9))` selects the `[other_dtype]` cast
                        // over `[T: Intable, target_dtype]`, and a variadic
                        // candidate loses to a fixed-arity one. The implicit
                        // copies a place costs a `var` parameter count here too,
                        // so `C[Int](s)` ranks as `C(s)` does; this path has no
                        // trivially-register-passable classification, so the
                        // binding's variadic tie-breaks stay out of it.
                        let mut binding = ArgumentBinding::new(None);
                        for (expression, _, convention) in &bound {
                            binding.bind(*convention, false, expression);
                        }
                        let rank = overload_rank(
                            score,
                            sig.variadic.is_some(),
                            baked_decl_count(&sig.decls),
                            !decls_are_concrete(&sig.decls),
                        ) + binding.rank();
                        matches.push(GenericConstructorMatch {
                            rank,
                            sig: sig.clone(),
                            tyargs,
                            own_arguments: own.arguments,
                            expected: expected_tys,
                            slots: matched.slots,
                            overflow: matched.positional_overflow,
                            subst,
                            pointer_origins,
                        });
                    }
                }
            }
            let best = matches.iter().map(|candidate| candidate.rank).min();
            if let Some(best) = best {
                let mut best_matches = matches
                    .into_iter()
                    .filter(|candidate| candidate.rank == best)
                    .collect::<Vec<_>>();
                if best_matches.len() != 1 {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "ambiguous overloaded constructor call".to_string(),
                    });
                }
                let GenericConstructorMatch {
                    sig,
                    tyargs,
                    own_arguments,
                    expected,
                    slots,
                    overflow,
                    mut subst,
                    pointer_origins,
                    ..
                } = best_matches.remove(0);
                unify_through_callable_bounds(&sig.decls, &mut subst)?;
                if let Some(specialized) =
                    self.finish_variadic_construction(span, name, param_args, &tyargs, args, kwargs)
                {
                    return specialized;
                }
                self.record_struct_instantiation(name, &tyargs, span.source.as_deref());
                // Rebuild the packed bound used during scoring: regular slots in
                // slot order (skipping defaults), then variadic overflow. The
                // `expected` indices address into this same layout. Overflow
                // carries no consuming convention here (see the scoring note).
                let mut bound: Vec<(&Expr, Option<ArgConvention>)> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, slot)| {
                        let expression = match slot {
                            ArgSlot::Positional(position) => &args[*position],
                            ArgSlot::Keyword(position) => &kwargs[*position].value,
                            ArgSlot::Default => return None,
                        };
                        Some((expression, sig.conventions.get(index).copied().flatten()))
                    })
                    .collect();
                // A forwarded pack was bound whole while scoring, its
                // ownership checked there.
                bound.extend(
                    overflow
                        .iter()
                        .map(|position| &args[*position])
                        .filter(|argument| self.forwarded_pack(argument).is_none())
                        .map(|argument| (argument, sig.variadic_convention)),
                );
                // Each argument's last inference is the contextual one against
                // its solved parameter, so a display keeps that collection.
                for (index, expected) in &expected {
                    let Some(&(expression, convention)) = bound.get(*index) else {
                        continue;
                    };
                    let actual = self.infer_with_expected(expression, expected, true)?;
                    if coerces(&actual, expected) {
                        self.record_literal_materializations(expression, &actual, expected)?;
                    } else if !self.record_implicit_conversion(expression, &actual, expected)? {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: actual.to_string(),
                            context: format!("argument {} to '{name}.__init__'", index + 1),
                        });
                    }
                    if let Some(convention @ (ArgConvention::Var | ArgConvention::Deinit)) =
                        convention
                    {
                        self.check_consuming_as(
                            expression,
                            &actual,
                            &format!("argument {} to '{}'", index + 1, name),
                            consume_kind(convention),
                        )?;
                    }
                }
                // The template overload the source spelled.
                self.record_at(span.clone()).overload_target = Some(constructor_symbol(
                    name,
                    "__init__",
                    overloaded,
                    &sig,
                    self.self_instance_ty(name).as_ref(),
                ));
                self.record_construction_instantiation(
                    span,
                    name,
                    (&decls, param_args),
                    &sig.names,
                    tyargs.iter().cloned().chain(own_arguments).collect(),
                );
                // `expected` addresses the packed layout: the non-default
                // slots in order, then the gathered arguments.
                let expected_at = |packed: usize| {
                    expected
                        .iter()
                        .find(|(index, _)| *index == packed)
                        .map(|(_, ty)| ty.clone())
                };
                let mut packed = 0;
                let bound_params = slots
                    .iter()
                    .enumerate()
                    .map(|(index, slot)| {
                        if matches!(slot, ArgSlot::Default) {
                            return sig.params[index].clone();
                        }
                        packed += 1;
                        expected_at(packed - 1).unwrap_or_else(|| sig.params[index].clone())
                    })
                    .collect();
                let gathered = overflow
                    .iter()
                    .copied()
                    .filter(|position| self.forwarded_pack(&args[*position]).is_none())
                    .collect::<Vec<_>>();
                // A homogeneous collector binds every gathered argument to
                // the one solved element.
                let resolve =
                    |element: &Ty| Ok(expected_at(packed).unwrap_or_else(|| element.clone()));
                let constructed =
                    self.constructed_type(name, tyargs, &partitioned.tail, &pointer_origins);
                let exclusivity = ConstructorExclusivity {
                    callee: constructed.to_string(),
                    parameter_names: &sig.names,
                    declared: sig
                        .params
                        .iter()
                        .map(|parameter| pointer_origins.substitute(parameter))
                        .collect(),
                    bound: bound_params,
                    nested_origins: sig.nested_origins,
                    gathered: self.signature_collected_arguments(&sig, &gathered, args, resolve),
                };
                self.record_constructor_call(
                    span,
                    name,
                    &ConstructorBinding::of_signature(&sig, &slots, &overflow, exclusivity),
                    args,
                    kwargs,
                )?;
                return Ok(constructed);
            }
            return Err(constructor_miss(
                name,
                OverloadSelect::NoMatch,
                bound_failure.or(origin_failure).as_ref(),
                availability_failure,
            ));
        }
        if info.methods.contains_key("__init__") {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: info
                    .methods
                    .get("__init__")
                    .and_then(|sigs| sigs.first())
                    .map_or(0, |sig| sig.params.len()),
                got: args.len(),
            });
        }
        if !info.fieldwise_init {
            return Err(TypeError::NoConstructor(name.to_string()));
        }
        // The synthesized constructor has no parameters of its own.
        self.resolve_use_params(&format!("{name}.__init__"), &[], &own_args, &[], &[])?;
        // The synthesized constructor names its parameters after the fields:
        // keyword arguments bind by name, and the fieldwise tail below sees
        // the arguments in field order.
        let ordered: Vec<Expr>;
        let args = if kwargs.is_empty() {
            args
        } else {
            ordered = fieldwise_arguments(name, info, args, kwargs)?;
            ordered.as_slice()
        };
        let decls = info.decls.clone();
        let explicit = self.explicit_type_subst(name, &decls, param_args)?;
        let field_tys: Vec<Ty> = info.fields.iter().map(|(_, t)| t.clone()).collect();
        if field_tys.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: field_tys.len(),
                got: args.len(),
            });
        }
        let arg_tys = args
            .iter()
            .zip(&field_tys)
            .map(|(argument, field)| {
                if self.type_contains_reference(field) {
                    self.infer_storage_value(argument, field)
                } else if matches!(field.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) {
                    // Callable fields keep uncontextualized inference: the
                    // storage rule below must still see a capturing closure's
                    // full environment rather than an adapted contract.
                    self.infer(argument)
                } else if mojito_types::types::is_symbolic(field) {
                    // A field naming the struct's binders: the argument is
                    // checked against the solved field below.
                    self.solving_argument_ty(argument, field, &decls, &explicit)
                } else {
                    // Contextual: a display argument checks its elements
                    // against the field's element types, so a string literal
                    // converts where a nominal String element is expected.
                    self.infer_with_expected(argument, field, true)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        // The origin slots were partitioned out of `param_args` above; the
        // field types bind the struct's origin binders from the arguments.
        Self::reject_unconstrained_pack(name, &decls, param_args, constructed, &field_tys)?;
        let (instance_patterns, instance_actuals) =
            self.constructed_unification(name, constructed, &field_tys, &arg_tys);
        let BoundParameters {
            subst,
            arguments: tyargs,
            attributes,
        } = self.resolve_use_params(
            name,
            &decls,
            param_args,
            &instance_patterns,
            &instance_actuals,
        )?;
        self.record_bracket_attributes(span.clone(), partitioned.brackets.scatter(attributes));
        if let Some(specialized) =
            self.finish_variadic_construction(span, name, param_args, &tyargs, args, &[])
        {
            return specialized;
        }
        // The construction calls the synthesized fieldwise `__init__`
        // (`CheckedProgram::synthesizes_fieldwise_constructor`), whose
        // parameters bind the field-ordered arguments by its conventions
        // (bound below, once each field's type is solved).
        self.record_at(span.clone()).overload_target = Some(format!("{name}.__init__"));
        let field_names: Vec<String> = info.fields.iter().map(|(name, _)| name.clone()).collect();
        self.record_construction_instantiation(
            span,
            name,
            (&decls, param_args),
            &field_names,
            tyargs.clone(),
        );
        let origin_bindings = self.bind_fieldwise_origins(
            name,
            &info.source_params,
            &info.fields,
            args,
            &arg_tys,
            &partitioned.explicit_origins,
        )?;
        // A field type names the struct's own value parameters; the resolved
        // application binds them beside its type parameters.
        let bindings = AssocBindings {
            types: subst,
            values: solved_value_bindings(&decls, &tyargs),
            ids: mojito_types::param_expr::ParamBindings::new(),
            origins: HashMap::new(),
        };
        // The type each synthesized parameter binds: a `ref` field's is its
        // referent, as a written `ref [Self.o] field: T` parameter's is.
        let parameter_ty = |ty: Ty| match ty.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => ty,
        };
        let mut bound = Vec::with_capacity(args.len());
        for (i, (aty, fty)) in arg_tys.iter().zip(&field_tys).enumerate() {
            // Origin binders first, as for a constructor's parameters.
            let expected = substitute_assoc(
                &origin_bindings.substitute(&expand_bound_pack(fty, &decls, &tyargs)),
                &bindings,
            );
            let contextual;
            let aty = if !mojito_types::types::is_symbolic(fty)
                || self.type_contains_reference(fty)
                || matches!(fty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
            {
                aty
            } else {
                contextual = self.infer_with_expected(&args[i], &expected, true)?;
                &contextual
            };
            if Self::storage_value_coerces(aty, &expected) {
                self.record_literal_materializations(&args[i], aty, &expected)?;
            } else if !self.record_constructor_conversion(&args[i], aty, &expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: aty.to_string(),
                    context: format!("field {} of '{}'", i + 1, name),
                });
            }
            bound.push(parameter_ty(expected.clone()));
            // A `ref` field's argument binds its place, as the parameter's
            // `ref` convention retains it ([`Self::record_constructor_call`]).
            if matches!(expected.kind(), TyKind::Ref(_)) {
                continue;
            }
            if self.type_contains_reference(&expected) {
                self.mark_reference_storage_uses(&args[i], &expected);
            }
            // A constructor stores each argument in a field by value — a consuming
            // position.
            self.check_consuming(&args[i], aty, &format!("field {} of '{}'", i + 1, name))?;
        }
        let constructed = self.constructed_type(name, tyargs, &partitioned.tail, &origin_bindings);
        let conventions = self.fieldwise_conventions(&info.fields);
        let ref_params = Self::fieldwise_ref_params(info);
        let slots: Vec<ArgSlot> = (0..args.len()).map(ArgSlot::Positional).collect();
        let exclusivity = ConstructorExclusivity {
            callee: constructed.to_string(),
            parameter_names: &field_names,
            declared: field_tys
                .iter()
                .map(|ty| parameter_ty(origin_bindings.substitute(ty)))
                .collect(),
            bound,
            nested_origins: NestedOrigins::AsDeclared,
            gathered: CollectedArguments::none(),
        };
        self.record_constructor_call(
            span,
            name,
            &ConstructorBinding::fieldwise(&slots, &conventions, &ref_params, exclusivity),
            args,
            &[],
        )?;
        Ok(constructed)
    }

    /// The values a spread of a type list binds a pack to: one type per
    /// element once every length in the list is known, and the list itself,
    /// as the one spread type, until then.
    fn spread_pack_values(&self, operand: &Expr) -> Result<Vec<CtValue>, TypeError> {
        let list = self.type_list_operand(operand)?.ok_or_else(|| {
            TypeError::Unsupported("a spread type argument takes a pack or a type list".to_string())
        })?;
        mojito_types::types::spread_arguments(mojito_types::types::TUPLE_TYPE_NAME, &list)
            .map_err(param_error)
            .map(|arguments| {
                arguments
                    .into_iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(ty) => Some(CtValue::Type(ty)),
                        TyArg::Val(_) | TyArg::Origin(_) => None,
                    })
                    .collect()
            })
    }

    /// Resolve a generic use site's parameters, returning a type-parameter
    /// substitution and the full argument list (types + values) for the struct's
    /// identity. When `param_args` is non-empty the parameters are supplied
    /// explicitly (positionally), and the parameters they leave open are
    /// inferred; otherwise every parameter is inferred from
    /// `patterns`/`actuals` (a value parameter where it occupies an argument
    /// type's slot).
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn resolve_use_params(
        &self,
        name: &str,
        decls: &[ParamDecl],
        param_args: &[mojito_ast::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<BoundParameters, TypeError> {
        let mut subst: TySubst = TySubst::new();
        if decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok(BoundParameters {
                subst,
                arguments: Vec::new(),
                attributes: Vec::new(),
            });
        }
        if !param_args.is_empty() {
            let bound = bracket_bindings(name, decls, param_args)?;
            let mut attributes: BracketAttributes = vec![None; param_args.len()];
            // Infer parameters hidden before `//` even when later parameters
            // were supplied explicitly (`hash[Fnv1a](value)` infers `T` from
            // `value` while binding the explicit hasher argument). A pack
            // pattern (`*args: *Self.Ts`) is bound from the arguments above,
            // each element checked against its bound there; unifying it
            // would tie the pack to the first collected argument.
            for (pattern, actual) in patterns.iter().zip(actuals) {
                if matches!(pattern.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*'))
                {
                    continue;
                }
                unify(pattern, actual, &mut subst)?;
            }
            // A value binder no bracket argument spells is solved from the
            // argument types as in an inferred call (`bitcast[DType.uint8,
            // 4](x)` solves `src_dtype` and `src_width` from `x`).
            let mut value_solutions = HashMap::new();
            for (pattern, actual) in patterns.iter().zip(actuals) {
                solve_value_args(pattern, actual, &mut value_solutions);
            }
            self.solve_value_binders_from_brackets(decls, &bound, &mut value_solutions);
            unify_through_callable_bounds(decls, &mut subst)?;
            self.solve_binders_from_values(decls, &mut subst, |index, declared| {
                self.bound_value_ty(&bound[index], declared)
            })?;
            let trait_solutions = self.solve_trait_binders_from_lists(
                decls,
                |index| bound[index].is_empty(),
                |index, declared| self.bound_value_ty(&bound[index], declared),
            )?;
            let mut tyargs = Vec::with_capacity(decls.len());
            let mut value_environment = HashMap::new();
            for (decl, arguments) in decls.iter().zip(bound) {
                let typed = substituted_value_decl(decl, &subst);
                let typed = with_earlier_arguments(typed.as_ref().unwrap_or(decl), decls, &tyargs)
                    .or(typed);
                let decl = typed.as_ref().unwrap_or(decl);
                let variadic = matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                );
                if variadic {
                    // A pack no bracket argument binds is inferred from the
                    // runtime arguments, as Mojo binds `scaled[3](7, "x")`'s
                    // `*Us` beside the explicit `n`.
                    if arguments.is_empty()
                        && let ParamDecl::Type { id, .. } = decl
                    {
                        let argument = self.collected_pack_argument(id, patterns, actuals);
                        match &argument {
                            TyArg::Val(value) => {
                                value_environment.insert(
                                    decl.name().trim_start_matches('*').to_string(),
                                    value.clone(),
                                );
                            }
                            TyArg::Ty(pack) => {
                                subst.insert(id.clone(), pack.clone());
                            }
                            TyArg::Origin(_) => {}
                        }
                        tyargs.push(argument);
                        continue;
                    }
                    // A spread of a value pack that is still a parameter
                    // (`PL[*vs]`, `PL[*Self.vs]`) forwards that pack whole,
                    // and a spread `ParameterList` construction its raw list
                    // (`It[*ParameterList[Self.values]()]`).
                    if let [(position, argument)] = arguments.as_slice()
                        && matches!(decl, ParamDecl::Value { .. })
                        && let Some(list) = self
                            .value_pack_spread(argument)
                            .map(|pack| Ok(CtValue::Expr(pack)))
                            .or_else(|| {
                                self.parameter_list_spread(argument)
                                    .map(|spread| spread.map(|(_, raw)| raw))
                            })
                    {
                        let value = TyArg::Val(list?);
                        // The spread construction is compile-time only: the
                        // list it binds is its attribute.
                        attributes[*position] = match argument {
                            mojito_ast::ast::ParamArg::Value(spread) => {
                                self.record_at(spread.source_span()).argument_attribute =
                                    Some(value.clone());
                                Some(value.clone())
                            }
                            _ => bracket_attribute(argument, &value),
                        };
                        if let TyArg::Val(value) = &value {
                            value_environment.insert(
                                decl.name().trim_start_matches('*').to_string(),
                                value.clone(),
                            );
                        }
                        tyargs.push(value);
                        continue;
                    }
                    // Each argument binds one value of the pack's element,
                    // as upstream checks a variadic argument against its
                    // list's `getParameterListInfo().elementType`.
                    let element = decl
                        .value_element_ty()
                        .map(|element| decl.with_value_type(element.clone()));
                    let element = element.as_ref().unwrap_or(decl);
                    let mut values = Vec::with_capacity(arguments.len());
                    for (position, argument) in arguments {
                        // A spread of a computed list binds its elements,
                        // or the list itself while a length in it is open.
                        if let mojito_ast::ast::ParamArg::Value(Expr {
                            kind: ExprKind::Spread(operand),
                            ..
                        }) = argument
                        {
                            values.extend(self.spread_pack_values(operand)?);
                            continue;
                        }
                        let resolved = self.resolve_param_arg(element, argument)?;
                        attributes[position] = self.bound_bracket_attribute(argument, &resolved);
                        values.push(resolved.ct_value().ok_or_else(|| {
                            TypeError::Unsupported(
                                "an origin argument cannot bind a type or value parameter"
                                    .to_string(),
                            )
                        })?);
                    }
                    // A spread of a pack that is still a parameter forwards
                    // that pack whole, as `Self`'s own argument spells it.
                    let spread: Vec<Ty> = values
                        .iter()
                        .filter_map(|value| match value {
                            CtValue::Type(ty) => Some(ty.clone()),
                            _ => None,
                        })
                        .collect();
                    Self::reject_mixed_spread(name, &spread)?;
                    if let [CtValue::Type(pack)] = values.as_slice()
                        && (mojito_types::types::pack_spread(std::slice::from_ref(pack)).is_some()
                            || mojito_types::types::list_spread(pack).is_some())
                    {
                        if let ParamDecl::Type { id, .. } = decl {
                            subst.insert(id.clone(), (*pack).clone());
                        }
                        tyargs.push(TyArg::Ty((*pack).clone()));
                        continue;
                    }
                    let value = CtValue::Tuple(values);
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                    tyargs.push(TyArg::Val(value));
                    continue;
                }
                let tyarg = if let Some((position, argument)) = arguments.first() {
                    let resolved = self.resolve_param_arg(decl, argument)?;
                    attributes[*position] = self.bound_bracket_attribute(argument, &resolved);
                    resolved
                } else if let Some(solved) = trait_solutions.get(decl.id()) {
                    TyArg::Val(solved.clone())
                } else if let ParamDecl::Value { name: pname, .. } = decl
                    && let Some(solved) = value_solutions.get(pname)
                {
                    TyArg::Val(solved.clone())
                } else if let ParamDecl::Value {
                    callable_default: Some(_),
                    ..
                } = decl
                {
                    // The VM evaluates the symbolic default after reifying all
                    // preceding scalar/callable parameters.  Generic identity
                    // records only that this runtime value occupies the slot.
                    TyArg::Val(CtValue::Deferred(decl.binder()))
                } else if let ParamDecl::Value {
                    default: Some(value),
                    ty,
                    ..
                } = decl
                {
                    TyArg::Val(default_argument(
                        &self.param_context,
                        decls,
                        value,
                        &value_environment,
                        ty,
                        decl.name(),
                    )?)
                } else if let ParamDecl::Type { id, .. } = decl
                    && let Some(inferred) = subst.get(id)
                {
                    TyArg::Ty(inferred.clone())
                } else if let ParamDecl::Type {
                    default: Some(ty), ..
                } = decl
                {
                    TyArg::Ty((*ty).clone())
                } else {
                    return Err(TypeError::CannotInferTypeParam {
                        name: name.to_string(),
                        param: decl.name().to_string(),
                    });
                };
                if let (ParamDecl::Type { id, .. }, TyArg::Ty(t)) = (decl, &tyarg) {
                    subst.insert(id.clone(), t.clone());
                }
                tyargs.push(tyarg);
                if let Some(TyArg::Val(value)) = tyargs.last() {
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                }
            }
            self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
            self.validate_trait_parameter_bounds(name, decls, &tyargs)?;
            self.validate_generic_constraints(name, decls, &tyargs)?;
            return Ok(BoundParameters {
                subst,
                arguments: tyargs,
                attributes,
            });
        }
        // Inference: type parameters (and value parameters occupying a solved
        // argument slot, `Array[T, length]` against `Array[Int, 3]`) from the
        // argument types.
        let mut value_solutions = HashMap::new();
        for (pat, act) in patterns.iter().zip(actuals) {
            solve_value_args(pat, act, &mut value_solutions);
        }
        solve_value_binders_from_defaults(decls, &mut value_solutions);
        for (pat, act) in patterns.iter().zip(actuals) {
            if let TyKind::Param { binder, bounds, .. } = pat.kind()
                && binder.name.starts_with('*')
            {
                for bound in bounds {
                    if !self.conforms_to(act, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: binder.name.to_string(),
                            ty: act.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(act, bound),
                        });
                    }
                }
                // A pack forwarded whole binds the callee's pack to the
                // caller's; elements bind the pack to itself and are
                // collected below.
                let solved = if forwards_pack(act) { act } else { pat };
                subst
                    .entry(binder.id.clone())
                    .or_insert_with(|| solved.clone());
            } else if let TyKind::Param { binder, bounds, .. } = pat.kind()
                && !subst.contains_key(&binder.id)
                && self.binds_unmaterialized_literal(act, bounds)
            {
                subst.insert(binder.id.clone(), act.clone());
            } else if let TyKind::Param { binder, .. } = pat.kind()
                && let Some(bound) = subst.get(&binder.id)
                && *bound != *act
                && self.implicit_conversion_constructor(act, bound)?.is_some()
            {
                // An earlier argument bound the parameter, and this one
                // converts to that type, as the pin binds a parameter at its
                // first argument (`select(c, 0, length)` is over `Int`).
            } else {
                unify(pat, act, &mut subst)?;
            }
        }
        self.solve_binders_from_values(decls, &mut subst, |index, declared| {
            value_solutions
                .get(decls[index].name())
                .and_then(|value| self.ct_value_ty(value, declared))
        })?;
        unify_through_callable_bounds(decls, &mut subst)?;
        let materialize_pack_element = |ty: Ty| self.materialize_pack_element(ty);
        let inferred_packs: HashMap<String, Vec<CtValue>> = patterns
            .iter()
            .zip(actuals)
            .filter_map(|(pattern, actual)| match pattern.kind() {
                TyKind::Param { binder, .. }
                    if binder.name.starts_with('*') && !forwards_pack(actual) =>
                {
                    Some((
                        binder.name.trim_start_matches('*').to_string(),
                        actual.clone(),
                    ))
                }
                _ => None,
            })
            .fold(HashMap::new(), |mut packs, (name, ty)| {
                packs
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(CtValue::Type(materialize_pack_element(ty)));
                packs
            });
        let mut tyargs = Vec::with_capacity(decls.len());
        let mut value_environment = HashMap::new();
        for decl in decls {
            let typed = substituted_value_decl(decl, &subst);
            let typed =
                with_earlier_arguments(typed.as_ref().unwrap_or(decl), decls, &tyargs).or(typed);
            match typed.as_ref().unwrap_or(decl) {
                ParamDecl::Value {
                    name: pname,
                    default,
                    callable_default,
                    ty,
                    ..
                } => {
                    // A value pack's list is solved under its binder's
                    // name, which its declaration spells with the `*`.
                    if let Some(value) = value_solutions
                        .get(pname)
                        .or_else(|| value_solutions.get(pname.trim_start_matches('*')))
                    {
                        value_environment
                            .insert(pname.trim_start_matches('*').to_string(), value.clone());
                        tyargs.push(TyArg::Val(value.clone()));
                    } else if let Some(value) = default {
                        let value = default_argument(
                            &self.param_context,
                            decls,
                            value,
                            &value_environment,
                            ty,
                            pname,
                        )?;
                        value_environment
                            .insert(pname.trim_start_matches('*').to_string(), value.clone());
                        tyargs.push(TyArg::Val(value));
                    } else if callable_default.is_some() {
                        tyargs.push(TyArg::Val(CtValue::Deferred(decl.binder())));
                    } else {
                        return Err(TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        });
                    }
                }
                ParamDecl::Type {
                    id,
                    name: pname,
                    bounds,
                    default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        let pack_name = pname.trim_start_matches('*');
                        let argument = match subst.get(id) {
                            // Only the *caller's* pack, bound here by a whole
                            // forward, stands as the argument. The binding a
                            // collected element leaves is this pack bound to
                            // itself, which spreads like a forward but names
                            // this very binder: its elements are below.
                            Some(pack) if forwards_pack(pack) && !binds_pack_itself(pack, id) => {
                                TyArg::Ty(pack.clone())
                            }
                            // A spread of the pack in a parameter type
                            // (`storage: Tuple[*Self.Ts]`) solved it whole
                            // against the argument's element list; its
                            // elements materialize like collected ones.
                            _ => match value_solutions.get(pack_name) {
                                Some(CtValue::Tuple(elements)) => TyArg::Val(CtValue::Tuple(
                                    elements
                                        .iter()
                                        .map(|element| match element {
                                            CtValue::Type(ty) => CtValue::Type(
                                                materialize_pack_element((*ty).clone()),
                                            ),
                                            other => other.clone(),
                                        })
                                        .collect(),
                                )),
                                _ => TyArg::Val(CtValue::Tuple(
                                    inferred_packs.get(pack_name).cloned().unwrap_or_default(),
                                )),
                            },
                        };
                        tyargs.push(argument);
                        continue;
                    }
                    let solved = subst
                        .get(id)
                        .cloned()
                        .or_else(|| default.clone())
                        .ok_or_else(|| TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        })?;
                    subst.insert(id.clone(), solved.clone());
                    for bound in bounds {
                        if !self.conforms_to(&solved, bound) {
                            return Err(TypeError::TraitNotSatisfied {
                                param: pname.clone(),
                                ty: solved.to_string(),
                                trait_name: bound.clone(),
                                reason: self.trait_failure_reason(&solved, bound),
                            });
                        }
                    }
                    tyargs.push(TyArg::Ty(solved));
                }
            }
        }
        self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
        self.validate_trait_parameter_bounds(name, decls, &tyargs)?;
        self.validate_generic_constraints(name, decls, &tyargs)?;
        Ok(BoundParameters {
            subst,
            arguments: tyargs,
            attributes: Vec::new(),
        })
    }

    /// The type binders of `decls` a call's bracket arguments bind
    /// explicitly, each with the type it names: an argument display solves
    /// only the binders these leave open.
    pub(super) fn explicit_type_subst(
        &self,
        name: &str,
        decls: &[ParamDecl],
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Result<TySubst, TypeError> {
        let mut explicit = TySubst::new();
        if param_args.is_empty() || decls.is_empty() {
            return Ok(explicit);
        }
        for (decl, arguments) in decls.iter().zip(bracket_bindings(name, decls, param_args)?) {
            if let (
                ParamDecl::Type {
                    id,
                    variadic: false,
                    ..
                },
                [(_, argument)],
            ) = (decl, arguments.as_slice())
                && let TyArg::Ty(ty) = self.resolve_param_arg(decl, argument)?
            {
                explicit.insert(id.clone(), ty);
            }
        }
        Ok(explicit)
    }

    /// Whether a numeric literal argument of type `literal` binds a type
    /// parameter bounded by `bounds` as itself: the pin materializes it
    /// unless the type it materializes to fails a bound its own type meets
    /// (`FloatLiteral` is `Floatable`, `Float64` is not).
    fn binds_unmaterialized_literal(&self, literal: &Ty, bounds: &[String]) -> bool {
        matches!(literal.kind(), TyKind::IntLiteral | TyKind::FloatLiteral) && {
            let materialized = default_literal(literal);
            !bounds
                .iter()
                .all(|bound| self.conforms_to(&materialized, bound))
                && bounds.iter().all(|bound| self.conforms_to(literal, bound))
        }
    }

    /// Solve the type binders a value parameter's declared type names
    /// (`[T: AnyType, //, v: T]`) from the type of the value `value_ty`
    /// finds for it, else of its default, the way a runtime argument solves
    /// its parameter's type: a literal materializes. A later value is
    /// checked against the solution as it binds.
    fn solve_binders_from_values(
        &self,
        decls: &[ParamDecl],
        subst: &mut TySubst,
        value_ty: impl Fn(usize, &Ty) -> Option<Ty>,
    ) -> Result<(), TypeError> {
        for (index, decl) in decls.iter().enumerate() {
            let (
                ParamDecl::Value {
                    default,
                    callable_default: None,
                    ..
                },
                Some(ty),
            ) = (decl, decl.value_element_ty())
            else {
                continue;
            };
            let open: Vec<&ParamDecl> = decls
                .iter()
                .filter(|binder| {
                    matches!(binder, ParamDecl::Type { id, .. } if !subst.contains_key(id))
                        && mojito_types::types::names_binder(ty, &binder.binder())
                })
                .collect();
            if open.is_empty() {
                continue;
            }
            let Some(actual) = value_ty(index, ty).or_else(|| {
                default
                    .as_ref()
                    .and_then(|default| default.evaluate_named(&HashMap::new()).ok())
                    .and_then(|value| self.ct_value_ty(&value, ty))
            }) else {
                continue;
            };
            unify(ty, &actual, subst)?;
            for binder in open {
                let (ParamDecl::Type { bounds, .. }, Some(solved)) =
                    (binder, subst.get(binder.id()))
                else {
                    continue;
                };
                for bound in bounds {
                    if !self.conforms_to(solved, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: binder.name().to_string(),
                            ty: solved.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(solved, bound),
                        });
                    }
                }
                // A compile-time string is carried as a literal, which no
                // `String`-typed read of the binder materializes yet.
                if matches!(solved.kind(), TyKind::Struct(name, _)
                    if mojito_symbol::symbol::is_stdlib_string_struct(name))
                {
                    return Err(TypeError::Unsupported(format!(
                        "value parameter '{}' typed by '{}' cannot bind a 'String' value yet",
                        decl.name().trim_start_matches('*'),
                        binder.name()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Solve each open trait-valued binder a raw list parameter's element
    /// names (`values: KGENParamList[Tr]`) from the list its argument is, as
    /// upstream solves `TypeList[Ts.values]`'s `Trait` from `Ts`'s bound: a
    /// list of a trait binds that trait, a list of a trait binder in scope
    /// forwards the binder. A list whose element is no trait refining the
    /// binder's bound does not convert. `open` answers whether a binder has
    /// no explicit argument, and `value_ty` the type of a parameter's.
    fn solve_trait_binders_from_lists(
        &self,
        decls: &[ParamDecl],
        open: impl Fn(usize) -> bool,
        value_ty: impl Fn(usize, &Ty) -> Option<Ty>,
    ) -> Result<HashMap<ParamId, CtValue>, TypeError> {
        let mut solutions = HashMap::new();
        for (index, decl) in decls.iter().enumerate() {
            let ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } = decl
            else {
                continue;
            };
            let TyKind::ParamList(element) = ty.kind() else {
                continue;
            };
            let TyKind::Dependent(dependent) = element.kind() else {
                continue;
            };
            let Some(binder) = dependent.expr().as_decl_ref() else {
                continue;
            };
            let Some((position, ParamDecl::Value { ty: binder_ty, .. })) = decls
                .iter()
                .enumerate()
                .find(|(_, sibling)| sibling.id() == &binder.id)
            else {
                continue;
            };
            let (TyKind::AnyTrait(bound), true) = (binder_ty.kind(), open(position)) else {
                continue;
            };
            if solutions.contains_key(&binder.id) {
                continue;
            }
            let Some(actual) = value_ty(index, ty) else {
                continue;
            };
            let solved = match actual.kind() {
                TyKind::ParamList(element) => mojito_types::types::trait_type_bound(element)
                    .filter(|available| self.traits_satisfy(available, bound))
                    .and_then(|_| match element.kind() {
                        TyKind::Trait(traits) => Some(CtValue::Trait(traits.clone())),
                        TyKind::Dependent(forwarded) => {
                            Some(CtValue::Expr(forwarded.expr().clone()))
                        }
                        _ => None,
                    }),
                _ => None,
            };
            let solved = solved.ok_or_else(|| TypeError::ParamListConversion {
                param: name.trim_start_matches('*').to_string(),
                from: actual.to_string(),
                to: ty.to_string(),
            })?;
            solutions.insert(binder.id.clone(), solved);
        }
        Ok(solutions)
    }

    /// The type of the value the first bracket argument bound to a
    /// parameter declared `declared` spells: a forwarded value pack's element
    /// type, else the value as [`Self::resolve_param_arg`] types it.
    fn bound_value_ty(
        &self,
        arguments: &[(usize, &mojito_ast::ast::ParamArg)],
        declared: &Ty,
    ) -> Option<Ty> {
        let (_, argument) = arguments.first()?;
        if let Some(spread) = self.parameter_list_spread(argument) {
            return spread.ok().map(|(element, _)| element);
        }
        match self.value_pack_spread(argument) {
            Some(pack) => match pack.meta() {
                mojito_types::param_expr::MetaTy::ParamList(element) => element.as_value().cloned(),
                _ => None,
            },
            None => self.param_arg_value_ty(argument, declared),
        }
    }

    /// Solve each value binder a bracket argument's type determines
    /// (`g[T[n=4]()]()` over `def g[n: Int, //, t: T[n=n]]`) into
    /// `solutions`, from the arguments `bound` gives each of `decls`.
    fn solve_value_binders_from_brackets(
        &self,
        decls: &[ParamDecl],
        bound: &[Vec<(usize, &mojito_ast::ast::ParamArg)>],
        solutions: &mut HashMap<String, CtValue>,
    ) {
        for (decl, arguments) in decls.iter().zip(bound) {
            if let (ParamDecl::Value { .. }, Some(declared)) = (decl, decl.value_element_ty())
                && let Some(actual) = self.bound_value_ty(arguments, declared)
            {
                solve_value_args(declared, &actual, solutions);
            }
        }
    }

    /// The type of the compile-time value a bracket argument spells, typed as
    /// [`Self::resolve_param_arg`] types it against `declared`.
    fn param_arg_value_ty(
        &self,
        argument: &mojito_ast::ast::ParamArg,
        declared: &Ty,
    ) -> Option<Ty> {
        match argument {
            mojito_ast::ast::ParamArg::Value(expr) => self
                .member_value_type(expr)
                .or_else(|| {
                    self.eval_associated_ct(expr)
                        .ok()
                        .and_then(|value| self.ct_value_ty(&value, declared))
                })
                .or_else(|| self.infer(expr).ok()),
            mojito_ast::ast::ParamArg::Named { value, .. } => {
                self.param_arg_value_ty(value, declared)
            }
            mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(param))
                if let Some(value) = self.self_param_value(param) =>
            {
                self.ct_value_ty(&value, declared)
            }
            mojito_ast::ast::ParamArg::Type(source)
                if let Some(read) = self.type_spelled_member_read(source) =>
            {
                self.param_arg_value_ty(&mojito_ast::ast::ParamArg::Value(read), declared)
            }
            mojito_ast::ast::ParamArg::Type(_) => None,
        }
    }

    /// `P(copy=other)`: the explicit copy constructor when the struct declares
    /// one, else the synthesized `Copyable` copy; the result keeps the
    /// source's origin tail.
    fn infer_copy_construction(
        &self,
        span: &SourceSpan,
        name: &str,
        info: &StructInfo,
        param_args: &[mojito_ast::ast::ParamArg],
        kwargs: &[mojito_ast::ast::KwArg],
        partitioned: &super::type_resolution::PartitionedStructArgs,
    ) -> Result<Ty, TypeError> {
        let sigs = info.methods.get("__copyinit__");
        let sig = sigs.and_then(|sigs| sigs.iter().find(|sig| sig.params.len() == 1));
        if sig.is_none() {
            if !info
                .conforms
                .iter()
                .any(|conformance| self.trait_implies(conformance, "Copyable"))
            {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "no matching copy constructor".to_string(),
                });
            }
            let arg_ty = self.infer(&kwargs[0].value)?;
            self.record_read_temporary_argument(&kwargs[0].value);
            return match arg_ty.kind() {
                TyKind::Struct(actual, _) if actual == name => Ok(arg_ty),
                _ => Err(TypeError::TypeMismatch {
                    expected: name.to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument 'copy' to '{name}.__init__'"),
                }),
            };
        }
        let sig = sig.expect("explicit copy constructor was resolved");
        let params = sig.params.clone();
        let decls = info.decls.clone();
        let arg_ty = self.infer(&kwargs[0].value)?;
        self.record_read_temporary_argument(&kwargs[0].value);
        let BoundParameters {
            subst,
            arguments: tyargs,
            attributes,
        } = self.resolve_use_params(
            name,
            &decls,
            param_args,
            &params,
            std::slice::from_ref(&arg_ty),
        )?;
        self.record_bracket_attributes(span.clone(), partitioned.brackets.scatter(attributes));
        let expected = if let Some(parameter) = params.first() {
            substitute_assoc(
                parameter,
                &AssocBindings {
                    types: subst,
                    values: solved_value_bindings(&decls, &tyargs),
                    ids: mojito_types::param_expr::ParamBindings::new(),
                    origins: HashMap::new(),
                },
            )
        } else {
            Self::struct_instance_type(name, tyargs.clone())
        };
        let own = self.solve_constructor_own_params(
            name,
            sig,
            (&[], &TySubst::new()),
            std::iter::once((expected.clone(), arg_ty.clone())),
        )?;
        let expected = own.apply(self, &expected)?;
        if !coerces(&arg_ty, &expected) {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: arg_ty.to_string(),
                context: format!("argument 'copy' to '{name}.__init__'"),
            });
        }
        self.record_at(span.clone()).overload_target = Some(constructor_symbol(
            name,
            "__copyinit__",
            sigs.is_some_and(|sigs| sigs.len() > 1),
            sig,
            self.self_instance_ty(name).as_ref(),
        ));
        self.record_construction_instantiation(
            span,
            name,
            (&decls, param_args),
            &sig.names,
            tyargs.iter().cloned().chain(own.arguments).collect(),
        );
        // A copy keeps the source's origin tail.
        let mut arguments = tyargs;
        match arg_ty.kind() {
            TyKind::Struct(actual, actual_arguments) if actual == name => {
                arguments.extend_from_slice(info.origin_tail(actual_arguments));
            }
            _ => arguments.extend(partitioned.tail_arguments()),
        }
        Ok(Self::struct_instance_type(name, arguments))
    }

    /// Bind a hand-written constructor's struct origin binders named by its
    /// `Pointer[T, Self.o]` parameters from the call's arguments, as the
    /// fieldwise and generic paths do, and substitute the bound provenance
    /// into `params` before scoring. The inner `Err` skips the candidate:
    /// `None` when the call's slots do not fit it, `Some` with the origin
    /// failure to report when no candidate matches.
    fn bind_constructor_pointer_params(
        &self,
        name: &str,
        sig: &MethodSig,
        params: Vec<Ty>,
        explicit: &[super::type_resolution::ExplicitStructOrigin],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<BoundConstructorParams, TypeError> {
        let source_params = self
            .structs
            .get(name)
            .map(|info| info.source_params.as_slice())
            .unwrap_or_default();
        // A parameter naming one of the struct's own origin binders: a
        // pointer or reference over it, or a struct carrying it in its tail.
        let names_origin_binder = |parameter: &Ty| {
            mojito_types::types::mentions(parameter, &|candidate| match candidate.kind() {
                TyKind::Pointer {
                    origin: mojito_types::origin::PointerOrigin::Param { .. },
                    ..
                } => true,
                TyKind::Ref(reference) => {
                    matches!(reference.origin, mojito_types::origin::Origin::Param(_))
                }
                TyKind::Struct(_, arguments) => arguments.iter().any(|argument| {
                    matches!(
                        argument,
                        TyArg::Origin(mojito_types::origin::Origin::Param(_))
                    )
                }),
                _ => false,
            })
        };
        if !params.iter().any(names_origin_binder)
            && sig.origin_binders.iter().flatten().count() == 0
        {
            return Ok(Ok((params, ConstructorOriginBindings::default())));
        }
        let keyword_names: Vec<&str> = kwargs.iter().map(|k| k.name.as_str()).collect();
        let Ok(matched) = mojito_ast::call::match_call_slots(
            &sig.names,
            &sig.required,
            sig.positional_only,
            sig.keyword_only,
            args.len(),
            &keyword_names,
            mojito_ast::call::CallVariadics {
                positional: sig.variadic.is_some(),
                keyword: sig.kw_variadic.is_some(),
            },
        ) else {
            return Ok(Err(None));
        };
        let mut bound_slots: Vec<(usize, &Expr, &Ty)> = Vec::new();
        let mut arg_tys: Vec<Ty> = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let Some(pattern) = params.get(index).filter(|parameter| {
                names_origin_binder(parameter)
                    || matches!(sig.origin_binders.get(index), Some(Some(_)))
            }) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            arg_tys.push(self.infer(expression)?);
            bound_slots.push((index, expression, pattern));
        }
        let bindings = match self.bind_constructor_origins(
            name,
            "__init__",
            source_params,
            sig,
            &bound_slots,
            &arg_tys,
            explicit,
        ) {
            Ok(bindings) => bindings,
            Err(error) => return Ok(Err(Some(error))),
        };
        let params = params
            .iter()
            .map(|parameter| bindings.substitute(parameter))
            .collect();
        Ok(Ok((params, bindings)))
    }

    /// A constructor parameter type at an application: type parameters by
    /// `subst`, and the struct's own value binders (`Scalar[Self.dt]`) by
    /// `values` — to the caller's symbolic `dt` inside a template.
    fn constructor_parameter_ty(
        &self,
        pty: &Ty,
        subst: &TySubst,
        values: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        let expected = substitute(pty, subst);
        if values.is_empty() {
            return Ok(expected);
        }
        self.resolve_dependent_ty(&expected, values)
    }

    /// An `__init__`'s own compile-time parameters (`__init__[other_dtype:
    /// DType, //](out self, value: SIMD[other_dtype, Self.length])`), bound
    /// from the list an explicit `T.__init__[ks](…)` spells and solved from
    /// its arguments against its parameter types, the struct's binders
    /// `struct_subst` solves already substituted in both (`__init__[*values:
    /// Self.E]` of `T[Int, 3]` takes `Int` values).
    fn solve_constructor_own_params(
        &self,
        name: &str,
        sig: &MethodSig,
        (explicit, struct_subst): (&[mojito_ast::ast::ParamArg], &TySubst),
        bound: impl Iterator<Item = (Ty, Ty)>,
    ) -> Result<OwnParameters, TypeError> {
        let initializer = format!("{name}.__init__");
        if sig.decls.is_empty() {
            return self
                .resolve_use_params(&initializer, &[], explicit, &[], &[])
                .map(|_| OwnParameters::default());
        }
        let decls = sig
            .decls
            .iter()
            .map(|decl| substituted_value_decl(decl, struct_subst).unwrap_or_else(|| decl.clone()))
            .collect::<Vec<_>>();
        let (patterns, actuals): (Vec<Ty>, Vec<Ty>) = bound.unzip();
        let BoundParameters {
            mut subst,
            arguments,
            ..
        } = self.resolve_use_params(&initializer, &decls, explicit, &patterns, &actuals)?;
        unify_through_callable_bounds(&decls, &mut subst)?;
        Ok(OwnParameters {
            values: Self::value_argument_environment(&decls, &arguments),
            subst,
            arguments,
        })
    }

    /// The struct and own parameters of a construction selecting `sig`, an
    /// initializer whose `out self` refines its result to `refined`, solved
    /// as the pin solves one: a function generic over the struct parameters
    /// the refinement leaves free and its own, whose parameter patterns have
    /// each defined struct parameter replaced by its refinement and whose
    /// result is the refinement. The bracket instance the call spells (or
    /// `constructed`, an alias's) solves the own parameters a refinement
    /// names, and must agree with every position the refinement defines.
    fn solve_refined_construction(
        &self,
        name: &str,
        (sig, explicit): (&MethodSig, &[mojito_ast::ast::ParamArg]),
        refined: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        constructed: Option<&Ty>,
        bound: &[(Ty, Ty)],
    ) -> Result<(BoundParameters, OwnParameters), TypeError> {
        let unregistered = || {
            TypeError::InvariantViolation(format!("refined initializer of unregistered '{name}'"))
        };
        let decls = &self.structs.get(name).ok_or_else(unregistered)?.decls;
        let instance = self.self_instance_ty(name).ok_or_else(unregistered)?;
        let binders = mojito_types::types::nominal_parts(&instance)
            .map(|(_, arguments)| arguments.into_owned())
            .unwrap_or_default();
        let refinement = mojito_types::types::nominal_parts(refined)
            .map(|(_, arguments)| arguments.into_owned())
            .unwrap_or_default();
        let defined: Vec<bool> = decls
            .iter()
            .enumerate()
            .map(|(index, _)| binders.get(index) != refinement.get(index))
            .collect();
        let defining = |arguments: &[TyArg]| AssocBindings {
            types: decls
                .iter()
                .zip(arguments)
                .zip(&defined)
                .filter_map(|((decl, argument), defined)| match (decl, argument) {
                    (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) if *defined => {
                        Some((id.clone(), ty.clone()))
                    }
                    _ => None,
                })
                .collect(),
            values: decls
                .iter()
                .zip(arguments)
                .zip(&defined)
                .filter_map(|((decl, argument), defined)| match (decl, argument) {
                    (ParamDecl::Value { name, .. }, TyArg::Val(value)) if *defined => {
                        Some((name.clone(), value.clone()))
                    }
                    _ => None,
                })
                .collect(),
            ids: mojito_types::param_expr::ParamBindings::new(),
            origins: HashMap::new(),
        };
        // The instance the brackets or an alias spell, whole.
        let known = if param_args.is_empty() {
            constructed.and_then(|ty| {
                mojito_types::types::nominal_parts(ty).map(|(_, arguments)| arguments.into_owned())
            })
        } else {
            self.resolve_use_params(name, decls, param_args, &[], &[])
                .ok()
                .map(|bound| bound.arguments)
        };
        let own_patterns: Vec<(Ty, Ty)> = bound
            .iter()
            .map(|(pattern, actual)| {
                (
                    substitute_assoc(pattern, &defining(&refinement)),
                    actual.clone(),
                )
            })
            .chain(known.iter().map(|arguments| {
                (
                    refined.clone(),
                    TyKind::Struct(name.to_string(), arguments.clone()).into(),
                )
            }))
            .collect();
        // A default of the initializer's own never feeds the struct: a
        // refinement naming an own parameter no argument, no explicit own
        // argument, and no spelled instance determines leaves its struct
        // parameter uninferred.
        let spelled_own = bracket_bindings(&format!("{name}.__init__"), &sig.decls, explicit)?;
        for ((decl, argument), defined) in decls.iter().zip(&refinement).zip(&defined) {
            if !defined || known.is_some() {
                continue;
            }
            let undetermined = sig.decls.iter().zip(&spelled_own).any(|(own, spelled)| {
                let own = std::slice::from_ref(own);
                spelled.is_empty()
                    && mentioned_binder(argument, own).is_some()
                    && !own_patterns.iter().any(|(pattern, _)| {
                        mentioned_binder(&TyArg::Ty(pattern.clone()), own).is_some()
                    })
            });
            if undetermined {
                return Err(TypeError::CannotInferTypeParam {
                    name: name.to_string(),
                    param: decl.name().to_string(),
                });
            }
        }
        let own = self.solve_constructor_own_params(
            name,
            sig,
            (explicit, &TySubst::new()),
            own_patterns.into_iter(),
        )?;
        let solved = own.apply(self, refined)?;
        let solved_refinement = mojito_types::types::nominal_parts(&solved)
            .map(|(_, arguments)| arguments.into_owned())
            .unwrap_or_default();
        // The defined positions bind as the solved refinement spells them;
        // a free one is solved as an unrefined construction solves it.
        let masked = |arguments: &[TyArg]| -> Ty {
            TyKind::Struct(
                name.to_string(),
                arguments
                    .iter()
                    .zip(&defined)
                    .map(|(argument, defined)| {
                        if *defined {
                            argument.clone()
                        } else {
                            TyArg::Ty(Ty::none())
                        }
                    })
                    .collect(),
            )
            .into()
        };
        let (patterns, actuals): (Vec<Ty>, Vec<Ty>) =
            std::iter::once((masked(&binders), masked(&solved_refinement)))
                .chain(
                    bound
                        .iter()
                        .map(|(pattern, actual)| {
                            Ok::<_, TypeError>((
                                own.apply(
                                    self,
                                    &substitute_assoc(pattern, &defining(&solved_refinement)),
                                )?,
                                actual.clone(),
                            ))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .unzip();
        let (patterns, actuals) =
            self.constructed_unification(name, constructed, &patterns, &actuals);
        let bound_parameters =
            self.resolve_use_params(name, decls, param_args, &patterns, &actuals)?;
        let spelled = canonical_arguments(name, &bound_parameters.arguments);
        for ((decl, (argument, expected)), defined) in decls
            .iter()
            .zip(
                spelled
                    .iter()
                    .zip(&canonical_arguments(name, &solved_refinement)),
            )
            .zip(&defined)
        {
            if *defined && argument != expected {
                return Err(TypeError::TypeMismatch {
                    expected: argument.to_string(),
                    found: expected.to_string(),
                    context: format!("parameter '{}' of the return type '{solved}'", decl.name()),
                });
            }
        }
        Ok((bound_parameters, own))
    }

    /// A pack element materializes like any other inferred instantiation
    /// argument (a literal binds `Int`/`String`). A seam without the linked
    /// String struct keeps the literal: materializing to an absent struct
    /// would fail every conformance the literal itself satisfies.
    fn materialize_pack_element(&self, ty: Ty) -> Ty {
        if ty.is_string_literal()
            && !self
                .structs
                .contains_key(mojito_symbol::symbol::STDLIB_STRING_STRUCT)
        {
            return ty;
        }
        match materialized_instantiation_argument(&TyArg::Ty(ty)) {
            TyArg::Ty(ty) => ty,
            _ => unreachable!("a type argument materializes to a type"),
        }
    }

    /// The argument a type pack `id` takes from the runtime arguments its
    /// collector gathered: a caller's whole pack forwarded into it, or the
    /// tuple of the collected arguments' materialized types.
    fn collected_pack_argument(
        &self,
        id: &mojito_types::param_expr::ParamId,
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> TyArg {
        let collected = patterns
            .iter()
            .zip(actuals)
            .filter(|(pattern, _)| matches!(pattern.kind(), TyKind::Param { binder, .. } if binder.id == *id));
        let mut elements = Vec::new();
        for (_, actual) in collected {
            if forwards_pack(actual) && !binds_pack_itself(actual, id) {
                return TyArg::Ty(actual.clone());
            }
            elements.push(CtValue::Type(self.materialize_pack_element(actual.clone())));
        }
        TyArg::Val(CtValue::Tuple(elements))
    }
}

/// A constructor candidate's parameter types with the struct's origin binders
/// bound from the arguments, or the origin-binding failure that rejected the
/// candidate (`None` when the arguments simply did not match its slots).
type BoundConstructorParams = Result<(Vec<Ty>, ConstructorOriginBindings), Option<TypeError>>;

/// The consumption a `var` or `deinit` argument convention performs.
fn consume_kind(convention: ArgConvention) -> super::traits::ConsumeKind {
    if convention == ArgConvention::Deinit {
        super::traits::ConsumeKind::Deinit
    } else {
        super::traits::ConsumeKind::Move
    }
}

/// Whether a `*args: *Self.Ts` collector took exactly the elements of the
/// pack an application bound (`Bag[Int, String](1)` is short one). A
/// collector over an open pack, or a constructor without one, takes what it
/// is given.
fn collector_takes_bound_pack(
    sig: &MethodSig,
    pack_elements: Option<&(String, Vec<Ty>)>,
    overflow: usize,
) -> bool {
    let spreads_pack = sig.variadic.as_ref().is_some_and(|element| {
        mojito_types::types::pack_spread(std::slice::from_ref(element)).is_some()
    });
    match pack_elements {
        Some((_, elements)) if spreads_pack => overflow == elements.len(),
        _ => true,
    }
}

/// Whether an actual is a caller's whole pack, still a parameter, forwarded
/// into the callee's collector.
/// A generic constructor's own type arguments, as `subst` solved them in
/// declaration order; none unless every one is a solved type (a value
/// parameter of its own is left to the call's arguments).
/// Whether an initializer whose `out self` refines its result to `refined`
/// can construct `target`, an instance of the struct at its own binders
/// `instance`: every position the refinement defines is `target`'s, or
/// names one of the initializer's own parameters (`own`), which a call
/// solves.
pub(super) fn refinement_admits(
    refined: &Ty,
    instance: &Ty,
    own: &[ParamDecl],
    target: &Ty,
) -> bool {
    let arguments =
        |ty| mojito_types::types::nominal_parts(ty).map(|(_, arguments)| arguments.into_owned());
    let (Some(refinement), Some(binders), Some(target)) =
        (arguments(refined), arguments(instance), arguments(target))
    else {
        return false;
    };
    refinement
        .iter()
        .zip(&binders)
        .zip(&target)
        .all(|((defined, binder), argument)| {
            defined == binder || defined == argument || mentioned_binder(defined, own).is_some()
        })
}

/// The name of the first of `decls`' binders `argument` mentions, a type
/// binder anywhere in a type or a value binder in a compile-time expression.
fn mentioned_binder<'d>(argument: &TyArg, decls: &'d [ParamDecl]) -> Option<&'d str> {
    let binder = |id: &mojito_types::param_expr::ParamId| {
        decls
            .iter()
            .find(|decl| decl.id() == id)
            .map(ParamDecl::name)
    };
    let in_value = |value: &CtValue| match value {
        CtValue::Expr(expression) => expression
            .free_parameters()
            .iter()
            .find_map(|reference| binder(&reference.id)),
        CtValue::Type(ty) => mentioned_binder(&TyArg::Ty(ty.clone()), decls),
        _ => None,
    };
    match argument {
        TyArg::Ty(ty) => {
            let found = std::cell::Cell::new(None);
            mojito_types::types::mentions(ty, &|nested| {
                let hit = match nested.kind() {
                    TyKind::Param {
                        binder: reference, ..
                    } => binder(&reference.id),
                    TyKind::Struct(_, arguments) => {
                        arguments.iter().find_map(|argument| match argument {
                            TyArg::Val(value) => in_value(value),
                            _ => None,
                        })
                    }
                    _ => None,
                };
                if hit.is_some() {
                    found.set(hit);
                }
                hit.is_some()
            });
            found.get()
        }
        TyArg::Val(value) => in_value(value),
        TyArg::Origin(_) => None,
    }
}

/// `subst` without the bindings of `sig`'s own compile-time parameters,
/// which [`Checker::solve_constructor_own_params`] solves apart.
fn struct_binders_only(subst: &TySubst, sig: &MethodSig) -> TySubst {
    subst
        .iter()
        .filter(|(id, _)| !sig.decls.iter().any(|decl| decl.id() == *id))
        .map(|(id, ty)| (id.clone(), ty.clone()))
        .collect()
}

fn forwards_pack(actual: &Ty) -> bool {
    mojito_types::types::pack_spread(std::slice::from_ref(actual)).is_some()
}

/// Whether the pack `bound` is the binder `id` itself, the self-binding a
/// collected element leaves behind rather than a forward of another pack.
fn binds_pack_itself(bound: &Ty, id: &mojito_types::param_expr::ParamId) -> bool {
    matches!(bound.kind(), TyKind::Param { binder, .. } if binder.id == *id)
}

/// Whether a `return` occurs anywhere below. A nested function's own
/// `return` counts too, which only defers more to the executable check.
fn block_holds_return(body: &[Stmt]) -> bool {
    struct Finder(bool);
    impl mojito_ast::visit::Visitor for Finder {
        fn visit_stmt(&mut self, stmt: &Stmt) {
            self.0 |= matches!(stmt.kind, StmtKind::Return(_));
        }
    }
    let mut finder = Finder(false);
    mojito_ast::visit::walk_block(&mut finder, body);
    finder.0
}

/// A fieldwise construction's arguments in field order, keywords bound to the
/// fields they name.
fn fieldwise_arguments(
    name: &str,
    info: &StructInfo,
    args: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Result<Vec<Expr>, TypeError> {
    let field_names: Vec<String> = info.fields.iter().map(|(field, _)| field.clone()).collect();
    let keyword_names: Vec<&str> = kwargs.iter().map(|k| k.name.as_str()).collect();
    let slots = mojito_ast::call::match_fieldwise_slots(&field_names, args.len(), &keyword_names)
        .map_err(|error| error.into_type_error(name))?;
    Ok(slots
        .into_iter()
        .filter_map(|slot| match slot {
            ArgSlot::Positional(position) => Some(args[position].clone()),
            ArgSlot::Keyword(position) => Some(kwargs[position].value.clone()),
            ArgSlot::Default => None,
        })
        .collect())
}

/// The `TyKind::Param` of the earlier, non-variadic type binder `name` names in a
/// list being classified.
fn sibling_type_binder(decls: &[ParamDecl], name: &str) -> Option<Ty> {
    decls
        .iter()
        .find(|decl| {
            decl.name() == name
                && matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: false,
                        ..
                    }
                )
        })
        .and_then(type_parameter)
}

/// The earlier trait-valued binder `name` names in a list being classified
/// (`Tr: type_of(Copyable)`), with its declared trait set.
fn sibling_trait_binder(decls: &[ParamDecl], name: &str) -> Option<(ParamRef, Vec<String>)> {
    decls.iter().find_map(|decl| match decl {
        ParamDecl::Value {
            ty,
            variadic: false,
            ..
        } if decl.name() == name
            && let TyKind::AnyTrait(bound) = ty.kind() =>
        {
            Some((decl.binder(), bound.clone()))
        }
        _ => None,
    })
}

/// Solve each value binder a default's type determines (`def g[t: T =
/// T[n=4]()]` over an infer-only `n`) into `solutions`.
fn solve_value_binders_from_defaults(
    decls: &[ParamDecl],
    solutions: &mut HashMap<String, CtValue>,
) {
    for decl in decls {
        if let ParamDecl::Value {
            default: Some(default),
            callable_default: None,
            ..
        } = decl
            && let (Some(declared), Some(actual)) =
                (decl.value_element_ty(), default.meta().as_value())
        {
            solve_value_args(declared, actual, solutions);
        }
    }
}

/// `decl` with each earlier value binder its value type names (`values:
/// KGENParamList[Tr]`, `mask: StaticTuple[Int, n]` after `n: Int`) replaced
/// by the argument `arguments` bound it, or `None` when it names none: a
/// slot's type reads the arguments bound before it.
fn with_earlier_arguments(
    decl: &ParamDecl,
    decls: &[ParamDecl],
    arguments: &[TyArg],
) -> Option<ParamDecl> {
    let ParamDecl::Value { ty, .. } = decl else {
        return None;
    };
    let context = mojito_types::param_expr::ParamContext::detached();
    let mut bindings = mojito_types::param_expr::ParamBindings::new();
    for (sibling, argument) in decls.iter().zip(arguments) {
        if let (ParamDecl::Value { .. }, TyArg::Val(value)) = (sibling, argument)
            && let Some(value) = match value {
                CtValue::Expr(expr) => Some(expr.clone()),
                value => context.constant(value.clone()).ok(),
            }
        {
            bindings.bind(sibling.id().clone(), value);
        }
    }
    let solved = mojito_types::types::replace_parameters(&context, ty, &bindings, 0).ok()?;
    (solved != **ty).then(|| decl.with_value_type(solved))
}

/// The argument the default `value` of the value parameter `name` of type
/// `ty` gives a use that omits it, under the earlier arguments
/// `environment`: the value it folds to, or a closed application it denotes
/// (`e: Extent = Extent(2, 3)`), which the elaborator evaluates.
fn default_argument(
    params: &mojito_types::param_expr::ParamContext,
    decls: &[ParamDecl],
    value: &ParamExpr,
    environment: &HashMap<String, CtValue>,
    ty: &Ty,
    name: &str,
) -> Result<CtValue, TypeError> {
    let context = || format!("default for parameter '{name}'");
    let value = match value.evaluate_named(environment) {
        Ok(value) => value,
        // A closed application the elaborator evaluates, or a default over
        // binders still open at the use (`Self.dtype` at a receiver in its
        // struct's own generic body), which each instance closes. The
        // earlier parameters it reads are their arguments (`w[dtype]()` at
        // `dtype = DType.float32`).
        Err(_) if value.meta().as_value() == Some(ty) => {
            let mut bindings = mojito_types::param_expr::ParamBindings::new();
            for decl in decls {
                if let Some(argument) = environment
                    .get(decl.name().trim_start_matches('*'))
                    .and_then(|argument| params.constant(argument.clone()).ok())
                {
                    bindings.bind(decl.id().clone(), argument);
                }
            }
            return Ok(CtValue::Expr(
                params
                    .replace(value, &bindings)
                    .unwrap_or_else(|_| value.clone()),
            ));
        }
        Err(_) => return Err(TypeError::NotComptime(context())),
    };
    let rendered = value.to_string();
    value
        .materialize_as(ty)
        .ok_or_else(|| TypeError::TypeMismatch {
            expected: ty.to_string(),
            found: rendered,
            context: context(),
        })
}

/// `decl` with the binders its value type names replaced by their solutions
/// in `subst`, or `None` when it names none.
fn substituted_value_decl(decl: &ParamDecl, subst: &TySubst) -> Option<ParamDecl> {
    let ParamDecl::Value { ty, .. } = decl else {
        return None;
    };
    let solved = mojito_types::types::substitute(ty, subst);
    (solved != **ty).then(|| decl.with_value_type(solved))
}

/// Each of `decls`' bracket arguments in `param_args`, with its source
/// position, bound as [`mojito_ast::call::match_parameter_slots`] binds them:
/// positional arguments to the slots that are not infer-only, in order, then
/// keyword ones by name, an infer-only binder included.
fn bracket_bindings<'a>(
    name: &str,
    decls: &[ParamDecl],
    param_args: &'a [mojito_ast::ast::ParamArg],
) -> Result<Vec<Vec<(usize, &'a mojito_ast::ast::ParamArg)>>, TypeError> {
    let keywords = mojito_ast::call::bracket_keywords(param_args);
    let slots = mojito_ast::call::match_parameter_slots(decls, &keywords)
        .map_err(|error| error.into_type_error(name))?;
    let mut bound: Vec<Vec<(usize, &mojito_ast::ast::ParamArg)>> = vec![Vec::new(); decls.len()];
    for (position, (argument, index)) in param_args.iter().zip(slots).enumerate() {
        let argument = match argument {
            mojito_ast::ast::ParamArg::Named { value, .. } => &**value,
            positional => positional,
        };
        bound[index].push((position, argument));
    }
    Ok(bound)
}

/// One generic constructor overload the arguments match: its rank, its
/// solved struct parameters, and the solved parameter type each bound
/// argument is checked against once it wins.
/// An `__init__`'s own compile-time parameters, solved from a
/// construction's arguments.
#[derive(Default)]
struct OwnParameters {
    subst: TySubst,
    values: HashMap<String, CtValue>,
    arguments: Vec<TyArg>,
}

impl OwnParameters {
    /// `ty`, a parameter type over the struct's arguments, at these own
    /// parameters too.
    fn apply(&self, checker: &Checker, ty: &Ty) -> Result<Ty, TypeError> {
        if self.arguments.is_empty() {
            return Ok(ty.clone());
        }
        let substituted = checker.resolve_assoc_ty(&substitute(ty, &self.subst));
        checker.resolve_dependent_ty(&substituted, &self.values)
    }
}

struct GenericConstructorMatch {
    rank: usize,
    sig: MethodSig,
    tyargs: Vec<TyArg>,
    /// The selected `__init__`'s own compile-time parameters.
    own_arguments: Vec<TyArg>,
    /// Indexed by position in the scored bound (regular slots, then
    /// variadic overflow), the forwarded pack left out.
    expected: Vec<(usize, Ty)>,
    slots: Vec<ArgSlot>,
    overflow: Vec<usize>,
    subst: TySubst,
    pointer_origins: ConstructorOriginBindings,
}

/// How a selected `__init__` binds a construction's arguments, as
/// [`Checker::record_constructor_call`] records them.
struct ConstructorBinding<'a> {
    slots: &'a [ArgSlot],
    conventions: &'a [Option<ArgConvention>],
    ref_params: &'a [Option<mojito_types::origin::RefSig>],
    /// The positions the `*args` collector gathers.
    collected: Gathered<'a>,
    raises: bool,
    error: Option<&'a Ty>,
    exclusivity: ConstructorExclusivity<'a>,
}

impl<'a> ConstructorBinding<'a> {
    /// `overflow` holds the positions the signature's `*args` collector
    /// gathers.
    fn of_signature(
        sig: &'a MethodSig,
        slots: &'a [ArgSlot],
        overflow: &'a [usize],
        exclusivity: ConstructorExclusivity<'a>,
    ) -> Self {
        let collected = Gathered::of_convention(sig.variadic_convention, overflow);
        Self {
            slots,
            conventions: &sig.conventions,
            ref_params: &sig.ref_params,
            collected,
            raises: sig.raises,
            error: sig.error.as_ref(),
            exclusivity,
        }
    }

    /// The synthesized fieldwise `__init__`'s binding of the field-ordered
    /// arguments, by the conventions and `ref` contracts the check gives its
    /// parameters ([`Checker::fieldwise_conventions`],
    /// [`Checker::fieldwise_ref_params`]): it gathers nothing and never
    /// raises.
    const fn fieldwise(
        slots: &'a [ArgSlot],
        conventions: &'a [Option<ArgConvention>],
        ref_params: &'a [Option<mojito_types::origin::RefSig>],
        exclusivity: ConstructorExclusivity<'a>,
    ) -> Self {
        Self {
            slots,
            conventions,
            ref_params,
            collected: Gathered::NONE,
            raises: false,
            error: None,
            exclusivity,
        }
    }
}

/// What a construction's within-call exclusivity is judged against, as
/// [`ExclusivityCallee`] spells it for the selected `__init__`.
struct ConstructorExclusivity<'a> {
    /// The constructed type, which names the initializer call.
    callee: String,
    parameter_names: &'a [String],
    declared: Vec<Ty>,
    /// The parameter type each slot binds, aligned with the slots.
    bound: Vec<Ty>,
    nested_origins: NestedOrigins,
    gathered: CollectedArguments<'a>,
}

/// Bind each origin slot `partitioned` leaves open to the one `instance`
/// binds, as a spelled bracket's origin argument does: the construction of
/// an instance with no source spelling (the initializer list `{...}` at
/// `Span[T, o]`) is `Span[T, o](...)`, never a construction whose arguments
/// pick the origin. A slot the instance binds to the receiver's symbolic
/// origin (`origin_of(self)` in a signature) is the receiver argument's to
/// bind, as it names no place of its own.
fn bind_instance_origins(
    partitioned: &mut super::type_resolution::PartitionedStructArgs,
    source_params: &[mojito_ast::ast::TypeParam],
    instance: &Ty,
) {
    use mojito_types::origin::{Origin, OriginParamId};
    let TyKind::Struct(_, arguments) = instance.kind() else {
        return;
    };
    let origins = arguments.iter().filter_map(|argument| match argument {
        TyArg::Origin(origin) => Some(origin),
        _ => None,
    });
    let slots = source_params
        .iter()
        .enumerate()
        .filter(|(_, param)| !param.infer_only && super::is_origin_type_param(param));
    for (tail, ((slot, _), origin)) in slots.zip(origins).enumerate() {
        let id = OriginParamId(u32::try_from(slot).unwrap_or(u32::MAX));
        if matches!(origin, Origin::Unbound | Origin::SelfParam)
            || partitioned
                .explicit_origins
                .iter()
                .any(|explicit| explicit.id == id)
        {
            continue;
        }
        partitioned
            .explicit_origins
            .push(super::type_resolution::ExplicitStructOrigin {
                id,
                origin: origin.clone(),
                mutability: None,
            });
        if let Some(entry) = partitioned.tail.get_mut(tail) {
            *entry = origin.clone();
        }
    }
}

/// `arguments` as the instance of `name` they spell holds them: a bridged
/// struct's (`SIMD`'s) arguments fold as its `Ty` does, so a bracket's
/// `SIMDLength(1)` application and a refinement's folded `1` agree.
fn canonical_arguments(name: &str, arguments: &[TyArg]) -> Vec<TyArg> {
    let instance: Ty = TyKind::Struct(name.to_string(), arguments.to_vec()).into();
    mojito_types::types::nominal_parts(&instance).map_or_else(
        || arguments.to_vec(),
        |(_, arguments)| arguments.into_owned(),
    )
}
