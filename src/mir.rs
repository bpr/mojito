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

mod calls;
mod facts;
mod lower_expr;
mod lower_stmt;

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
        &HashMap::new(),
    )
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
                            m.keyword_only,
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
                        m.keyword_only,
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
                    let mut param_decls = checked
                        .generic_parameters_at(&generic_site)
                        .unwrap_or(&[])
                        .to_vec();
                    if method_name == "__init__" {
                        // A constructor's compile-time interface is the struct's
                        // parameter list: an explicit `Array[Int, 3](fill=7)`
                        // call supplies the struct arguments, and the VM reifies
                        // the constructed value's `value_params` in this
                        // declaration order.
                        let struct_decls = checked
                            .generic_parameters_at(&GenericSite::Struct {
                                module: s.module.clone(),
                                declaration: name.clone(),
                            })
                            .unwrap_or(&[]);
                        param_decls = struct_decls.iter().cloned().chain(param_decls).collect();
                    }
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
            // A generic comptime alias is a pure type declaration the checker
            // consumed; like a struct or trait it has no runtime form.
            StmtKind::Comptime { type_params, .. } if !type_params.is_empty() => {}
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
        checked.call_transfers(),
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

/// An expression's source span, stamped by the parser (`ast::Expr.span`). Fed
/// into the [`SpanTable`] so each temporary can be traced back to its origin.
fn span(e: &Expr) -> SourceSpan {
    e.source_span()
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
            crate::origin::PointerOrigin::Param { mutability, .. }
            | crate::origin::PointerOrigin::SelfPlace { mutability, .. } => Some(*mutability),
            crate::origin::PointerOrigin::Static
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
    /// Accumulated transferred loans per interior destination domain
    /// (`(root, path)`), so repeated transfers into one domain merge while
    /// sibling domains keep independent generations.
    transfer_domain_loans: HashMap<(VarId, Vec<crate::origin::OriginSeg>), Vec<MirLoan>>,
    /// Checker-substituted loan transfers per call occurrence: after the
    /// call, the destination actual's root receives the sources' loans.
    call_transfers: HashMap<SourceSpan, Vec<crate::checked::CheckedCallTransfer>>,
    /// Names rebound more than once, or captured by a nested `def`. A pointer
    /// variable outside this set keeps one statically known loan place for its
    /// whole live range, so deref sites may substitute the owner place.
    reassigned_names: std::collections::HashSet<String>,
    returns_reference: bool,
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
    /// Whether an expression's checked type is a pointer whose provenance
    /// designates a single checked place. Dereferencing such a pointer goes
    /// through its frame/slot handle instead of allocation arithmetic. A
    /// multi-element origin-bearing pointer (an interior-generation domain)
    /// is heap storage: it keeps allocation arithmetic and carries its loan
    /// on the binding instead.
    fn is_origin_bearing_pointer(&self, expression: &Expr) -> bool {
        matches!(
            self.checked_ty(expression),
            Some(Ty::Pointer { origin, .. })
                if origin.as_origin().is_some() && !origin.multi_element()
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
        matches!(self.checked_ty(object), Some(Ty::StringLiteral))
            .then_some(MirIntrinsicSubscript::String)
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

#[derive(Clone)]
struct ExprFacts {
    ty: Option<Ty>,
    place_ty: Option<Ty>,
    owner: Option<crate::origin::OwnerId>,
    raises: Option<Ty>,
    adjustments: Vec<crate::SemanticAdjustment>,
    comprehension_bindings: Vec<crate::checked::CheckedComprehensionBinding>,
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
    call_transfers: &HashMap<SourceSpan, Vec<crate::checked::CheckedCallTransfer>>,
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
            call_transfers: call_transfers.clone(),
            active_semantics: Vec::new(),
            aliases: HashMap::new(),
            runtime_aliases: reference_parameters
                .iter()
                .take(cfg.n_params)
                .enumerate()
                .filter_map(|(slot, reference)| reference.then_some(slot as VarId))
                .collect(),
            aggregate_loans: HashMap::new(),
            transfer_domain_loans: HashMap::new(),
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

#[derive(Clone, PartialEq, Eq)]
struct NestedCapture {
    name: String,
    binding: crate::origin::OwnerId,
    ty: Ty,
    kind: crate::ast::CaptureKind,
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
                    crate::ast::SubscriptArg::Index(value)
                    | crate::ast::SubscriptArg::Keyword { value, .. } => children.push(value),
                    crate::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | crate::ast::SubscriptArg::KeywordSlice {
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
                            Const::Str(_) => Some(Ty::StringLiteral),
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
                    }
                    | MirInstr::SimdCast {
                        dest, dtype, width, ..
                    } => Some((
                        dest,
                        Some(Ty::Simd {
                            dtype: *dtype,
                            width: *width as i64,
                        }),
                    )),
                    // A shuffle keeps the source dtype at the mask's width.
                    MirInstr::SimdShuffle { dest, value, mask } => {
                        let dtype = match reg_types.get(&value.0) {
                            Some(Ty::Simd { dtype, .. }) => Some(*dtype),
                            _ => None,
                        };
                        Some((
                            dest,
                            dtype.map(|dtype| Ty::Simd {
                                dtype,
                                width: mask.len() as i64,
                            }),
                        ))
                    }
                    MirInstr::VariantIs { dest, .. } => Some((dest, Some(Ty::Bool))),
                    MirInstr::VariantSetInitWith { dest, .. }
                    | MirInstr::VariantDeinitWith { dest, .. } => Some((dest, Some(Ty::None))),
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
                    MirInstr::Next { dest, iter, call } => {
                        // Nominal iteration carries its checked executable
                        // result directly. Only compiler-private storage has no
                        // call contract and needs the binding/slot fallback.
                        let result = call
                            .as_ref()
                            .map(|call| call.result_ty.clone())
                            .or_else(|| {
                                blocks
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
                                        MirInstr::Store { place, src } if src == dest => {
                                            place.ty.clone()
                                        }
                                        _ => None,
                                    })
                            })
                            .or_else(|| {
                                var_tys.get(iter).and_then(|ty| match ty {
                                    Ty::Struct(name, _) => {
                                        declared_return(declarations, &format!("{name}.__next__"))
                                    }
                                    _ => None,
                                })
                            });
                        Some((dest, result))
                    }
                    MirInstr::TryNext { dest, call, .. } => {
                        Some((dest, Some(call.result_ty.clone())))
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
        StmtKind::Unpack { targets, value, .. } => {
            let mut roots: Vec<&Expr> = targets.iter().collect();
            roots.push(value);
            roots
        }
        StmtKind::Return(Some(value)) => vec![value],
        StmtKind::With { items, .. } => items.iter().map(|item| &item.context).collect(),
        _ => Vec::new(),
    }
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

mod nested;

use nested::*;
