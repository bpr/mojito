//! AST rewriting helpers used by compile-time elaboration.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::FnParam;

// --- Substitution / materialization -----------------------------------------
//
// One generic rewrite over the AST parameterized by a name→value lookup, used for
// two things: substituting a `comptime for` loop variable with its literal (in the
// unrolled body — does NOT descend into nested `def`/`struct`), and materializing
// module-level `comptime` constants into runtime literals (does descend, minus a
// function's own parameter names, which shadow).

/// A name→compile-time-value lookup for a rewrite.
pub(super) type Subs<'a> = &'a dyn Fn(&str) -> Option<CtValue>;

/// Materialize module-level comptime constants throughout a program.
pub(super) fn materialize_block(
    stmts: Vec<Stmt>,
    consts: &HashMap<String, CtValue>,
    type_names: &HashSet<String>,
) -> Vec<Stmt> {
    // Declared struct names are marked so a subscript-shaped projection on a
    // runtime local (`v[String]`) can be told from ordinary indexing.
    // A collection constant never inlines: its runtime uses were folded
    // through `materialize[...]()` or rejected before this pass.
    let subs: Subs = &|n| {
        consts
            .get(n)
            .filter(|value| !value.is_runtime_collection())
            .cloned()
            .or_else(|| type_names.contains(n).then(type_name_marker))
    };
    stmts
        .into_iter()
        .map(|s| rewrite_stmt_cloned(&s, subs, true))
        .collect()
}

pub(super) fn rewrite_stmt_cloned(s: &Stmt, subs: Subs, into_defs: bool) -> Stmt {
    let mut s = s.clone();
    rewrite_stmt(&mut s, subs, into_defs);
    s
}

// --- Type-binding substitution ----------------------------------------------
//
// `materialize_block` above substitutes *value* occurrences of the bindings;
// this second rewrite substitutes the *type* occurrences it leaves behind:
// annotations, compile-time argument lists, and constructor-call heads. Unlike
// the value rewrite it always descends into nested `def`/`struct` bodies —
// a dropped binding is in scope throughout the clone — shadowed only by a
// nested declaration's own type parameter of the same name.

/// A binding→concrete-source-type lookup for a dropped-parameter rewrite.
pub(super) type TypeSubs<'a> = &'a HashMap<String, Type>;

pub(super) fn substitute_type_bindings_in_block(body: &mut [Stmt], subs: TypeSubs) {
    for statement in body {
        retype_stmt(statement, subs);
    }
}

pub(super) fn substitute_type_bindings_in_type(ty: &mut Type, subs: TypeSubs) {
    for (binding, replacement) in subs {
        substitute_source_type_binding(ty, binding, replacement);
    }
}

pub(super) fn rewrite_expr(e: &mut Expr, subs: Subs) {
    if let Some(mut folded) = fold_pack_typelist_use(e, subs) {
        // The fold keeps the use's identity, as a folded loop variable
        // keeps its identifier's below.
        folded.syntax_id = e.syntax_id;
        *e = folded;
        return;
    }
    // A type application on a runtime local (`v[Int]`) is a subscript whose
    // index names a type — the checker's type-keyed `__getitem_param__[T]`
    // projection (the self-hosted `Variant`'s `v[T]`), in value and place
    // positions alike; `v[String]` already parses as that subscript.
    if projection_to_subscript(e, subs) {
        return;
    }
    // A field chain off a substituted binder (`e.i` of a `comptime for`
    // element) is upstream's struct extract on the parameter: the leaf
    // alone materializes, never the whole struct.
    if matches!(e.kind, ExprKind::Member { .. })
        && let Some(mut materialized) = substituted_field(e, subs)
            .filter(materializes_at_its_type)
            .and_then(|leaf| leaf.materialize(e.span))
    {
        materialized.syntax_id = e.syntax_id;
        *e = materialized;
        return;
    }
    match &mut e.kind {
        ExprKind::Identifier(name) => {
            if let Some(value) = subs(name) {
                if let CtValue::Type(ty) = &value
                    && let Some(ty) = source_type_from_ty(ty)
                {
                    e.kind = ExprKind::TypeValue(ty);
                } else if let Some(mut materialized) = value.materialize(e.span) {
                    // A folded loop variable keeps the identifier's identity:
                    // that is the occurrence-level trace a checked template's
                    // instances are matched by, as `rebuilt` keeps a
                    // statement's.
                    materialized.syntax_id = e.syntax_id;
                    *e = materialized;
                }
            }
        }
        ExprKind::Spread(value) => rewrite_expr(value, subs),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::EmptySubscript => {}
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let mojito_ast::ast::TStringPart::Expr(value) = part {
                    rewrite_expr(value, subs);
                }
            }
        }
        // A value **parameter** argument (`Box[CAP](…)`, `UnsafePointer[CAP]`) may
        // reference a comptime constant, so rewrite the `Value` param args too.
        ExprKind::TypeApply { args, .. } => rewrite_spelled_param_args(args, subs, e.syntax_id),
        ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) => rewrite_expr(inner, subs),
        ExprKind::Infix(_, l, r) => {
            rewrite_expr(l, subs);
            rewrite_expr(r, subs);
        }
        ExprKind::Compare { first, rest } => {
            rewrite_expr(first, subs);
            for (_, r) in rest {
                rewrite_expr(r, subs);
            }
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => {
            // `H()` on a substituted type parameter constructs the bound struct;
            // a vector alias (`U256(0)`) constructs `SIMD[DType.d, w](...)`.
            if let Some(CtValue::Type(bound)) = subs(name)
                && param_args.is_empty()
            {
                match &*bound {
                    Ty::Struct(struct_name, struct_args) if struct_args.is_empty() => {
                        name.clone_from(struct_name);
                    }
                    // A bound vector type is concrete (the elaborator binds
                    // no symbolic slot), so a symbolic one is left as it is.
                    Ty::Simd { dtype, width } => {
                        if let (Some(mut dtype), Some(mut width)) = (
                            dtype
                                .known()
                                .and_then(|dtype| CtValue::Dtype(dtype).materialize(e.span)),
                            width
                                .known()
                                .and_then(|width| CtValue::Int(width).materialize(e.span)),
                        ) {
                            // The expansion's operands are named by the
                            // spelled call, so every clone of one template
                            // traces them to the same occurrences.
                            dtype.syntax_id =
                                mojito_common::token::SyntaxId::derived(e.syntax_id, 0);
                            width.syntax_id =
                                mojito_common::token::SyntaxId::derived(e.syntax_id, 1);
                            *name = "SIMD".to_string();
                            *param_args = vec![ParamArg::Value(dtype), ParamArg::Value(width)];
                        }
                    }
                    _ => {}
                }
            }
            rewrite_spelled_param_args(param_args, subs, e.syntax_id);
            rewrite_exprs(args, subs);
            for k in kwargs {
                rewrite_expr(&mut k.value, subs);
            }
        }
        ExprKind::Member { object, field } => {
            // `Self.<binder>` of a baked value parameter (`Self.key`) reads
            // the bound value inside a specialization body.
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && let Some(value) = subs(field)
                && !matches!(
                    value,
                    CtValue::Type(_)
                        | CtValue::Reflected(_)
                        | CtValue::Expr(_)
                        | CtValue::Deferred(_)
                        | CtValue::Marker(_)
                )
                && let Some(mut materialized) = value.materialize(e.span)
            {
                // The fold keeps the use's identity, which the checked
                // template typed; a vector's lanes are the fold's own.
                materialized.syntax_id = e.syntax_id;
                *e = materialized;
                return;
            }
            rewrite_expr(object, subs);
        }
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            // `Self.T()` in a generic struct becomes an ordinary constructor
            // once specialization has baked the enclosing type parameter.
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && args.is_empty()
                && kwargs.is_empty()
                && let Some(CtValue::Type(bound)) = subs(method)
            {
                match &*bound {
                    Ty::Struct(name, parameters) if parameters.is_empty() => {
                        e.kind = ExprKind::Call {
                            name: name.clone(),
                            param_args: Vec::new(),
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        };
                        return;
                    }
                    Ty::Simd { dtype, width } => {
                        let Some(dtype) = dtype
                            .known()
                            .and_then(|dtype| CtValue::Dtype(dtype).materialize(e.span))
                        else {
                            return;
                        };
                        let Some(width) = width
                            .known()
                            .and_then(|width| CtValue::Int(width).materialize(e.span))
                        else {
                            return;
                        };
                        e.kind = ExprKind::Call {
                            name: "SIMD".to_string(),
                            param_args: vec![ParamArg::Value(dtype), ParamArg::Value(width)],
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        };
                        return;
                    }
                    _ => {}
                }
            }
            rewrite_expr(object, subs);
            rewrite_exprs(args, subs);
            for k in kwargs {
                rewrite_expr(&mut k.value, subs);
            }
        }
        ExprKind::Index { object, index } => {
            rewrite_expr(object, subs);
            rewrite_expr(index, subs);
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            rewrite_expr(object, subs);
            for b in [lower, upper, step].into_iter().flatten() {
                rewrite_expr(b, subs);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            rewrite_expr(object, subs);
            for argument in args {
                match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        rewrite_expr(value, subs);
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            rewrite_expr(value, subs);
                        }
                    }
                }
            }
        }
        ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => rewrite_exprs(elems, subs),
        ExprKind::TypeValue(ty) => rewrite_type(ty, subs),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            rewrite_expr(callee, subs);
            rewrite_spelled_param_args(param_args, subs, e.syntax_id);
            rewrite_exprs(args, subs);
            for argument in kwargs {
                rewrite_expr(&mut argument.value, subs);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                rewrite_expr(key, subs);
                if let Some(value) = value {
                    rewrite_expr(value, subs);
                }
            }
        }
        // Comprehension targets introduce a nested lexical environment. The
        // runtime checker/lowerer handles their expressions; blindly applying
        // this flat comptime substitution could replace a shadowed target.
        ExprKind::Comprehension { .. } => {}
        ExprKind::Uninitialized => {}
        ExprKind::Named { value, .. } => rewrite_expr(value, subs),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite_expr(cond, subs);
            rewrite_expr(then_branch, subs);
            rewrite_expr(else_branch, subs);
        }
        // Always substitute into a lambda's hidden definition (its own
        // argument and type-parameter names shadow): an enclosing comptime
        // binding is substituted, never captured, so it must materialize in
        // the lambda body even in walks that skip named nested `def`s.
        ExprKind::Lambda { def } => rewrite_stmt(def, subs, true),
    }
}

/// The source type of element `index` of the type pack `pack`, when `subs`
/// binds the pack and `index` is a literal in range.
fn bound_pack_element(pack: &str, index: &Expr, subs: Subs) -> Option<Type> {
    let ExprKind::Int(position) = &index.kind else {
        return None;
    };
    let CtValue::Tuple(elements) = subs(pack.trim_start_matches('*'))? else {
        return None;
    };
    let position = usize::try_from(position.to_i64()?).ok()?;
    match elements.get(position)? {
        CtValue::Type(ty) => source_type_from_ty(ty),
        _ => None,
    }
}

/// Fold upstream's `TypeList` uses of a bound type pack in a runtime
/// position — `Ts.length` / `Self.Ts.length` / `len(Ts)` to the pack's length and
/// `Ts.contains[X]()` to whether `X` names an element — so a specialized
/// variadic struct's methods (`is_type_supported`, `__len__`) see literals.
/// The conformance-dependent members (`all_conforms_to`, `all`, `any`) need
/// the elaborator's oracle and fold only in compile-time positions.
fn fold_pack_typelist_use(e: &Expr, subs: Subs) -> Option<Expr> {
    fn pack_elements(object: &Expr, subs: Subs) -> Option<Vec<Type>> {
        let name = match &object.kind {
            ExprKind::Identifier(name) => name,
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") => {
                field
            }
            _ => return None,
        };
        let CtValue::Tuple(values) = subs(name)? else {
            return None;
        };
        values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => source_type_from_ty(ty),
                _ => None,
            })
            .collect()
    }
    /// The bound pack's element count: every element counts, including the
    /// callable values of a function-valued Tuple (`(identity, offset)`),
    /// which `pack_elements` cannot spell as source types.
    fn pack_length(object: &Expr, subs: Subs) -> Option<usize> {
        let name = match &object.kind {
            ExprKind::Identifier(name) => name,
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") => {
                field
            }
            _ => return None,
        };
        let CtValue::Tuple(values) = subs(name)? else {
            return None;
        };
        Some(values.len())
    }
    /// The element count of a list computed from bound packs: a reversal
    /// keeps its operand's, a concatenation sums its operands'.
    fn list_length(object: &Expr, subs: Subs) -> Option<usize> {
        if let Some(length) = pack_length(object, subs) {
            return Some(length);
        }
        match &object.kind {
            ExprKind::TupleLit(elements) => Some(elements.len()),
            ExprKind::Member { object, field } if field == "values" => list_length(object, subs),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "reverse" && args.is_empty() && kwargs.is_empty() => {
                list_length(object, subs)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty()
                && kwargs.is_empty()
                && matches!(&callee.kind, ExprKind::Member { object, field }
                    if field == "_concat"
                        && matches!(&object.kind, ExprKind::Identifier(name) if name == "TypeList")) =>
            {
                param_args
                    .iter()
                    .map(|argument| match argument {
                        mojito_ast::ast::ParamArg::Value(values) => list_length(values, subs),
                        _ => None,
                    })
                    .sum()
            }
            _ => None,
        }
    }
    if let Some(construction) = pack_element_construction(e, subs) {
        return Some(construction);
    }
    match &e.kind {
        ExprKind::Member { object, field } if field == "length" => {
            let length = list_length(object, subs)?;
            CtValue::Int(length as i64).materialize(e.span)
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } if name == "len" && param_args.is_empty() && kwargs.is_empty() => {
            let [pack] = args.as_slice() else {
                return None;
            };
            let length = pack_length(pack, subs)?;
            CtValue::Int(length as i64).materialize(e.span)
        }
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } if args.is_empty() && kwargs.is_empty() => {
            let ExprKind::Member { object, field } = &callee.kind else {
                return None;
            };
            if field != "contains" {
                return None;
            }
            let elements = pack_elements(object, subs)?;
            let [needle] = param_args.as_slice() else {
                return None;
            };
            let needle = match needle {
                mojito_ast::ast::ParamArg::Type(ty) => ty.clone(),
                mojito_ast::ast::ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                }) => match subs(name) {
                    Some(CtValue::Type(ty)) => source_type_from_ty(&ty)?,
                    Some(_) => return None,
                    None => Type::Named(name.clone(), Vec::new()),
                },
                mojito_ast::ast::ParamArg::Value(Expr {
                    kind: ExprKind::TypeValue(ty),
                    ..
                }) => ty.clone(),
                _ => return None,
            };
            CtValue::Bool(elements.contains(&needle)).materialize(e.span)
        }
        _ => None,
    }
}

/// Elaborate a bound pack element's default construction — `Self.Ts[i]()`
/// or a `def`'s own `Ts[i]()` at a folded index — into the element type's
/// own concrete construction.
fn pack_element_construction(e: &Expr, subs: Subs) -> Option<Expr> {
    let (pack, param_args) = match &e.kind {
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } if args.is_empty() && kwargs.is_empty() => match &callee.kind {
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") => {
                (field, param_args)
            }
            _ => return None,
        },
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } if args.is_empty() && kwargs.is_empty() => (name, param_args),
        _ => return None,
    };
    let [mojito_ast::ast::ParamArg::Value(index)] = param_args.as_slice() else {
        return None;
    };
    let index = match &index.kind {
        ExprKind::Int(value) => value.to_i64()?,
        ExprKind::Identifier(name) => match subs(name)? {
            CtValue::Int(value) => value,
            CtValue::IntLiteral(value) => value.to_i64()?,
            _ => return None,
        },
        _ => return None,
    };
    let CtValue::Tuple(elements) = subs(pack)? else {
        return None;
    };
    let CtValue::Type(element) = elements.get(usize::try_from(index).ok()?)? else {
        return None;
    };
    pack_element_default_construction(element, e.span, e.syntax_id)
}

/// The `Subs` marker for a name bound as a runtime local (a parameter or a
/// declared variable) rather than a compile-time constant: it materializes
/// as nothing, and a type application on it is a type-keyed accessor call.
pub(super) const fn runtime_local_marker() -> CtValue {
    CtValue::Marker(CtMarker::RuntimeLocal)
}

/// The `Subs` marker for a declared struct name (see `materialize_block`).
const fn type_name_marker() -> CtValue {
    CtValue::Marker(CtMarker::TypeName)
}

/// The value a field chain off a substituted binder (`e.i.v`) selects,
/// when each step names a field of a struct value.
fn substituted_field(e: &Expr, subs: Subs) -> Option<CtValue> {
    match &e.kind {
        ExprKind::Identifier(name) if name != "Self" => subs(name),
        ExprKind::Member { object, field } => match substituted_field(object, subs)? {
            CtValue::Struct { fields, .. } => fields
                .into_iter()
                .find(|(name, _)| name == field)
                .map(|(_, value)| value),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `value`'s materialized expression has the type of the field it
/// was read from: an `Int`, a `Bool`, a `DType`, a vector, or a struct,
/// whose construction names its type. A string, a float, a `UInt`, or a
/// tuple materializes as a literal of a default type instead.
const fn materializes_at_its_type(value: &CtValue) -> bool {
    matches!(
        value,
        CtValue::Int(_)
            | CtValue::Bool(_)
            | CtValue::Dtype(_)
            | CtValue::Simd { .. }
            | CtValue::Struct { .. }
    )
}

fn is_runtime_local(subs: Subs, name: &str) -> bool {
    matches!(subs(name), Some(CtValue::Marker(CtMarker::RuntimeLocal)))
}

/// Rewrite a single-argument type application on a runtime local (`v[Int]`)
/// into the subscript `v[<type>]` the checker projects through the struct's
/// type-keyed `__getitem_param__[T]`.
fn projection_to_subscript(e: &mut Expr, subs: Subs) -> bool {
    let (name, mut param_args) = match &mut e.kind {
        ExprKind::TypeApply { name, args } if is_runtime_local(subs, name) && args.len() == 1 => {
            (name.clone(), std::mem::take(args))
        }
        _ => return false,
    };
    rewrite_param_args(&mut param_args, subs);
    let index = match param_args.pop().expect("one argument") {
        mojito_ast::ast::ParamArg::Type(ty) => Expr::new(ExprKind::TypeValue(ty), e.span),
        mojito_ast::ast::ParamArg::Value(value) => value,
        mojito_ast::ast::ParamArg::Named { value, .. } => match *value {
            mojito_ast::ast::ParamArg::Type(ty) => Expr::new(ExprKind::TypeValue(ty), e.span),
            mojito_ast::ast::ParamArg::Value(value) => value,
            mojito_ast::ast::ParamArg::Named { .. } => return false,
        },
    };
    // The synthesized nodes share the projection's span and source (the
    // checker re-keys duplicate syntax ids before recording facts), so
    // their checked facts stay addressable.
    let mut receiver = e.clone();
    receiver.kind = ExprKind::Identifier(name);
    let mut index = index;
    index.source.clone_from(&e.source);
    e.kind = ExprKind::Index {
        object: Box::new(receiver),
        index: Box::new(index),
    };
    true
}

fn rewrite_block(body: &mut [Stmt], subs: Subs, into_defs: bool) {
    if !into_defs {
        // Loop-variable substitution keeps the flat walk: the unrolled
        // binding is an outer compile-time symbol, and the walk already
        // stops at nested `def`/`struct` scopes.
        for s in body {
            rewrite_stmt(s, subs, into_defs);
        }
        return;
    }
    // Module-constant materialization is shadow-aware within a block: a
    // local declaration rebinds its name for the remainder of the block
    // (and the nested blocks below it), so the constant must stop
    // materializing there — `var n = 1; n += 1` mutates the local, never
    // `5 += 1`. The declaring statement's own initializer still reads the
    // constant (`var n = n + 1` sees the outer value), so the name is
    // shadowed only after its statement is rewritten.
    let mut shadowed: HashSet<String> = HashSet::new();
    for s in body {
        {
            let inner: Subs = &|name| {
                if shadowed.contains(name) {
                    Some(runtime_local_marker())
                } else {
                    subs(name)
                }
            };
            rewrite_stmt(s, inner, into_defs);
        }
        declared_local_names(s, &mut shadowed);
    }
}

/// The names a statement binds for the remainder of its block: explicit
/// `var`/`ref` declarations, implicit function-scope assignments, and
/// declaring unpack targets.
fn declared_local_names(s: &Stmt, names: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::VarDecl { name, .. }
        | StmtKind::RefDecl { name, .. }
        | StmtKind::Assign { name, .. } => {
            names.insert(name.clone());
        }
        StmtKind::Unpack { targets, .. } => {
            for target in targets {
                if let ExprKind::Identifier(name) = &target.kind {
                    names.insert(name.clone());
                }
            }
        }
        _ => {}
    }
}

/// Substitute compile-time loop bindings inside nested type syntax as well as
/// value arguments. A dependent type such as `Ts[i]` stores `i` below a
/// `ParamArg::Type`, so rewriting only top-level value arguments leaves an
/// unbound index in an otherwise-unrolled specialization.
pub(super) fn rewrite_type(ty: &mut Type, subs: Subs) {
    match ty {
        // A bare name bound to a compile-time type (a module alias such as
        // `U128`, or a baked type binder) spells the bound type.
        Type::Named(name, arguments) if arguments.is_empty() => {
            if let Some(CtValue::Type(bound)) = subs(name)
                && let Some(source) = source_type_from_ty(&bound)
            {
                *ty = source;
            }
        }
        Type::Named(name, arguments) => {
            rewrite_param_args(arguments, subs);
            // `Ts[k]` of a bound pack at a folded index spells the element.
            if let [ParamArg::Value(index)] = arguments.as_slice()
                && let Some(element) = bound_pack_element(name, index, subs)
            {
                *ty = element;
            }
        }
        // `Self.T` of a baked type binder spells the bound type.
        Type::SelfParam(name) => {
            if let Some(CtValue::Type(bound)) = subs(name)
                && let Some(source) = source_type_from_ty(&bound)
            {
                *ty = source;
            }
        }
        Type::Assoc { base, args, .. } => {
            rewrite_type(base, subs);
            rewrite_param_args(args, subs);
        }
        Type::IndexedProjection { base, index } => {
            rewrite_expr(index, subs);
            // `Self.Ts[k]` of a bound pack at a folded index spells the
            // element.
            if let Type::SelfParam(pack) | Type::Named(pack, _) = &**base
                && let Some(element) = bound_pack_element(pack, index, subs)
            {
                *ty = element;
                return;
            }
            rewrite_type(base, subs);
        }
        Type::Func {
            type_params,
            params,
            ret,
            capturing,
            raises_type,
            ..
        } => {
            // The contract's own binders shadow same-spelled outer names
            // throughout its signature; outer names may still occur in its
            // bounds and parameter types.
            let own: Vec<String> = type_params
                .iter()
                .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                .collect();
            let subs: Subs = &|name: &str| {
                if own.iter().any(|binder| binder == name) {
                    None
                } else {
                    subs(name)
                }
            };
            for parameter in type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    rewrite_type(value_type, subs);
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    rewrite_type(callable, subs);
                }
                if let Some(mutability) = &mut parameter.origin_mutability {
                    rewrite_expr(mutability, subs);
                }
                if let Some(default) = &mut parameter.default {
                    rewrite_expr(default, subs);
                }
                for constraint in &mut parameter.constraints {
                    rewrite_expr(constraint, subs);
                }
            }
            for parameter in params {
                rewrite_type(&mut parameter.ty, subs);
                if let Some(origins) = &mut parameter.origin {
                    for origin in origins {
                        rewrite_expr(origin, subs);
                    }
                }
            }
            rewrite_type(ret, subs);
            for origin in capturing.iter_mut().flatten() {
                rewrite_expr(origin, subs);
            }
            if let Some(error) = raises_type {
                rewrite_type(error, subs);
            }
        }
        Type::Ref { referent, origin } => {
            rewrite_type(referent, subs);
            for origin in origin.iter_mut().flatten() {
                rewrite_expr(origin, subs);
            }
        }
        Type::Int
        | Type::UInt
        | Type::Bool
        | Type::StringLiteral
        | Type::ClosedStringLiteral
        | Type::Float64
        | Type::None
        | Type::SelfType => {}
    }
}

fn rewrite_exprs(es: &mut [Expr], subs: Subs) {
    for e in es {
        rewrite_expr(e, subs);
    }
}

fn rewrite_param_args(args: &mut [mojito_ast::ast::ParamArg], subs: Subs) {
    for a in args {
        match a {
            // `Self.n` of a baked value parameter in a bracket slot
            // (`Scalar[Self.dtype]`, upstream's required spelling) reads the
            // bound value, like the expression form `Self.n`.
            mojito_ast::ast::ParamArg::Type(Type::SelfParam(name))
                if let Some(value) = subs(name)
                    && !matches!(
                        value,
                        CtValue::Type(_)
                            | CtValue::Reflected(_)
                            | CtValue::Expr(_)
                            | CtValue::Deferred(_)
                            | CtValue::Marker(_)
                    )
                    && let Some(materialized) =
                        value.materialize(mojito_common::token::DUMMY_SPAN) =>
            {
                *a = mojito_ast::ast::ParamArg::Value(materialized);
            }
            mojito_ast::ast::ParamArg::Type(ty) => rewrite_type(ty, subs),
            mojito_ast::ast::ParamArg::Value(e) => {
                rewrite_expr(e, subs);
                if let ExprKind::TypeValue(ty) = &e.kind {
                    *a = mojito_ast::ast::ParamArg::Type(ty.clone());
                }
            }
            mojito_ast::ast::ParamArg::Named { value, .. } => {
                rewrite_param_args(std::slice::from_mut(value), subs);
            }
        }
    }
}

/// Rewrite the parameter arguments of the expression `parent`. A type an
/// alias or binder expands to (`c_int` spelling `SIMD[DType.int32, 1]`)
/// brings expression nodes of its own; each takes an identity derived from
/// `parent`, so every clone of one template names them alike. Ordinals 0
/// and 1 are a vector alias call's dtype and width.
fn rewrite_spelled_param_args(
    args: &mut [mojito_ast::ast::ParamArg],
    subs: Subs,
    parent: mojito_common::token::SyntaxId,
) {
    let mut spelled = Spelled::default();
    for argument in args.iter() {
        mojito_ast::visit::walk_param_arg(&mut spelled, argument);
    }
    rewrite_param_args(args, subs);
    let mut synthesized = spelled.synthesized(parent, 2);
    for argument in args.iter_mut() {
        mojito_ast::visit::walk_param_arg_mut(&mut synthesized, argument);
    }
}

/// Rewrite the annotation of the statement `parent`, naming its expansion's
/// nodes as [`rewrite_spelled_param_args`] does.
fn rewrite_spelled_type(ty: &mut Type, subs: Subs, parent: mojito_common::token::SyntaxId) {
    let mut spelled = Spelled::default();
    mojito_ast::visit::walk_type(&mut spelled, ty);
    rewrite_type(ty, subs);
    mojito_ast::visit::walk_type_mut(&mut spelled.synthesized(parent, 0), ty);
}

/// The expression identities spelled in a node before its rewrite.
#[derive(Default)]
struct Spelled(HashSet<mojito_common::token::SyntaxId>);

impl Spelled {
    fn synthesized(self, parent: mojito_common::token::SyntaxId, next: u32) -> Synthesized {
        Synthesized {
            spelled: self.0,
            parent,
            next,
        }
    }
}

impl mojito_ast::visit::Visitor for Spelled {
    fn visit_expr(&mut self, expr: &Expr) {
        self.0.insert(expr.syntax_id);
    }
}

/// Names each expression a rewrite introduced by its ordinal under `parent`.
struct Synthesized {
    spelled: HashSet<mojito_common::token::SyntaxId>,
    parent: mojito_common::token::SyntaxId,
    next: u32,
}

impl mojito_ast::visit::MutVisitor for Synthesized {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if !self.spelled.contains(&expr.syntax_id) {
            expr.syntax_id = mojito_common::token::SyntaxId::derived(self.parent, self.next);
            self.next += 1;
        }
    }
}

fn rewrite_type_parameter(parameter: &mut TypeParam, subs: Subs) {
    if let Some(value_type) = &mut parameter.value_type {
        rewrite_type(value_type, subs);
    }
    if let Some(callable) = &mut parameter.callable_bound {
        rewrite_type(callable, subs);
    }
    if let Some(mutability) = &mut parameter.origin_mutability {
        rewrite_expr(mutability, subs);
    }
    if let Some(default) = &mut parameter.default {
        rewrite_expr(default, subs);
    }
    rewrite_exprs(&mut parameter.constraints, subs);
}

fn rewrite_fn_parameter(parameter: &mut FnParam, subs: Subs) {
    rewrite_type(&mut parameter.ty, subs);
    if let Some(origins) = &mut parameter.origin {
        rewrite_exprs(origins, subs);
    }
    if let Some(default) = &mut parameter.default {
        rewrite_expr(default, subs);
    }
}

fn rewrite_decorators(decorators: &mut [mojito_ast::ast::Decorator], subs: Subs) {
    for decorator in decorators {
        rewrite_exprs(&mut decorator.args, subs);
        for argument in &mut decorator.kwargs {
            rewrite_expr(&mut argument.value, subs);
        }
    }
}

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    reason = "TODO: split this pass; TODO: split this pass"
)]
fn rewrite_stmt(s: &mut Stmt, subs: Subs, into_defs: bool) {
    match &mut s.kind {
        // A local's annotation is a type position over the clone's bound
        // parameters (`var x: Scalar[dt] = 2.5`), like a signature's.
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                rewrite_spelled_type(ty, subs, s.syntax_id);
            }
            rewrite_expr(value, subs);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value)) => rewrite_expr(value, subs),
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            let shadowed: HashSet<String> = type_params
                .iter()
                .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                .collect();
            let inner: Subs = &|name| {
                if shadowed.contains(name) {
                    None
                } else {
                    subs(name)
                }
            };
            for parameter in type_params {
                rewrite_type_parameter(parameter, inner);
            }
            if let Some(ty) = ty {
                rewrite_type(ty, inner);
            }
            for condition in where_clauses {
                rewrite_expr(condition, inner);
            }
            rewrite_expr(value, inner);
        }
        StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            rewrite_expr(place, subs);
            rewrite_expr(value, subs);
        }
        // A declaring unpack's bare name binds a new local, so it is never a
        // use of a same-named constant.
        StmtKind::Unpack {
            targets,
            value,
            declares,
        } => {
            for target in targets {
                if !(*declares && matches!(target.kind, ExprKind::Identifier(_))) {
                    rewrite_expr(target, subs);
                }
            }
            rewrite_expr(value, subs);
        }
        StmtKind::Expr(e) => rewrite_expr(e, subs),
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (c, b) in branches {
                rewrite_expr(c, subs);
                rewrite_block(b, subs, into_defs);
            }
            if let Some(b) = orelse {
                rewrite_block(b, subs, into_defs);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            rewrite_expr(cond, subs);
            rewrite_block(body, subs, into_defs);
            if let Some(body) = orelse {
                rewrite_block(body, subs, into_defs);
            }
        }
        StmtKind::For {
            var,
            iter,
            body,
            orelse,
            ..
        } => {
            rewrite_expr(iter, subs);
            if into_defs {
                // The loop variable rebinds its name inside the loop (and
                // the for-else, which still reads the loop binding).
                let loop_var = var.clone();
                let inner: Subs = &|name| {
                    if name == loop_var { None } else { subs(name) }
                };
                rewrite_block(body, inner, into_defs);
                if let Some(body) = orelse {
                    rewrite_block(body, inner, into_defs);
                }
            } else {
                rewrite_block(body, subs, into_defs);
                if let Some(body) = orelse {
                    rewrite_block(body, subs, into_defs);
                }
            }
        }
        StmtKind::ComptimeFor { var, iter, body } => {
            rewrite_expr(iter, subs);
            if into_defs {
                // A loop the template keeps binds its index in the body,
                // where it shadows a module constant of its name.
                let index = var.clone();
                let inner: Subs = &|name| if name == index { None } else { subs(name) };
                rewrite_block(body, inner, into_defs);
            } else {
                rewrite_block(body, subs, into_defs);
            }
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            rewrite_block(body, subs, into_defs);
            if let Some((binding, b)) = except {
                if into_defs && let Some(bound) = binding {
                    // The except binding rebinds its name in the handler.
                    let bound = bound.clone();
                    let inner: Subs = &|name| {
                        if name == bound { None } else { subs(name) }
                    };
                    rewrite_block(b, inner, into_defs);
                } else {
                    rewrite_block(b, subs, into_defs);
                }
            }
            if let Some(b) = orelse {
                rewrite_block(b, subs, into_defs);
            }
            if let Some(b) = finalbody {
                rewrite_block(b, subs, into_defs);
            }
        }
        StmtKind::Scope(body) => {
            rewrite_block(body, subs, into_defs);
        }
        StmtKind::With { items, body } => {
            let mut bound_names: HashSet<String> = HashSet::new();
            for WithItem { context, var } in items {
                rewrite_expr(context, subs);
                if into_defs && let Some(name) = var {
                    bound_names.insert(name.clone());
                }
            }
            if bound_names.is_empty() {
                rewrite_block(body, subs, into_defs);
            } else {
                // An `as NAME` binding rebinds its name in the with-body.
                let inner: Subs = &|name| {
                    if bound_names.contains(name) {
                        None
                    } else {
                        subs(name)
                    }
                };
                rewrite_block(body, inner, into_defs);
            }
        }
        // A nested `def`/`struct` is a separate scope. For materialization
        // (`into_defs`), descend but shadow the function's parameters (a parameter
        // named like a module constant is *not* that constant). For loop-variable
        // substitution, don't descend (the loop var is an outer compile-time symbol).
        StmtKind::Def {
            decorators,
            type_params,
            params,
            raises_type,
            ret,
            where_clauses,
            body,
            ..
        } => {
            if into_defs {
                let locals: HashSet<String> = params
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect();
                let shadowed: HashSet<String> = type_params
                    .iter()
                    .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                    .collect();
                let inner: Subs = &|name| {
                    if locals.contains(name) {
                        Some(runtime_local_marker())
                    } else if shadowed.contains(name) {
                        None
                    } else {
                        subs(name)
                    }
                };
                rewrite_decorators(decorators, inner);
                for parameter in type_params {
                    rewrite_type_parameter(parameter, inner);
                }
                for parameter in params {
                    rewrite_fn_parameter(parameter, inner);
                }
                if let Some(error) = raises_type {
                    rewrite_type(error, inner);
                }
                if let Some(ret) = ret {
                    rewrite_type(ret, inner);
                }
                for condition in where_clauses {
                    rewrite_expr(condition, inner);
                }
                rewrite_block(body, inner, into_defs);
            }
        }
        StmtKind::Struct {
            decorators,
            type_params,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            ..
        } => {
            if into_defs {
                let shadowed: HashSet<String> = type_params
                    .iter()
                    .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                    .collect();
                let inner: Subs = &|name| {
                    if shadowed.contains(name) {
                        None
                    } else {
                        subs(name)
                    }
                };
                rewrite_decorators(decorators, inner);
                for parameter in type_params {
                    rewrite_type_parameter(parameter, inner);
                }
                if let Some(callable) = callable_conformance {
                    rewrite_type(callable, inner);
                }
                for (_, condition) in conformance_conditions {
                    rewrite_expr(condition, inner);
                }
                for condition in where_clauses {
                    rewrite_expr(condition, inner);
                }
                for field in fields {
                    rewrite_type(&mut field.ty, inner);
                }
                for member in associated {
                    let member_shadowed: HashSet<String> = member
                        .params
                        .iter()
                        .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                        .collect();
                    let member_subs: Subs = &|name| {
                        if member_shadowed.contains(name) {
                            None
                        } else {
                            inner(name)
                        }
                    };
                    for parameter in &mut member.params {
                        rewrite_type_parameter(parameter, member_subs);
                    }
                    if let Some(ty) = &mut member.ty {
                        rewrite_type(ty, member_subs);
                    }
                    for condition in &mut member.where_clauses {
                        rewrite_expr(condition, member_subs);
                    }
                    rewrite_expr(&mut member.value, member_subs);
                }
                for method in methods {
                    let mut method_locals: HashSet<String> = method
                        .params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect();
                    method_locals.insert("self".to_string());
                    let method_shadowed: HashSet<String> = method
                        .type_params
                        .iter()
                        .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                        .collect();
                    let method_subs: Subs = &|name| {
                        if method_locals.contains(name) {
                            Some(runtime_local_marker())
                        } else if method_shadowed.contains(name) {
                            None
                        } else {
                            inner(name)
                        }
                    };
                    rewrite_decorators(&mut method.decorators, method_subs);
                    for parameter in &mut method.type_params {
                        rewrite_type_parameter(parameter, method_subs);
                    }
                    if let Some(origins) = &mut method.self_origin {
                        rewrite_exprs(origins, method_subs);
                    }
                    for parameter in &mut method.params {
                        rewrite_fn_parameter(parameter, method_subs);
                    }
                    if let Some(error) = &mut method.raises_type {
                        rewrite_type(error, method_subs);
                    }
                    if let Some(ret) = &mut method.ret {
                        rewrite_type(ret, method_subs);
                    }
                    for condition in &mut method.where_clauses {
                        rewrite_expr(condition, method_subs);
                    }
                    rewrite_block(&mut method.body, method_subs, into_defs);
                }
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            if into_defs {
                for method in methods {
                    let mut shadowed: HashSet<String> = method
                        .params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect();
                    shadowed.extend(
                        method
                            .type_params
                            .iter()
                            .map(|parameter| parameter.name.trim_start_matches('*').to_string()),
                    );
                    shadowed.insert("self".to_string());
                    let inner: Subs = &|name| {
                        if shadowed.contains(name) {
                            None
                        } else {
                            subs(name)
                        }
                    };
                    for parameter in &mut method.type_params {
                        rewrite_type_parameter(parameter, inner);
                    }
                    if let Some(origins) = &mut method.self_origin {
                        rewrite_exprs(origins, inner);
                    }
                    for parameter in &mut method.params {
                        rewrite_fn_parameter(parameter, inner);
                    }
                    if let Some(error) = &mut method.raises_type {
                        rewrite_type(error, inner);
                    }
                    if let Some(ret) = &mut method.ret {
                        rewrite_type(ret, inner);
                    }
                    for condition in &mut method.where_clauses {
                        rewrite_expr(condition, inner);
                    }
                    if let Some(body) = &mut method.default_body {
                        rewrite_block(body, inner, into_defs);
                    }
                }
                for member in comptime_members {
                    let shadowed: HashSet<String> = member
                        .params
                        .iter()
                        .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                        .collect();
                    let inner: Subs = &|name| {
                        if shadowed.contains(name) {
                            None
                        } else {
                            subs(name)
                        }
                    };
                    for parameter in &mut member.params {
                        rewrite_type_parameter(parameter, inner);
                    }
                    rewrite_type(&mut member.ty, inner);
                    for condition in &mut member.where_clauses {
                        rewrite_expr(condition, inner);
                    }
                }
            }
        }
    }
}

fn retype_type_parameter(parameter: &mut TypeParam, subs: TypeSubs) {
    if let Some(value_type) = &mut parameter.value_type {
        substitute_type_bindings_in_type(value_type, subs);
    }
    if let Some(callable) = &mut parameter.callable_bound {
        substitute_type_bindings_in_type(callable, subs);
    }
    if let Some(mutability) = &mut parameter.origin_mutability {
        retype_expr(mutability, subs);
    }
    if let Some(default) = &mut parameter.default {
        retype_expr(default, subs);
    }
    retype_exprs(&mut parameter.constraints, subs);
}

fn retype_fn_parameter(parameter: &mut FnParam, subs: TypeSubs) {
    substitute_type_bindings_in_type(&mut parameter.ty, subs);
    if let Some(origins) = &mut parameter.origin {
        retype_exprs(origins, subs);
    }
    if let Some(default) = &mut parameter.default {
        retype_expr(default, subs);
    }
}

fn retype_decorators(decorators: &mut [mojito_ast::ast::Decorator], subs: TypeSubs) {
    for decorator in decorators {
        retype_exprs(&mut decorator.args, subs);
        for argument in &mut decorator.kwargs {
            retype_expr(&mut argument.value, subs);
        }
    }
}

fn retype_stmt(s: &mut Stmt, subs: TypeSubs) {
    match &mut s.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                substitute_type_bindings_in_type(ty, subs);
            }
            retype_expr(value, subs);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value)) => retype_expr(value, subs),
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            let Some(inner) = without_shadowed(subs, type_params) else {
                return;
            };
            for parameter in type_params {
                retype_type_parameter(parameter, &inner);
            }
            if let Some(ty) = ty {
                substitute_type_bindings_in_type(ty, &inner);
            }
            for condition in where_clauses {
                retype_expr(condition, &inner);
            }
            retype_expr(value, &inner);
        }
        StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            retype_expr(place, subs);
            retype_expr(value, subs);
        }
        StmtKind::Unpack { targets, value, .. } => {
            retype_exprs(targets, subs);
            retype_expr(value, subs);
        }
        StmtKind::Expr(e) => retype_expr(e, subs),
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (condition, body) in branches {
                retype_expr(condition, subs);
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            retype_expr(cond, subs);
            substitute_type_bindings_in_block(body, subs);
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::For {
            iter, body, orelse, ..
        } => {
            retype_expr(iter, subs);
            substitute_type_bindings_in_block(body, subs);
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::ComptimeFor { iter, body, .. } => {
            retype_expr(iter, subs);
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            substitute_type_bindings_in_block(body, subs);
            if let Some((_, body)) = except {
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = finalbody {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::Scope(body) => {
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::With { items, body } => {
            for WithItem { context, .. } in items {
                retype_expr(context, subs);
            }
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::Def {
            decorators,
            type_params,
            params,
            raises_type,
            ret,
            where_clauses,
            body,
            ..
        } => {
            let Some(inner) = without_shadowed(subs, type_params) else {
                return;
            };
            retype_decorators(decorators, &inner);
            for parameter in type_params.iter_mut() {
                retype_type_parameter(parameter, &inner);
            }
            for parameter in params {
                retype_fn_parameter(parameter, &inner);
            }
            if let Some(ret) = ret {
                substitute_type_bindings_in_type(ret, &inner);
            }
            if let Some(error) = raises_type {
                substitute_type_bindings_in_type(error, &inner);
            }
            for predicate in where_clauses {
                retype_expr(predicate, &inner);
            }
            substitute_type_bindings_in_block(body, &inner);
        }
        StmtKind::Struct {
            decorators,
            type_params,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            ..
        } => {
            let Some(inner) = without_shadowed(subs, type_params) else {
                return;
            };
            retype_decorators(decorators, &inner);
            for parameter in type_params.iter_mut() {
                retype_type_parameter(parameter, &inner);
            }
            if let Some(callable) = callable_conformance {
                substitute_type_bindings_in_type(callable, &inner);
            }
            for (_, condition) in conformance_conditions {
                retype_expr(condition, &inner);
            }
            for condition in where_clauses {
                retype_expr(condition, &inner);
            }
            for field in fields {
                substitute_type_bindings_in_type(&mut field.ty, &inner);
            }
            for member in associated {
                let Some(member_inner) = without_shadowed(&inner, &member.params) else {
                    continue;
                };
                for parameter in &mut member.params {
                    retype_type_parameter(parameter, &member_inner);
                }
                if let Some(ty) = &mut member.ty {
                    substitute_type_bindings_in_type(ty, &member_inner);
                }
                for condition in &mut member.where_clauses {
                    retype_expr(condition, &member_inner);
                }
                retype_expr(&mut member.value, &member_inner);
            }
            for method in methods {
                let Some(method_inner) = without_shadowed(&inner, &method.type_params) else {
                    continue;
                };
                retype_decorators(&mut method.decorators, &method_inner);
                for parameter in &mut method.type_params {
                    retype_type_parameter(parameter, &method_inner);
                }
                if let Some(origins) = &mut method.self_origin {
                    retype_exprs(origins, &method_inner);
                }
                for parameter in &mut method.params {
                    retype_fn_parameter(parameter, &method_inner);
                }
                if let Some(ret) = &mut method.ret {
                    substitute_type_bindings_in_type(ret, &method_inner);
                }
                if let Some(error) = &mut method.raises_type {
                    substitute_type_bindings_in_type(error, &method_inner);
                }
                for predicate in &mut method.where_clauses {
                    retype_expr(predicate, &method_inner);
                }
                substitute_type_bindings_in_block(&mut method.body, &method_inner);
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            for method in methods {
                let Some(inner) = without_shadowed(subs, &method.type_params) else {
                    continue;
                };
                for parameter in &mut method.type_params {
                    retype_type_parameter(parameter, &inner);
                }
                if let Some(origins) = &mut method.self_origin {
                    retype_exprs(origins, &inner);
                }
                for parameter in &mut method.params {
                    retype_fn_parameter(parameter, &inner);
                }
                if let Some(ret) = &mut method.ret {
                    substitute_type_bindings_in_type(ret, &inner);
                }
                if let Some(error) = &mut method.raises_type {
                    substitute_type_bindings_in_type(error, &inner);
                }
                for condition in &mut method.where_clauses {
                    retype_expr(condition, &inner);
                }
                if let Some(body) = &mut method.default_body {
                    substitute_type_bindings_in_block(body, &inner);
                }
            }
            for member in comptime_members {
                let Some(inner) = without_shadowed(subs, &member.params) else {
                    continue;
                };
                for parameter in &mut member.params {
                    retype_type_parameter(parameter, &inner);
                }
                substitute_type_bindings_in_type(&mut member.ty, &inner);
                for condition in &mut member.where_clauses {
                    retype_expr(condition, &inner);
                }
            }
        }
    }
}

fn retype_expr(e: &mut Expr, subs: TypeSubs) {
    match &mut e.kind {
        ExprKind::Identifier(_) => {}
        ExprKind::Spread(value) => retype_expr(value, subs),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::EmptySubscript => {}
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let mojito_ast::ast::TStringPart::Expr(value) = part {
                    retype_expr(value, subs);
                }
            }
        }
        ExprKind::TypeApply { name, args } => {
            retype_head(name, args, subs);
            retype_param_args(args, subs);
        }
        ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) => retype_expr(inner, subs),
        ExprKind::Infix(_, l, r) => {
            retype_expr(l, subs);
            retype_expr(r, subs);
        }
        ExprKind::Compare { first, rest } => {
            retype_expr(first, subs);
            for (_, r) in rest {
                retype_expr(r, subs);
            }
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => {
            retype_head(name, param_args, subs);
            retype_param_args(param_args, subs);
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
        }
        ExprKind::Member { object, .. } => retype_expr(object, subs),
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            retype_exprs(args, subs);
            for k in kwargs.iter_mut() {
                retype_expr(&mut k.value, subs);
            }
            // `Self.T(...)` on a dropped binding constructs the bound type,
            // spelled as the ordinary constructor call it names.
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && subs.contains_key(method.as_str())
            {
                let mut name = method.clone();
                let mut param_args = Vec::new();
                retype_head(&mut name, &mut param_args, subs);
                if name != *method {
                    e.kind = ExprKind::Call {
                        name,
                        param_args,
                        args: std::mem::take(args),
                        kwargs: std::mem::take(kwargs),
                    };
                    return;
                }
            }
            retype_expr(object, subs);
        }
        ExprKind::Index { object, index } => {
            retype_expr(object, subs);
            retype_expr(index, subs);
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            retype_expr(object, subs);
            for b in [lower, upper, step].into_iter().flatten() {
                retype_expr(b, subs);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            retype_expr(object, subs);
            for argument in args {
                match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        retype_expr(value, subs);
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            retype_expr(value, subs);
                        }
                    }
                }
            }
        }
        ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => retype_exprs(elems, subs),
        ExprKind::TypeValue(ty) => substitute_type_bindings_in_type(ty, subs),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            retype_expr(callee, subs);
            retype_param_args(param_args, subs);
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                retype_expr(key, subs);
                if let Some(value) = value {
                    retype_expr(value, subs);
                }
            }
        }
        // Comprehension targets bind runtime values, which cannot shadow a
        // type-parameter binding in type position, so descend unconditionally.
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            if let Some(key) = key {
                retype_expr(key, subs);
            }
            retype_expr(value, subs);
            for clause in clauses {
                match clause {
                    mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                        retype_expr(iter, subs);
                    }
                    mojito_ast::ast::ComprehensionClause::If(condition) => {
                        retype_expr(condition, subs);
                    }
                }
            }
        }
        ExprKind::Uninitialized => {}
        ExprKind::Named { value, .. } => retype_expr(value, subs),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            retype_expr(cond, subs);
            retype_expr(then_branch, subs);
            retype_expr(else_branch, subs);
        }
        // A lambda's hidden definition retypes like the equivalent nested
        // `def` statement (signature and body, minus its shadowed names).
        ExprKind::Lambda { def } => retype_stmt(def, subs),
    }
}

fn retype_param_args(args: &mut Vec<ParamArg>, subs: TypeSubs) {
    for argument in args {
        retype_param_arg(argument, subs);
    }
}

fn retype_param_arg(argument: &mut ParamArg, subs: TypeSubs) {
    match argument {
        ParamArg::Type(ty) => substitute_type_bindings_in_type(ty, subs),
        ParamArg::Named { value, .. } => retype_param_arg(value, subs),
        // The parser encodes a bare identifier argument (`pick[T]`) as a value
        // expression; once the binding is concrete it is a type argument.
        ParamArg::Value(expr) => {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(replacement) = subs.get(name)
            {
                *argument = ParamArg::Type(replacement.clone());
            } else {
                retype_expr(expr, subs);
            }
        }
    }
}

/// Rewrite a constructor/type-application head that names a dropped binding
/// (`T(…)` / `T[…]`) to the concrete type's own head, prepending the concrete
/// type's arguments (`List[Int](…)` is head `List` with argument `[Int]`).
fn retype_head(name: &mut String, args: &mut Vec<ParamArg>, subs: TypeSubs) {
    let Some(replacement) = subs.get(name.as_str()) else {
        return;
    };
    let (head, head_args) = match replacement {
        Type::Named(head, head_args) => (head.clone(), head_args.clone()),
        Type::Int => ("Int".to_string(), Vec::new()),
        Type::UInt => ("UInt".to_string(), Vec::new()),
        Type::Bool => ("Bool".to_string(), Vec::new()),
        Type::StringLiteral | Type::ClosedStringLiteral => ("String".to_string(), Vec::new()),
        Type::Float64 => ("Float64".to_string(), Vec::new()),
        // No source constructor head exists (function types, references);
        // leave the call for the checker to report against the clone.
        _ => return,
    };
    *name = head;
    let mut merged = head_args;
    merged.append(args);
    *args = merged;
}

fn retype_exprs(exprs: &mut [Expr], subs: TypeSubs) {
    for value in exprs {
        retype_expr(value, subs);
    }
}

/// The substitutions still live inside a nested declaration that introduces its
/// own type parameters: a same-named parameter shadows the outer binding.
/// `None` means nothing is left to substitute.
fn without_shadowed(subs: TypeSubs, type_params: &[TypeParam]) -> Option<HashMap<String, Type>> {
    let inner: HashMap<String, Type> = subs
        .iter()
        .filter(|(binding, _)| {
            !type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == binding.as_str())
        })
        .map(|(binding, replacement)| (binding.clone(), replacement.clone()))
        .collect();
    if inner.is_empty() { None } else { Some(inner) }
}

/// The concrete default construction a bound pack element's `Ts[i]()`
/// elaborates to, its nodes identified by identities derived from the
/// construction's own (`parent`), so every copy builds the same syntax.
fn pack_element_default_construction(
    element: &Ty,
    span: Span,
    parent: mojito_common::token::SyntaxId,
) -> Option<Expr> {
    let mut construction = default_constructor_call(&source_type_from_ty(element)?, element, span)?;
    let mut identities = mojito_ast::visit::DerivedIdentities { parent, next: 0 };
    mojito_ast::visit::walk_expr_mut(&mut identities, &mut construction);
    construction.syntax_id = parent;
    Some(construction)
}

fn default_constructor_call(ty: &Type, semantic: &Ty, span: Span) -> Option<Expr> {
    // A SIMD element default-constructs to zero lanes: the checker accepts one
    // lane to splat, not a nullary construction, so spell the zero explicitly.
    if let (Ty::Simd { dtype, .. }, Type::Named(name, arguments)) = (semantic, ty) {
        let zero = match dtype.known() {
            Some(mojito_ast::ast::Dtype::Bool) => ExprKind::Bool(false),
            Some(dtype) if dtype.is_float() => ExprKind::Float(0.0.into()),
            // An integer literal splats into any numeric lane, a symbolic one
            // included.
            _ => ExprKind::Int(0.into()),
        };
        return Some(Expr::new(
            ExprKind::Call {
                name: name.clone(),
                param_args: arguments.clone(),
                args: vec![Expr::new(zero, span)],
                kwargs: Vec::new(),
            },
            span,
        ));
    }
    // A scalar element converts its zero literal explicitly, so the storage
    // is built at exactly the element types, with no literal left for the
    // store to materialize.
    let scalar = match ty {
        Type::Int => Some(("Int", ExprKind::Int(0.into()))),
        Type::UInt => Some(("UInt", ExprKind::Int(0.into()))),
        Type::Bool => Some(("Bool", ExprKind::Bool(false))),
        Type::Float64 => Some(("Float64", ExprKind::Float(0.0.into()))),
        _ => None,
    };
    if let Some((name, zero)) = scalar {
        return Some(Expr::new(
            ExprKind::Call {
                name: name.to_string(),
                param_args: Vec::new(),
                args: vec![Expr::new(zero, span)],
                kwargs: Vec::new(),
            },
            span,
        ));
    }
    let literal = match ty {
        Type::StringLiteral | Type::ClosedStringLiteral => Some(ExprKind::Str(String::new())),
        Type::None => Some(ExprKind::None),
        _ => None,
    };
    if let Some(kind) = literal {
        return Some(Expr::new(kind, span));
    }
    let (name, param_args) = match ty {
        Type::Named(name, arguments) => (
            name.clone(),
            // A specialized (mangled) name has its arguments baked in; an
            // open application (`Tuple[Int, Bool]`, `Optional[Int]`)
            // keeps them so the element constructs through its own
            // specialization.
            if name.contains('$') {
                Vec::new()
            } else {
                arguments.clone()
            },
        ),
        _ => return None,
    };
    Some(Expr::new(
        ExprKind::Call {
            name,
            param_args,
            args: Vec::new(),
            kwargs: Vec::new(),
        },
        span,
    ))
}
