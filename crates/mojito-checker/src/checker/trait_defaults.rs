//! Trait defaults as upstream emits them: each default is one function over
//! its trait's `Self`, checked and lowered once, and a conformer that does
//! not spell a defaulted method gets a forwarding wrapper calling it
//! (`StructEmitter::synthesizeDefaultTraitMethodWrapper` and
//! `populateDefaultedTraitFunction`).

use std::collections::{HashMap, HashSet};

use mojito_ast::ast::{
    ArgConvention, Decorator, Expr, ExprKind, FnParam, KwArg, Method, MethodProvenance, ParamArg,
    ParamKind, Stmt, StmtKind, TRAIT_DEFAULT_DECORATOR, TraitMethod, Type, TypeParam,
};
use mojito_ast::visit::{MutVisitor, walk_stmt_mut};
use mojito_common::error::TypeError;
use mojito_common::token::Span;
use mojito_symbol::symbol::SignatureKey;

/// Give every trait default its function and every conformer a wrapper for
/// each default it inherits and does not spell.
///
/// The function is named by [`default_function_name`]: its first
/// compile-time parameter is the trait's `Self` (spelled `_Self`, as
/// upstream's diagnostics name it), its first argument the receiver, and its
/// body the default's. A conformer's wrapper keeps the default's signature,
/// with `Self` now the conformer, and its body is one call of that function
/// with `Self` bound first and every argument forwarded. Defaults are
/// inherited per overload: a conformer method suppresses only the default of
/// its own shape.
///
/// The check binds a module function where it is declared, so a function is
/// declared ahead of the first conformer that calls it, and otherwise at the
/// end of the program, where its body sees every declaration.
///
/// `comptime::prepare` runs this before elaboration; the checker's own call
/// finds every function and wrapper in place on prepared source.
pub fn synthesize_trait_default_wrappers(stmts: &[Stmt]) -> Result<Vec<Stmt>, TypeError> {
    let traits: HashMap<&str, (&[String], &[TraitMethod])> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Trait {
                name,
                refines,
                methods,
                ..
            } => Some((name.as_str(), (refines.as_slice(), methods.as_slice()))),
            _ => None,
        })
        .collect();
    let functions: HashSet<&str> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Def { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut pending: Vec<(String, Stmt)> = Vec::new();
    for stmt in stmts {
        if let StmtKind::Trait { name, methods, .. } = &stmt.kind {
            for (index, method) in methods.iter().enumerate() {
                let function = default_function_name(name, methods, index);
                if method.default_body.is_some() && !functions.contains(function.as_str()) {
                    let declaration = default_function(stmt, name, method, function.clone());
                    pending.push((function, declaration));
                }
            }
        }
    }
    let mut out = Vec::with_capacity(stmts.len() + pending.len());
    for stmt in stmts {
        let mut stmt = stmt.clone();
        if matches!(stmt.kind, StmtKind::Struct { .. }) {
            let called = add_wrappers(&mut stmt, &traits)?;
            let (now, later): (Vec<_>, Vec<_>) = pending
                .into_iter()
                .partition(|(function, _)| called.contains(function));
            out.extend(now.into_iter().map(|(_, declaration)| declaration));
            pending = later;
        }
        out.push(stmt);
    }
    out.extend(pending.into_iter().map(|(_, declaration)| declaration));
    Ok(out)
}

/// The name of the function the default `methods[index]` of trait `trait_name`
/// lowers to: `Trait$default$method`, with the default's position among its
/// trait's same-name defaults appended when that name is overloaded. Named by
/// declaration, so it is stable across packages; the `$` keeps it apart from
/// every source name.
fn default_function_name(trait_name: &str, methods: &[TraitMethod], index: usize) -> String {
    let name = &methods[index].name;
    let same_name = |method: &&TraitMethod| method.default_body.is_some() && method.name == *name;
    let base = format!("{trait_name}$default${name}");
    if methods.iter().filter(same_name).count() > 1 {
        let ordinal = methods[..index].iter().filter(same_name).count();
        format!("{base}${ordinal}")
    } else {
        base
    }
}

/// One default a conformer inherits: its trait, the function it lowers to,
/// and its declaration.
struct Inherited<'a> {
    trait_name: &'a str,
    function: String,
    method: &'a TraitMethod,
}

/// Add to struct `stmt` the wrapper of every default it inherits and does
/// not spell, returning the functions the wrappers call.
fn add_wrappers(
    stmt: &mut Stmt,
    traits: &HashMap<&str, (&[String], &[TraitMethod])>,
) -> Result<Vec<String>, TypeError> {
    let (module, span) = (stmt.module.clone(), stmt.span);
    let StmtKind::Struct {
        name,
        type_params,
        conforms,
        methods,
        ..
    } = &mut stmt.kind
    else {
        return Ok(Vec::new());
    };
    let explicit: Vec<_> = methods
        .iter()
        .map(|method| {
            overload_key(
                &method.name,
                &method.params,
                method.keyword_only,
                Some(name),
            )
        })
        .collect();
    let mut reached: Vec<(OverloadKey, Inherited)> = Vec::new();
    for trait_name in conforms.iter() {
        defaults_for(trait_name, traits, &mut HashSet::new(), &mut reached)?;
    }
    let mut inherited: Vec<(OverloadKey, Inherited)> = Vec::new();
    for (key, default) in reached {
        if explicit.contains(&key) {
            continue;
        }
        if let Some((_, original)) = inherited.iter().find(|(seen, _)| *seen == key) {
            return Err(TypeError::ConflictingTraitDefaults {
                method: default.method.name.clone(),
                original: original.trait_name.to_string(),
                conflicting: default.trait_name.to_string(),
            });
        }
        inherited.push((key, default));
    }
    let mut called = Vec::with_capacity(inherited.len());
    for (_, default) in inherited {
        if let Some(conflict) = default.method.type_params.iter().find(|param| {
            type_params
                .iter()
                .any(|own| param_name(own) == param_name(param))
        }) {
            return Err(TypeError::TraitDefaultParameterConflict(
                param_name(conflict).to_string(),
            ));
        }
        methods.push(wrapper(&default, module.as_deref(), span)?);
        called.push(default.function);
    }
    Ok(called)
}

/// A default method's identity among its same-name overloads: its name and
/// its parameters' overload signature, with the conformer's own name read as
/// `Self` so a method spelling either suppresses the default.
#[derive(PartialEq, Eq)]
struct OverloadKey(String, SignatureKey);

fn overload_key(
    name: &str,
    params: &[FnParam],
    keyword_only: Option<usize>,
    conformer: Option<&str>,
) -> OverloadKey {
    let params: Vec<FnParam> = params
        .iter()
        .filter(|param| !param.is_named_result())
        .map(|param| {
            let mut param = param.clone();
            if matches!(&param.ty, Type::Named(named, args) if args.is_empty() && Some(named.as_str()) == conformer)
            {
                param.ty = Type::SelfType;
            }
            param
        })
        .collect();
    let collector = params
        .iter()
        .position(|param| param.kind == ParamKind::Variadic);
    let keyword_names = params
        .iter()
        .enumerate()
        .filter(|(index, param)| {
            param.kind == ParamKind::Regular
                && [keyword_only, collector]
                    .into_iter()
                    .flatten()
                    .any(|start| *index >= start)
        })
        .map(|(_, param)| param.name.clone())
        .collect();
    OverloadKey(
        name.to_string(),
        SignatureKey::from_ast_params(&params).with_keyword_names(keyword_names),
    )
}

/// Add to `reached` every default trait `name` gives a conformer, its
/// refinements' first, each once however many paths reach it.
fn defaults_for<'a>(
    name: &'a str,
    traits: &HashMap<&'a str, (&'a [String], &'a [TraitMethod])>,
    visiting: &mut HashSet<String>,
    reached: &mut Vec<(OverloadKey, Inherited<'a>)>,
) -> Result<(), TypeError> {
    if !visiting.insert(name.to_string()) {
        return Err(TypeError::Unsupported(format!(
            "cyclic trait refinement involving '{name}'"
        )));
    }
    let Some(&(refines, methods)) = traits.get(name) else {
        visiting.remove(name);
        return Ok(());
    };
    for parent in refines {
        defaults_for(parent, traits, visiting, reached)?;
    }
    for (index, method) in methods.iter().enumerate() {
        let function = default_function_name(name, methods, index);
        if method.default_body.is_none()
            || reached
                .iter()
                .any(|(_, default)| default.function == function)
        {
            continue;
        }
        let key = overload_key(&method.name, &method.params, method.keyword_only, None);
        reached.push((
            key,
            Inherited {
                trait_name: name,
                function,
                method,
            },
        ));
    }
    visiting.remove(name);
    Ok(())
}

/// The function default `method` of trait `trait_name` lowers to, declared
/// beside the trait statement `trait_stmt`.
fn default_function(
    trait_stmt: &Stmt,
    trait_name: &str,
    method: &TraitMethod,
    name: String,
) -> Stmt {
    let self_param = TypeParam {
        name: SELF_PARAM.to_string(),
        bounds: vec![trait_name.to_string()],
        value_type: None,
        callable_bound: None,
        origin_mutability: None,
        // The receiver's type infers it, so it joins an infer-only section
        // the default's own parameters open.
        infer_only: method
            .type_params
            .first()
            .is_some_and(|param| param.infer_only),
        default: None,
        constraints: Vec::new(),
    };
    let receiver = FnParam {
        name: "self".to_string(),
        ty: Type::Named(SELF_PARAM.to_string(), Vec::new()),
        default: None,
        kind: ParamKind::Regular,
        convention: method.self_convention,
        origin: method.self_origin.clone(),
    };
    let mut function = Stmt {
        kind: StmtKind::Def {
            name,
            decorators: vec![Decorator {
                path: vec![TRAIT_DEFAULT_DECORATOR.to_string()],
                args: Vec::new(),
                kwargs: Vec::new(),
            }],
            type_params: std::iter::once(self_param)
                .chain(method.type_params.iter().cloned())
                .collect(),
            params: std::iter::once(receiver)
                .chain(method.params.iter().cloned())
                .collect(),
            positional_only: method.positional_only.map(|index| index + 1),
            keyword_only: method.keyword_only.map(|index| index + 1),
            captures: None,
            raises: method.raises,
            raises_type: method.raises_type.clone(),
            ret: method.ret.clone(),
            where_clauses: method.where_clauses.clone(),
            body: method.default_body.clone().unwrap_or_default(),
        },
        span: trait_stmt.span,
        module: trait_stmt.module.clone(),
        syntax_id: mojito_common::token::SyntaxId::fresh(),
    };
    walk_stmt_mut(&mut SpellSelfParam, &mut function);
    if let StmtKind::Def { type_params, .. } = &mut function.kind {
        for param in type_params {
            for bound in &mut param.bounds {
                if let Some(rest) = bound.strip_prefix("Self.") {
                    *bound = format!("{SELF_PARAM}.{rest}");
                }
            }
        }
    }
    function
}

/// The trait's `Self` as its default's function names it.
const SELF_PARAM: &str = "_Self";

/// Respell the trait's `Self` as the function's [`SELF_PARAM`].
struct SpellSelfParam;

impl MutVisitor for SpellSelfParam {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            ExprKind::Identifier(name) | ExprKind::Call { name, .. } if name == "Self" => {
                *name = SELF_PARAM.to_string();
            }
            _ => {}
        }
    }

    fn visit_type_mut(&mut self, ty: &mut Type) {
        match ty {
            Type::SelfType => *ty = Type::Named(SELF_PARAM.to_string(), Vec::new()),
            Type::SelfParam(member) if !member.starts_with('*') => {
                *ty = Type::Assoc {
                    base: Box::new(Type::Named(SELF_PARAM.to_string(), Vec::new())),
                    name: std::mem::take(member),
                    args: Vec::new(),
                };
            }
            _ => {}
        }
    }
}

/// The wrapper a conformer gets for `default`: the default's signature, with
/// one statement calling its function, `Self` bound first and every argument
/// forwarded as `populateDefaultedTraitFunction` forwards it.
fn wrapper(default: &Inherited, module: Option<&str>, span: Span) -> Result<Method, TypeError> {
    let method = default.method;
    let expr = |kind| Expr::new(kind, span);
    let name = |name: &str| expr(ExprKind::Identifier(name.to_string()));
    let transfer = |param: &str| expr(ExprKind::Transfer(Box::new(name(param))));
    let mut param_args = Vec::new();
    if !method
        .type_params
        .first()
        .is_some_and(|param| param.infer_only)
    {
        param_args.push(ParamArg::Type(Type::SelfType));
    }
    for param in &method.type_params {
        if param.infer_only
            || param.name.starts_with('*')
            || param.is_erased_origin_param(&method.type_params)
        {
            continue;
        }
        param_args.push(ParamArg::Named {
            name: param.name.clone(),
            value: Box::new(ParamArg::Value(name(&param.name))),
        });
    }
    let consumes_receiver = matches!(
        method.self_convention,
        Some(ArgConvention::Var | ArgConvention::Deinit)
    );
    let mut args = vec![if consumes_receiver {
        transfer("self")
    } else {
        name("self")
    }];
    let mut kwargs = Vec::new();
    let mut keyword = false;
    let mut named_result = None;
    for (index, param) in method.params.iter().enumerate() {
        keyword |= method.keyword_only == Some(index);
        let owned = matches!(
            param.convention,
            Some(ArgConvention::Var | ArgConvention::Deinit)
        );
        match param.kind {
            _ if param.is_named_result() => named_result = Some(param.name.clone()),
            ParamKind::Regular => {
                let value = if owned {
                    transfer(&param.name)
                } else {
                    name(&param.name)
                };
                if keyword {
                    kwargs.push(KwArg {
                        name: param.name.clone(),
                        value,
                    });
                } else {
                    args.push(value);
                }
            }
            ParamKind::Variadic => {
                if !matches!(&param.ty, Type::Named(pack, _) if pack.starts_with('*')) {
                    return Err(TypeError::Unsupported(format!(
                        "forwarding the homogeneous variadic '*{}' of trait default '{}' to a \
                         conformer: a homogeneous variadic spread into a call is not supported \
                         yet",
                        param.name, method.name
                    )));
                }
                let pack = if owned {
                    transfer(&param.name)
                } else {
                    name(&param.name)
                };
                args.push(expr(ExprKind::Spread(Box::new(pack))));
                keyword = true;
            }
            ParamKind::KwVariadic => kwargs.push(KwArg {
                name: "**".to_string(),
                value: transfer(&param.name),
            }),
        }
    }
    let call = expr(ExprKind::Call {
        name: default.function.clone(),
        param_args,
        args,
        kwargs,
    });
    let statement = match (named_result, &method.ret) {
        (Some(result), _) => StmtKind::Assign {
            name: result,
            value: call,
        },
        (None, None) => StmtKind::Expr(call),
        (None, Some(_)) => StmtKind::Return(Some(call)),
    };
    let mut body = vec![Stmt::new(statement, span)];
    if let Some(module) = module {
        mojito_ast::ast::stamp_source(&mut body, module);
    }
    Ok(Method {
        name: method.name.clone(),
        type_params: method.type_params.clone(),
        has_self: true,
        self_convention: method.self_convention,
        self_origin: method.self_origin.clone(),
        decorators: Vec::new(),
        params: method.params.clone(),
        positional_only: method.positional_only,
        keyword_only: method.keyword_only,
        raises: method.raises,
        raises_type: method.raises_type.clone(),
        ret: method.ret.clone(),
        where_clauses: method.where_clauses.clone(),
        body,
        provenance: MethodProvenance::TraitDefaultWrapper,
    })
}

/// A compile-time parameter's name without a pack's leading `*`.
fn param_name(param: &TypeParam) -> &str {
    param.name.trim_start_matches('*')
}
