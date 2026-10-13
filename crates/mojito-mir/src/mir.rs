//! Stage 5: flatten the HIR's nested expressions into **A-Normal Form**.
//!
//! Every subexpression becomes a `MirInstr` writing a fresh [`Reg`] — three-
//! address code — so `foo(bar(x))` becomes `t0 = bar(x); t1 = foo(t0)`.
//!
//! The flattened form is what Stages 6–7 dataflow analysis (liveness / move /
//! borrow) runs over, and what the backends consume.
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

use mojito_ast::ast::DeclaredParam;
use mojito_ast::ast::{ArgConvention, Dtype, InfixOp, ParamKind, PrefixOp, StmtKind};
use mojito_ast::call::{effective_keyword_only_index, regular_marker_index};
use mojito_checked::checked::{AnnotationSite, GenericSite};
use mojito_checked::checked::{
    CheckedConst, CheckedNodeId, CheckedOp, CheckedParamArg, CheckedProgram, CheckedStmt,
    CheckedStmtId, CheckedStmtOp, WalkResult, Walked,
};
pub use mojito_checked::declaration::{
    FunctionDeclaration as MirFunctionDeclaration, StructDeclaration as MirStructDeclaration,
    TraitDeclaration, TraitRequirement,
};
use mojito_checked::unit::LoweringUnit;
use mojito_common::timing;
use mojito_common::token::{DUMMY_SPAN, SourceSpan};
use mojito_hir::hir::{self, Cfg, HirInstr, Terminator, VarId};
use mojito_symbol::symbol::OverloadLookup;
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{ParamContext, ParamExpr};
use mojito_types::types::TyKind;
use mojito_types::types::{
    ConstraintOperand, GenericConstraint, ParamDecl, Ty, TyArg, dict_elements, tuple_elements,
};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

mod ir;
pub use ir::*;
pub mod text;
pub mod verify;

mod calls;
mod facts;
pub mod link;
mod lower_expr;
mod lower_stmt;
mod node;

/// Lower a whole HIR control-flow graph (one function body) into a
/// `MirFunction`.
///
/// Each HIR block becomes a MIR block (same order); a single [`Flatten`]
/// threads the register counter, the variable interner (seeded from `cfg.vars`
/// so IDs agree with the HIR), and the span table across the whole function.
pub fn lower_cfg(cfg: &Cfg) -> MirFunction {
    lower_cfg_nested(
        cfg,
        HashMap::new(),
        &HashMap::new(),
        &std::sync::Arc::default(),
        false,
        &[],
        &[],
        &[],
        None,
        &EnclosingBinders::default(),
        &mut ComptimeThunks::default(),
    )
}

/// A whole program's worth of lowered functions, keyed by name.
///
/// The synthetic `__toplevel__` holds module initialization and explicit
/// legacy test snippets. Production compilation rejects executable file-scope
/// source statements.
#[derive(Debug, Clone)]
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

/// Concrete MIR: an elaborator's entry-rooted output, verified concrete.
///
/// It passed [`verify::verify_concrete`], so it names no symbolic type,
/// compile-time parameter, or erased-dispatch contract. The only constructor
/// runs that verification; a holder never has to know which verifier mode
/// ran.
#[derive(Debug, Clone)]
pub struct ConcreteMir(MirProgram);

impl ConcreteMir {
    /// Admit `program` as concrete, or return the verifier's findings.
    pub fn verified(program: MirProgram) -> Result<Self, Vec<String>> {
        let findings = verify::verify_concrete(&program);
        if findings.is_empty() {
            Ok(Self(program))
        } else {
            Err(findings)
        }
    }

    pub fn into_program(self) -> MirProgram {
        self.0
    }
}

impl std::ops::Deref for ConcreteMir {
    type Target = MirProgram;

    fn deref(&self) -> &MirProgram {
        &self.0
    }
}

/// A program's MIR declarations: its own, and those of the package it
/// imports, if any, which a lookup resolves on first use.
///
/// Look a declaration up by name through [`Self::function`] and
/// [`Self::structure`]: `structs` and `functions` hold the program's own
/// declarations alone, and an own declaration hides a package one of the
/// same name.
#[derive(Debug, Clone, Default)]
pub struct MirDeclarations {
    pub structs: Vec<MirStructDeclaration>,
    pub functions: Vec<MirFunctionDeclaration>,
    /// Every trait the program itself declares, sorted by name; an
    /// imported package's resolve through [`Self::trait_declaration`]. A
    /// compiler-known type conforms to a trait [`Self::declares_trait`]
    /// names to none, and to any other by its shape
    /// (`mojito_types::conformance::leaf_conforms`).
    pub traits: Vec<TraitDeclaration>,
    /// The imported package's declarations, attached by
    /// [`link::link_declarations`].
    pub imported: Option<link::ImportedDeclarations>,
}

impl MirDeclarations {
    /// The function declaration lowered as `name`.
    pub fn function(&self, name: &str) -> Option<&MirFunctionDeclaration> {
        self.functions
            .iter()
            .find(|declaration| declaration.lowered_name == name)
            .or_else(|| self.imported.as_ref()?.function(name))
    }

    /// The struct declaration `name`.
    pub fn structure(&self, name: &str) -> Option<&MirStructDeclaration> {
        self.structs
            .iter()
            .find(|declaration| declaration.name == name)
            .or_else(|| self.imported.as_ref()?.structure(name))
    }

    /// Whether the program or its package declares the trait `name`,
    /// decoding nothing.
    pub fn declares_trait(&self, name: &str) -> bool {
        self.own_trait(name).is_some()
            || self
                .imported
                .as_ref()
                .is_some_and(|imported| imported.package().declares_trait(name))
    }

    /// The trait declaration `name`: the program's own, else its package's,
    /// decoded on first use.
    pub fn trait_declaration(&self, name: &str) -> Option<&TraitDeclaration> {
        self.own_trait(name)
            .or_else(|| self.imported.as_ref()?.package().trait_declaration(name))
    }

    /// Whether the declared trait `candidate` refines `required`, directly
    /// or through its parents.
    pub fn trait_refines(&self, candidate: &str, required: &str) -> bool {
        mojito_types::conformance::refines(
            &|name| {
                self.trait_declaration(name)
                    .map(|declaration| declaration.refines.as_slice())
            },
            candidate,
            required,
        )
    }

    /// The requirement a call through `bounds` selects by its `dispatch`
    /// symbol: one a bound trait declares, or a parent it refines, nearest
    /// first.
    pub fn bound_requirement(
        &self,
        bounds: &[String],
        dispatch: &str,
    ) -> Option<&TraitRequirement> {
        let mut visited = BTreeSet::new();
        let mut pending: VecDeque<&str> = bounds.iter().map(String::as_str).collect();
        while let Some(name) = pending.pop_front() {
            if !visited.insert(name) {
                continue;
            }
            let Some(declaration) = self.trait_declaration(name) else {
                continue;
            };
            if let Some(requirement) = declaration
                .requirements
                .iter()
                .find(|requirement| requirement.dispatch == dispatch)
            {
                return Some(requirement);
            }
            pending.extend(declaration.refines.iter().map(String::as_str));
        }
        None
    }

    /// Every declaration, the imported package's decoded and joined to the
    /// program's own: own structs, then the package's in package order;
    /// own functions, then the package's the program does not lower. For a
    /// consumer of the whole program, such as its disassembly.
    #[must_use]
    pub fn materialized(&self) -> Self {
        let mut declarations = Self {
            structs: self.structs.clone(),
            functions: self.functions.clone(),
            traits: self.traits.clone(),
            imported: None,
        };
        if let Some(imported) = &self.imported {
            imported.materialize_into(&mut declarations);
        }
        declarations
    }

    fn own_trait(&self, name: &str) -> Option<&TraitDeclaration> {
        self.traits
            .binary_search_by(|declaration| declaration.name.as_str().cmp(name))
            .ok()
            .map(|position| &self.traits[position])
    }
}

/// Build the native layout's struct-field index from MIR declaration metadata.
///
/// Lives with MIR (not `native::layout`) so the layout vocabulary stays below
/// the MIR waist while this MIR-consuming constructor sits at it.
pub fn struct_field_index(
    declarations: &MirDeclarations,
) -> mojito_native_core::layout::StructFieldIndex {
    let mut index = mojito_native_core::layout::StructFieldIndex::default();
    for decl in &declarations.structs {
        index.insert(
            decl.name.clone(),
            decl.fields.iter().map(|(_, ty)| ty.clone()).collect(),
        );
    }
    index
}

/// [`struct_field_index`] over the structs `ty` reaches, each looked up in
/// `declarations`.
pub fn struct_field_index_reaching(
    declarations: &MirDeclarations,
    ty: &Ty,
) -> mojito_native_core::layout::StructFieldIndex {
    let mut index = mojito_native_core::layout::StructFieldIndex::default();
    for (name, fields) in reached_struct_fields(ty, |name| {
        declarations.structure(name).map(|declaration| {
            declaration
                .fields
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect()
        })
    }) {
        index.insert(name, fields);
    }
    index
}

/// Each struct `ty` reaches through its own structure and its structs'
/// field types, beside those field types as `fields` gives them. A struct
/// `fields` does not know ends the walk there.
pub fn reached_struct_fields(
    ty: &Ty,
    mut fields: impl FnMut(&str) -> Option<Vec<Ty>>,
) -> Vec<(String, Vec<Ty>)> {
    let mut reached = Vec::new();
    let mut seen = HashSet::new();
    let mut pending = vec![ty.clone()];
    while let Some(ty) = pending.pop() {
        let named = std::cell::RefCell::new(Vec::new());
        mojito_types::types::mentions(&ty, &|nested| {
            if let TyKind::Struct(name, _) = nested.kind() {
                named.borrow_mut().push(name.clone());
            }
            false
        });
        for name in named.into_inner() {
            if seen.insert(name.clone())
                && let Some(types) = fields(&name)
            {
                pending.extend(types.iter().cloned());
                reached.push((name, types));
            }
        }
    }
    reached
}

/// The variable slot holding `declaration`'s owning (`var`) positional
/// collector. Its elements are destroyed last to first, unlike a `Tuple`'s
/// storage.
#[must_use]
pub fn owned_pack_slot(declaration: &MirFunctionDeclaration) -> Option<VarId> {
    if declaration.variadic.is_none() || declaration.variadic_convention != Some(ArgConvention::Var)
    {
        return None;
    }
    let index = declaration
        .variadic_index
        .unwrap_or(declaration.param_names.len());
    VarId::try_from(usize::from(declaration.has_receiver) + index).ok()
}

/// The compile-time parameters in scope of the body `name`: those of each
/// declaration enclosing it, outermost first, then its own.
///
/// A nested body is a generator over its enclosing declarations' binders
/// and over the binders of the regions it is declared in, which come
/// between them and its own.
pub fn binder_scope<'a>(
    name: &str,
    lookup: impl Fn(&str) -> Option<&'a MirFunctionDeclaration>,
) -> Vec<&'a ParamDecl> {
    let mut chain = Vec::new();
    let mut link = lookup(name);
    while let Some(declaration) = link {
        chain.push(&declaration.param_decls);
        chain.push(&declaration.region_binders);
        link = declaration.enclosing.as_deref().and_then(&lookup);
    }
    chain.into_iter().rev().flatten().collect()
}

/// The declared type of the materialized value parameter `name` in `scope`.
///
/// Such a parameter is non-variadic and declared at a type binder (`v: T`),
/// an aggregate (`p: Tuple[Int, String]`), or a vector
/// (`v: SIMD[DType.int32, 2]`).
pub fn materialized_parameter_ty<'a>(
    name: &str,
    scope: impl IntoIterator<Item = &'a ParamDecl>,
) -> Option<&'a Ty> {
    scope.into_iter().find_map(|decl| match decl {
        ParamDecl::Value {
            name: declared,
            ty,
            variadic: false,
            ..
        } if declared == name
            && matches!(
                ty.kind(),
                TyKind::Param { .. } | TyKind::Struct(..) | TyKind::Simd { .. }
            ) =>
        {
            Some(ty)
        }
        _ => None,
    })
}

/// The slots of `function` that hold a value parameter of `scope` rather than
/// storage.
///
/// Such a parameter ([`materialized_parameter_ty`]), like a `comptime for`
/// binder, is materialized at each run-time read, whose consumer owns the
/// result, so its slot owns nothing and is no drop root. A callable-bound
/// parameter is left out: the elaborator may promote it to a run-time
/// parameter.
#[must_use]
pub fn materialized_parameter_slots(
    function: &MirFunction,
    scope: &[&ParamDecl],
) -> HashSet<VarId> {
    function
        .var_names
        .iter()
        .enumerate()
        .skip(function.n_params)
        .filter(|(_, name)| {
            materialized_parameter_ty(name, scope.iter().copied()).is_some_and(|ty| {
                !matches!(
                    ty.kind(),
                    TyKind::Param {
                        callable_bound: Some(_),
                        ..
                    }
                )
            })
        })
        .filter_map(|(var, _)| VarId::try_from(var).ok())
        .collect()
}

/// # Panics
///
/// Panics if a checked declaration names a parameter absent from its own
/// signature, which `CheckedProgram` construction rules out.
pub fn lower_checked_program(checked: &CheckedProgram) -> MirProgram {
    lower_checked_program_importing(checked, None)
}

/// [`lower_checked_program`] for a program that imports a compiled package.
///
/// A declaration the package compiled is not lowered again, and the
/// package's MIR declarations are attached to the program's before its
/// functions are closed and verified. The package's functions stay out of the result; a
/// consumer reaches them through the package.
///
/// # Panics
///
/// As [`lower_checked_program`].
pub fn lower_checked_program_importing(
    checked: &CheckedProgram,
    package: Option<&std::sync::Arc<text::package::PackageDeclarations>>,
) -> MirProgram {
    lower_program(checked, package).0
}

/// [`lower_checked_program`], with the functions each lowering unit
/// ([`CheckedProgram::lowering_units`]) lowered to, as a range of the
/// result's functions, in unit order.
///
/// # Panics
///
/// As [`lower_checked_program`].
pub fn lower_checked_program_units(
    checked: &CheckedProgram,
) -> (MirProgram, Vec<std::ops::Range<usize>>) {
    lower_program(checked, None)
}

/// Lower one lowering `unit` of `checked`, pushing the functions it lowers
/// to, and the declarations of each, onto `functions` and `declarations`.
/// A function another unit lifts first is not lowered again.
pub fn lower_unit(
    unit: LoweringUnit<'_>,
    checked: &CheckedProgram,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    let overloads = checked.overloads();
    match unit {
        LoweringUnit::Function { root, .. } => {
            let _def = timing::span("def");
            lower_function(root, checked, functions, declarations, invariant_errors);
        }
        LoweringUnit::Method {
            index,
            lowered,
            root,
            member,
            ..
        } => {
            let _struct = timing::span("struct");
            let Some(node) = checked.tables().statement(root) else {
                return;
            };
            let CheckedStmtOp::Struct(declaration) = &node.op else {
                return;
            };
            let Some(m) = declaration.methods.get(member) else {
                return;
            };
            lower_struct_member(
                &StructLowering::new(checked, node.module.as_ref(), &declaration.name),
                StructMember {
                    index,
                    name: m.lifecycle_name(),
                    lowered,
                    params: &m.params,
                    has_self: m.has_self,
                    self_convention: m.self_convention,
                    positional_only: m.positional_only,
                    keyword_only: m.keyword_only,
                    statements: &m.body,
                    defaults: m.params.iter().map(|param| param.default).collect(),
                },
                functions,
                declarations,
                invariant_errors,
            );
        }
        LoweringUnit::Wrapper { wrapper, index, .. } => {
            let _struct = timing::span("struct");
            lower_struct_member(
                &StructLowering::new(checked, wrapper.module.as_ref(), &wrapper.declaration),
                StructMember {
                    index: wrapper.method_index,
                    name: &wrapper.method,
                    lowered: wrapper.lowered_name.clone(),
                    params: &wrapper.params,
                    has_self: true,
                    self_convention: wrapper.self_convention,
                    positional_only: None,
                    keyword_only: None,
                    statements: checked.tables().wrapper_roots(index),
                    defaults: Vec::new(),
                },
                functions,
                declarations,
                invariant_errors,
            );
        }
        // A trait's requirements have no body (`...`); each default is
        // the trait's member, lowered as the check checked it.
        LoweringUnit::TraitMember { root, .. } => {
            let _other = timing::span("other");
            lower_function(root, checked, functions, declarations, invariant_errors);
        }
        // A module `comptime` declaration has no runtime form, as
        // upstream's `lit.alias.decl` has none: a body's read of it is the
        // parameter value the check recorded, and an application it makes
        // is lifted at module scope.
        LoweringUnit::Constant { root, .. } => {
            let _toplevel = timing::span("toplevel");
            let mut thunks = ComptimeThunks::for_owner("__toplevel__");
            thunks.enter_scope(&EnclosingBinders::default().with_loops(&[]));
            thunks.request_applications(checked, std::slice::from_ref(&root));
            thunks.lower(checked, overloads, functions, declarations);
        }
    }
}

/// Lower a package's checked `unit` alone, as its importer does when it
/// first reaches one of the unit's functions.
///
/// It gives each function the unit lowers to, its register types closed
/// against the package's own `declarations`, its locations naming source
/// provenance only.
///
/// # Errors
///
/// What the lowering found wrong with the unit.
pub fn lower_package_unit(
    unit: &mojito_checked::unit::UnitProgram,
    declarations: &MirDeclarations,
) -> Result<Vec<(String, MirFunction)>, String> {
    let lowering = unit
        .unit()
        .ok_or_else(|| "the unit's nodes are not of its kind".to_string())?;
    let mut functions = Vec::new();
    let mut invariant_errors = Vec::new();
    lower_unit(
        lowering,
        unit.program(),
        &mut functions,
        &mut MirDeclarations::default(),
        &mut invariant_errors,
    );
    for (name, function) in &mut functions {
        close_register_types(name, function, declarations, &mut invariant_errors);
        let registers = function.n_regs;
        function.spans.0.retain(|reg, _| *reg < registers);
        for (span, _) in function.spans.0.values_mut() {
            span.syntax = None;
        }
    }
    if invariant_errors.is_empty() {
        Ok(functions)
    } else {
        Err(invariant_errors.join("; "))
    }
}

fn lower_program(
    checked: &CheckedProgram,
    package: Option<&std::sync::Arc<text::package::PackageDeclarations>>,
) -> (MirProgram, Vec<std::ops::Range<usize>>) {
    let mut functions = Vec::new();
    let mut declarations = MirDeclarations {
        traits: checked.conformances().traits.clone(),
        ..MirDeclarations::default()
    };
    let mut invariant_errors = Vec::new();
    let overloads = checked.overloads();
    for root in checked.tables().roots() {
        declarations
            .structs
            .extend(struct_declaration(checked, *root, &mut invariant_errors));
    }
    let units = checked.lowering_units();
    let mut ranges = Vec::with_capacity(units.len());
    let mut lowered = |unit, functions: &mut Vec<_>, declarations: &mut _, errors: &mut _| {
        let start = functions.len();
        lower_unit(unit, checked, functions, declarations, errors);
        ranges.push(start..functions.len());
    };
    let constants = units
        .iter()
        .position(|unit| matches!(unit, LoweringUnit::Constant { .. }))
        .unwrap_or(units.len());
    for unit in &units[..constants] {
        lowered(
            unit.clone(),
            &mut functions,
            &mut declarations,
            &mut invariant_errors,
        );
    }
    let toplevel: Vec<CheckedStmtId> = checked
        .tables()
        .roots()
        .iter()
        .copied()
        .filter(|root| {
            checked.tables().statement(*root).is_some_and(|statement| {
                !matches!(
                    statement.op,
                    CheckedStmtOp::Def(_)
                        | CheckedStmtOp::Struct(_)
                        | CheckedStmtOp::Trait(_)
                        | CheckedStmtOp::Comptime { .. }
                )
            })
        })
        .collect();
    let mut toplevel_fn = {
        let _toplevel = timing::span("toplevel");
        let mut thunks = ComptimeThunks::for_owner("__toplevel__");
        let cfg = Cfg::build_checked_fn(checked, &[], &toplevel);
        let function = lower_cfg_nested(
            &cfg,
            HashMap::new(),
            &HashMap::new(),
            overloads,
            false,
            &[],
            &[],
            &[],
            None,
            &EnclosingBinders::default(),
            &mut thunks,
        );
        thunks.request_applications(checked, &cfg.body);
        thunks.lower(checked, overloads, &mut functions, &mut declarations);
        function
    };
    for unit in &units[constants..] {
        lowered(
            unit.clone(),
            &mut functions,
            &mut declarations,
            &mut invariant_errors,
        );
    }
    // The synthetic module initializer returns nothing and never raises.
    toplevel_fn.ret_ty = Some(Ty::none());
    functions.push(("__toplevel__".to_string(), toplevel_fn));
    if let Some(compiled) = package {
        link::link_declarations(&mut declarations, compiled);
    }
    adopt_member_defaults(checked, &mut declarations, &mut invariant_errors);
    {
        let _close = timing::span("close_register_types");
        for (name, function) in &mut functions {
            close_register_types(name, function, &declarations, &mut invariant_errors);
        }
    }
    let mut result = MirProgram {
        functions,
        declarations,
        invariant_errors,
    };
    timing::count(
        "instructions",
        result
            .functions
            .iter()
            .map(|(_, f)| f.blocks.iter().map(|b| b.instrs.len() as u64).sum::<u64>())
            .sum(),
    );
    let _verify = timing::span("verify.pre_drop");
    result.invariant_errors.extend(verify::verify(&result));
    (result, ranges)
}

/// The MIR declaration of the struct the statement node `root` declares, if
/// it declares one.
fn struct_declaration(
    checked: &CheckedProgram,
    root: CheckedStmtId,
    invariant_errors: &mut Vec<String>,
) -> Option<MirStructDeclaration> {
    let s = checked.tables().statement(root)?;
    let CheckedStmtOp::Struct(declaration) = &s.op else {
        return None;
    };
    let mojito_checked::checked::CheckedStruct {
        name,
        type_params,
        fields,
        methods,
        fieldwise_init,
        ..
    } = &**declaration;
    let overloads = checked.overloads();
    let mut_self_methods = methods
        .iter()
        .filter(|m| {
            matches!(
                m.self_convention,
                Some(ArgConvention::Mut | ArgConvention::Ref)
            )
        })
        .map(|m| {
            let method_name = m.lifecycle_name();
            let source = format!("{name}.{method_name}");
            let lowered = mojito_symbol::symbol::lowered_method_name(
                &source,
                type_params,
                m.shape(),
                &**overloads,
                &|name| checked.constants().get(name),
            );
            if lowered == source {
                method_name.to_string()
            } else {
                lowered
            }
        })
        .chain(
            checked
                .default_wrappers()
                .iter()
                .filter(|wrapper| {
                    wrapper.declaration == *name
                        && wrapper.module == s.module
                        && matches!(
                            wrapper.self_convention,
                            Some(ArgConvention::Mut | ArgConvention::Ref)
                        )
                })
                .map(|wrapper| {
                    if wrapper.lowered_name == format!("{name}.{}", wrapper.method) {
                        wrapper.method.clone()
                    } else {
                        wrapper.lowered_name.clone()
                    }
                }),
        )
        .collect();
    Some(MirStructDeclaration {
        name: name.clone(),
        fields: fields
            .iter()
            .enumerate()
            .map(|(field_index, field)| {
                (
                    field.name.clone(),
                    checked_type_or_record(
                        checked,
                        &AnnotationSite::StructField {
                            module: s.module.clone(),
                            declaration: name.clone(),
                            field: field_index,
                        },
                        &format!("field '{}' of struct '{name}'", field.name),
                        invariant_errors,
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
        explicit_destructors: checked
            .struct_destructors()
            .get(name)
            .cloned()
            .unwrap_or_default(),
        explicit_destroy: checked
            .explicit_destroy_types()
            .get(name)
            .map(mojito_checked::declaration::ExplicitDestroyObligation::of),
        conformances: checked
            .conformances()
            .structs
            .get(name)
            .cloned()
            .unwrap_or_default(),
        declared: checked
            .conformances()
            .declared
            .get(name)
            .cloned()
            .unwrap_or_default(),
        associated_types: checked
            .conformances()
            .associated
            .get(name)
            .cloned()
            .unwrap_or_default(),
        parameterized_associated_types: checked
            .conformances()
            .parameterized_associated
            .get(name)
            .cloned()
            .unwrap_or_default(),
        associated_values: checked
            .conformances()
            .associated_values
            .get(name)
            .cloned()
            .unwrap_or_default(),
    })
}

/// A parameter's default metadata: a folded literal, or a `Construct` when the
/// checker recorded an `@implicit` conversion at the default's node (so the
/// omitted-arg slot materializes by running the converting constructor, e.g.
/// a `None` default for `Optional[T]`). Any other default is `None` here;
/// [`lower_default`] lowers it as code.
fn mir_default(checked: &CheckedProgram, default: CheckedNodeId) -> Option<CheckedConst> {
    let base = CheckedConst::from_node(checked.tables(), default)?;
    let conversion = checked
        .expression(default)?
        .adjustments
        .iter()
        .rev()
        .find_map(|adjustment| match adjustment {
            mojito_checked::checked::SemanticAdjustment::ImplicitConversion(target)
            | mojito_checked::checked::SemanticAdjustment::IndexNormalization { target } => {
                Some(target.clone())
            }
            mojito_checked::checked::SemanticAdjustment::SplatScalar(_) => {
                Some(mojito_symbol::symbol::SCALAR_SPLAT_CONVERSION.to_string())
            }
            _ => None,
        });
    match conversion {
        Some(target) => Some(CheckedConst::Construct {
            target,
            arg: Box::new(base),
        }),
        None => Some(base),
    }
}

/// Lower the `def` whose statement node is `root`: its declaration, its
/// parameter defaults, and its body.
fn lower_function(
    root: CheckedStmtId,
    checked: &CheckedProgram,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    let Some(s) = checked.tables().statement(root) else {
        return;
    };
    let CheckedStmtOp::Def(def) = &s.op else {
        return;
    };
    let mojito_checked::checked::CheckedDef {
        name,
        type_params,
        params,
        positional_only,
        keyword_only,
        body: statements,
        ..
    } = &**def;
    let overloads = checked.overloads();
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
    let value_parameter_locals = callable_parameter_locals(&param_decls);
    let enclosing_binders = EnclosingBinders::of(&param_decls);
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
                    &AnnotationSite::FunctionParam {
                        module: s.module.clone(),
                        declaration: s.span,
                        syntax: s.syntax_id,
                        param,
                    },
                    &format!("parameter '{}' of function '{name}'", p.name),
                    invariant_errors,
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
        mojito_symbol::symbol::lowered_def_name(name, type_params, params, &**overloads, &|name| {
            checked.constants().get(name)
        });
    let variadic_idx = params.iter().position(|p| p.kind == ParamKind::Variadic);
    let kw_variadic_idx = params.iter().position(|p| p.kind == ParamKind::KwVariadic);
    let regular: Vec<_> = params
        .iter()
        .filter(|p| {
            p.kind == ParamKind::Regular && !matches!(p.convention, Some(ArgConvention::Out))
        })
        .collect();
    let return_site = AnnotationSite::FunctionReturn {
        module: s.module.clone(),
        declaration: s.span,
        syntax: s.syntax_id,
    };
    let ret_ty = checked_type_or_record(
        checked,
        &return_site,
        &format!("return type of function '{name}'"),
        invariant_errors,
    );
    let effect = checked
        .declaration_effect_at(&return_site)
        .cloned()
        .unwrap_or_else(|| {
            invariant_errors.push(format!("missing checked effect for function '{name}'"));
            mojito_checked::checked::DeclarationEffect {
                raises: false,
                error: None,
                returns_reference: false,
                param_writes: Vec::new(),
                availability: Vec::new(),
            }
        });
    let defaults = regular
        .iter()
        .map(|p| {
            let param = params
                .iter()
                .position(|candidate| std::ptr::eq(candidate, *p))
                .unwrap_or(params.len());
            let ty = checked.checked_type_at(&AnnotationSite::FunctionParam {
                module: s.module.clone(),
                declaration: s.span,
                syntax: s.syntax_id,
                param,
            });
            lower_default(
                DefaultLowering {
                    checked,
                    overloads,
                    owner: &lowered_name,
                    parameter: p,
                    default: p.default,
                    binders: &enclosing_binders,
                    ty,
                },
                functions,
                declarations,
            )
        })
        .collect();
    declarations.functions.push(MirFunctionDeclaration {
        lowered_name: lowered_name.clone(),
        param_names: regular.iter().map(|p| p.name.clone()).collect(),
        param_types: regular
            .iter()
            .map(|p| {
                checked_type_or_record(
                    checked,
                    &AnnotationSite::FunctionParam {
                        module: s.module.clone(),
                        declaration: s.span,
                        syntax: s.syntax_id,
                        param: params
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, *p))
                            .unwrap_or(params.len()),
                    },
                    &format!("parameter '{}' of function '{name}'", p.name),
                    invariant_errors,
                )
            })
            .collect(),
        defaults,
        required: regular.iter().map(|p| p.default.is_none()).collect(),
        variadic: variadic_idx.map(|i| {
            checked_type_or_record(
                checked,
                &AnnotationSite::FunctionParam {
                    module: s.module.clone(),
                    declaration: s.span,
                    syntax: s.syntax_id,
                    param: i,
                },
                &format!("variadic parameter of function '{name}'"),
                invariant_errors,
            )
        }),
        variadic_convention: variadic_idx.and_then(|i| params[i].convention),
        variadic_index: runtime_variadic_index(params, variadic_idx),
        kw_variadic: kw_variadic_idx.map(|i| {
            checked_type_or_record(
                checked,
                &AnnotationSite::FunctionParam {
                    module: s.module.clone(),
                    declaration: s.span,
                    syntax: s.syntax_id,
                    param: i,
                },
                &format!("keyword variadic parameter of function '{name}'"),
                invariant_errors,
            )
        }),
        kw_variadic_convention: kw_variadic_idx.and_then(|i| params[i].convention),
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
        param_writes: effect.param_writes.clone(),
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
    });
    lower_fn_nested(
        FunctionLowering {
            checked,
            name: &lowered_name,
            parameter_names: &names,
            parameter_types: ptys,
            value_parameter_locals,
            receiver_value_parameters: Vec::new(),
            static_receiver: None,
            enclosing_binders,
            owned_parameters: owned,
            deinit_parameters: deinit,
            reference_parameters: refp,
            returns_reference: effect.returns_reference,
            ret_ty,
            raises: effect.raises,
            error_ty: effect.error,
            named_result: named_result.map(|p| p.name.as_str()),
            statements,
            returned: None,
            overloads,
        },
        functions,
        declarations,
    );
}

/// Lower struct `structure`'s member `m`: its declaration, its parameter
/// defaults, and its body.
fn lower_struct_member<P: DeclaredParam>(
    structure: &StructLowering<'_>,
    m: StructMember<'_, P>,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    let StructLowering {
        checked,
        overloads,
        module,
        name,
    } = *structure;
    let module = module.cloned();
    let method_index = m.index;
    let method_name = m.name;
    let source_mangled = format!("{name}.{method_name}");
    let mangled = m.lowered;
    let variadic_idx = m
        .params
        .iter()
        .position(|param| param.kind() == ParamKind::Variadic);
    let kw_variadic_idx = m
        .params
        .iter()
        .position(|param| param.kind() == ParamKind::KwVariadic);
    let regular: Vec<_> = m
        .params
        .iter()
        .filter(|param| {
            param.kind() == ParamKind::Regular
                && !matches!(param.convention(), Some(ArgConvention::Out))
        })
        .collect();
    let return_site = AnnotationSite::MethodReturn {
        module: module.clone(),
        declaration: name.to_string(),
        method: method_index,
    };
    let ret_ty = checked_type_or_record(
        checked,
        &return_site,
        &format!("return type of method '{source_mangled}'"),
        invariant_errors,
    );
    let effect = checked
        .declaration_effect_at(&return_site)
        .cloned()
        .unwrap_or_else(|| {
            invariant_errors.push(format!(
                "missing checked effect for method '{source_mangled}'"
            ));
            mojito_checked::checked::DeclarationEffect {
                raises: false,
                error: None,
                returns_reference: false,
                param_writes: Vec::new(),
                availability: Vec::new(),
            }
        });
    let generic_site = GenericSite::Method {
        module: module.clone(),
        declaration: name.to_string(),
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
                module: module.clone(),
                declaration: name.to_string(),
            })
            .unwrap_or(&[]);
        param_decls = struct_decls.iter().cloned().chain(param_decls).collect();
    }
    let value_parameter_locals = callable_parameter_locals(&param_decls);
    let enclosing_binders = EnclosingBinders::of(
        checked
            .generic_parameters_at(&GenericSite::Struct {
                module: module.clone(),
                declaration: name.to_string(),
            })
            .unwrap_or(&[])
            .iter()
            .chain(checked.generic_parameters_at(&generic_site).unwrap_or(&[])),
    );
    let receiver_value_parameters: Vec<(String, Ty)> = checked
        .generic_parameters_at(&GenericSite::Struct {
            module: module.clone(),
            declaration: name.to_string(),
        })
        .unwrap_or(&[])
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => {
                Some((name.clone(), ty.clone()))
            }
            _ => None,
        })
        .collect();
    let static_receiver = (!m.has_self && !receiver_value_parameters.is_empty()).then(|| {
        let decls = checked
            .generic_parameters_at(&GenericSite::Struct {
                module: module.clone(),
                declaration: name.to_string(),
            })
            .unwrap_or(&[]);
        Ty::new(TyKind::Struct(
            name.to_string(),
            decls.iter().map(ParamDecl::own_argument).collect(),
        ))
    });
    let defaults = regular
        .iter()
        .map(|param| {
            let position = m
                .params
                .iter()
                .position(|candidate| std::ptr::eq(candidate, *param))
                .unwrap_or(m.params.len());
            let ty = checked.checked_type_at(&AnnotationSite::MethodParam {
                module: module.clone(),
                declaration: name.to_string(),
                method: method_index,
                param: position,
            });
            lower_default(
                DefaultLowering {
                    checked,
                    overloads,
                    owner: &mangled,
                    parameter: param,
                    default: m.defaults.get(position).copied().flatten(),
                    binders: &enclosing_binders,
                    ty,
                },
                functions,
                declarations,
            )
        })
        .collect();
    declarations.functions.push(MirFunctionDeclaration {
        lowered_name: mangled.clone(),
        param_names: regular
            .iter()
            .map(|param| param.name().to_string())
            .collect(),
        param_types: regular
            .iter()
            .map(|param| {
                checked_type_or_record(
                    checked,
                    &AnnotationSite::MethodParam {
                        module: module.clone(),
                        declaration: name.to_string(),
                        method: method_index,
                        param: m
                            .params
                            .iter()
                            .position(|candidate| std::ptr::eq(candidate, *param))
                            .unwrap_or(m.params.len()),
                    },
                    &format!("parameter '{}' of method '{source_mangled}'", param.name()),
                    invariant_errors,
                )
            })
            .collect(),
        defaults,
        required: regular.iter().map(|param| !param.has_default()).collect(),
        variadic: variadic_idx.map(|index| {
            checked_type_or_record(
                checked,
                &AnnotationSite::MethodParam {
                    module: module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                    param: index,
                },
                &format!("variadic parameter of method '{source_mangled}'"),
                invariant_errors,
            )
        }),
        variadic_convention: variadic_idx.and_then(|index| m.params[index].convention()),
        variadic_index: runtime_variadic_index(m.params, variadic_idx),
        kw_variadic: kw_variadic_idx.map(|index| {
            checked_type_or_record(
                checked,
                &AnnotationSite::MethodParam {
                    module: module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                    param: index,
                },
                &format!("keyword variadic parameter of method '{source_mangled}'"),
                invariant_errors,
            )
        }),
        kw_variadic_convention: kw_variadic_idx.and_then(|index| m.params[index].convention()),
        kw_variadic_index: runtime_parameter_index(m.params, kw_variadic_idx),
        positional_only: regular_marker_index(m.params, m.positional_only),
        keyword_only: effective_keyword_only_index(m.params, m.keyword_only, variadic_idx),
        param_decls,
        has_receiver: m.has_self,
        receiver_convention: m.self_convention,
        param_conventions: regular.iter().map(DeclaredParam::convention).collect(),
        ret_ty: ret_ty.clone(),
        returns_reference: effect.returns_reference,
        raises: effect.raises,
        error_ty: effect.error.clone(),
        ref_params: regular
            .iter()
            .map(|param| is_ref(&param.convention()))
            .collect(),
        param_writes: effect.param_writes.clone(),
        availability: effect.availability.clone(),
        enclosing: None,
        region_binders: Vec::new(),
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
        ptys.push(TyKind::Struct(name.to_string(), Vec::new()).into());
        owned.push(is_owned(&m.self_convention));
        deinit.push(is_deinit(&m.self_convention));
        refp.push(is_ref(&m.self_convention));
    }
    // The named result follows the ABI parameters as a
    // callee-local uninitialized slot no caller passes.
    let named_result = m
        .params
        .iter()
        .find(|param| DeclaredParam::is_named_result(*param));
    let caller_params = || {
        m.params
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_named_result())
    };
    names.extend(caller_params().map(|(_, p)| p.name().to_string()));
    names.extend(named_result.map(|result| result.name().to_string()));
    names.extend(value_parameter_locals.iter().map(|(name, _)| name.clone()));
    ptys.extend(caller_params().map(|(param, p)| {
        body_parameter_ty(
            p,
            checked_type_or_record(
                checked,
                &AnnotationSite::MethodParam {
                    module: module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                    param,
                },
                &format!("parameter '{}' of method '{source_mangled}'", p.name()),
                invariant_errors,
            ),
        )
    }));
    owned.extend(caller_params().map(|(_, p)| is_owned(&p.convention())));
    deinit.extend(caller_params().map(|(_, p)| is_deinit(&p.convention())));
    refp.extend(caller_params().map(|(_, p)| is_ref(&p.convention())));
    lower_fn_nested(
        FunctionLowering {
            checked,
            name: &mangled,
            parameter_names: &names,
            parameter_types: ptys,
            value_parameter_locals,
            receiver_value_parameters,
            static_receiver,
            enclosing_binders,
            owned_parameters: owned,
            deinit_parameters: deinit,
            reference_parameters: refp,
            returns_reference: effect.returns_reference,
            ret_ty,
            raises: effect.raises,
            error_ty: effect.error,
            named_result: named_result.map(DeclaredParam::name),
            statements: m.statements,
            returned: None,
            overloads,
        },
        functions,
        declarations,
    );
}

/// The struct a member of which MIR lowers.
#[derive(Clone, Copy)]
struct StructLowering<'a> {
    checked: &'a CheckedProgram,
    overloads: &'a std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    module: Option<&'a String>,
    name: &'a str,
}

impl<'a> StructLowering<'a> {
    /// The struct `name` of `module`, as `checked` lowers its members.
    const fn new(checked: &'a CheckedProgram, module: Option<&'a String>, name: &'a str) -> Self {
        Self {
            checked,
            overloads: checked.overloads(),
            module,
            name,
        }
    }
}

/// A struct member as MIR lowers it: a method the struct spells, or the
/// wrapper of a trait default it inherits, whose parameters are a resolved
/// signature's ([`mojito_checked::checked::CheckedDefaultWrapper`]).
struct StructMember<'a, P> {
    /// Its position among the struct's members, which its checked sites
    /// name.
    index: usize,
    /// The name it is registered and counted under.
    name: &'a str,
    /// The symbol it lowers to.
    lowered: String,
    params: &'a [P],
    has_self: bool,
    self_convention: Option<ArgConvention>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// Its body's statement nodes.
    statements: &'a [CheckedStmtId],
    /// Each parameter's checked default node.
    defaults: Vec<Option<CheckedNodeId>>,
}

/// A declaration parameter whose default [`lower_default`] lowers: `default`
/// is its default's checked node, `ty` its checked type, and `binders` the
/// binders in scope at the declaration, which the default function lacks.
struct DefaultLowering<'a, P> {
    checked: &'a CheckedProgram,
    overloads: &'a std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    owner: &'a str,
    parameter: &'a P,
    default: Option<CheckedNodeId>,
    binders: &'a EnclosingBinders,
    ty: Option<&'a Ty>,
}

impl<P> Clone for DefaultLowering<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for DefaultLowering<'_, P> {}

/// A parameter's default metadata ([`mir_default`]), or, for any other
/// default, the zero-parameter function `$default$<owner>$<parameter>`
/// lowered from the expression, which a call leaving the slot out runs. A
/// default naming a binder in scope (`Self.n * 2`, an enclosing `n`) reads
/// it as the function's own: the function declares those binders, and the
/// elaborator instantiates it with its owner instance's arguments.
fn lower_default<P: DeclaredParam>(
    request: DefaultLowering<'_, P>,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) -> Option<CheckedConst> {
    let DefaultLowering {
        checked,
        overloads,
        owner,
        parameter,
        default: node,
        binders,
        ty,
    } = request;
    let node = node?;
    if let Some(constant) = mir_default(checked, node) {
        return Some(constant);
    }
    let ty = ty?;
    let binders = if binders.named_by(checked.tables(), node) {
        binders.clone()
    } else {
        EnclosingBinders::default()
    };
    let function = mojito_symbol::symbol::lifted_default_name(owner, parameter.name());
    lower_expression_thunk(
        ExpressionThunk {
            checked,
            overloads,
            name: &function,
            prologue: &[],
            node,
            binders: &binders,
            ty,
        },
        functions,
        declarations,
    );
    Some(CheckedConst::Evaluate {
        function,
        arguments: Vec::new(),
    })
}

/// A zero-parameter function returning one checked expression over the
/// binders in scope: an evaluated default (`$default$…`), or a `comptime if`
/// condition or a `comptime for` display the elaborator runs
/// (`$comptime$…`). `prologue` holds the local `comptime` display bindings
/// the expression reads, which the function binds before it returns, and
/// `node` is the expression's checked node.
#[derive(Clone, Copy)]
struct ExpressionThunk<'a> {
    checked: &'a CheckedProgram,
    overloads: &'a std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    name: &'a str,
    prologue: &'a [CheckedStmtId],
    node: CheckedNodeId,
    binders: &'a EnclosingBinders,
    ty: &'a Ty,
}

/// Lower `<prologue>; return <node>` as the function `name`, whose
/// compile-time parameters are the binders and whose value binders are frame
/// locals, and declare it. HIR lowers the returned value from its node.
fn lower_expression_thunk(
    thunk: ExpressionThunk<'_>,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) {
    let ExpressionThunk {
        checked,
        overloads,
        name,
        prologue,
        node,
        binders,
        ty,
    } = thunk;
    let parameter_locals = callable_parameter_locals(&binders.declarations);
    let parameter_names: Vec<String> = parameter_locals
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    lower_fn_nested(
        FunctionLowering {
            checked,
            name,
            parameter_names: &parameter_names,
            parameter_types: Vec::new(),
            value_parameter_locals: parameter_locals,
            receiver_value_parameters: Vec::new(),
            static_receiver: None,
            enclosing_binders: binders.clone(),
            owned_parameters: Vec::new(),
            deinit_parameters: Vec::new(),
            reference_parameters: Vec::new(),
            returns_reference: false,
            ret_ty: ty.clone(),
            raises: false,
            error_ty: None,
            named_result: None,
            statements: prologue,
            returned: Some(node),
            overloads,
        },
        functions,
        declarations,
    );
    declarations.functions.push(MirFunctionDeclaration {
        lowered_name: name.to_string(),
        param_names: Vec::new(),
        param_types: Vec::new(),
        defaults: Vec::new(),
        required: Vec::new(),
        variadic: None,
        variadic_convention: None,
        variadic_index: None,
        kw_variadic: None,
        kw_variadic_convention: None,
        kw_variadic_index: None,
        positional_only: None,
        keyword_only: None,
        param_decls: binders.declarations.clone(),
        has_receiver: false,
        receiver_convention: None,
        param_conventions: Vec::new(),
        ret_ty: ty.clone(),
        returns_reference: false,
        raises: false,
        error_ty: None,
        ref_params: Vec::new(),
        param_writes: Vec::new(),
        availability: Vec::new(),
        enclosing: None,
        region_binders: Vec::new(),
    });
}

/// The compile-time expressions one function's lowering lifts as thunks: a
/// `comptime if` condition the checker compiled to no constraint (it applies
/// a function, or reads a `Bool` binding), a `comptime for` display the
/// checker did not close (`[n, n + 1]`), or a range bound or scalar that
/// reads a local display binding (`len(L)`, `L[0]`) becomes the
/// zero-parameter function `$comptime$<owner>$<k>` over the binders in scope
/// at the expression — the owner's, then the enclosing `comptime for`
/// indices — and its branch, loop header, or parameter constant carries the
/// application of that thunk, which the elaborator demands and runs. A local
/// `comptime` binding of such a display has no runtime form: it is lifted
/// once, under the name the check gave its sequence, for the loop headers
/// over its name and the types and parameter arguments that read an element
/// or the length, and a thunk that reads the name begins by binding the
/// display. An `Int` or a `Bool` the check lifted from a type or a parameter
/// argument, or from a local `comptime` value (`h(L[0])`, `n > 2`), is
/// lifted under the name the check gave it too, once the owner's body is
/// lowered ([`Self::request_applications`]).
#[derive(Default)]
struct ComptimeThunks {
    owner: String,
    requests: Vec<ThunkRequest>,
    /// The owner's local `comptime` bindings lowered so far that denote no
    /// parameter expression, the displays among them, in declaration order
    /// ([`Self::bind_evaluated`]): a thunk that reads one begins by binding
    /// it.
    evaluated: Vec<EvaluatedBinding>,
    /// The owner's local `comptime` bindings of a parameter expression
    /// lowered so far ([`Self::bind_value`]), which every thunk requested
    /// after them reads ([`EnclosingBinders::comptime_bindings`]).
    bound_values: Vec<(
        mojito_types::origin::OwnerId,
        mojito_types::param_expr::ParamExpr,
    )>,
    /// The binders in scope at each block lowered so far, one entry per
    /// distinct set ([`Self::enter_scope`]): the owner's, and those under
    /// each nest of `comptime for` loops.
    scopes: Vec<EnclosingBinders>,
}

/// A local `comptime` binding of a display the elaborator evaluates
/// (`comptime L = [n, n + 1]`), which references its value's node as an SSA
/// use names its definition.
#[derive(Clone)]
struct DisplayBinding<'a> {
    owner: mojito_types::origin::OwnerId,
    name: String,
    /// The display's value, built in place where it crosses
    /// (`materialize[L]()`).
    value: Node<'a>,
    /// The application of the display's thunk the loops over the name
    /// iterate ([`ComptimeThunks::bind_display`]).
    sequence: mojito_types::param_expr::ParamExpr,
    ty: Ty,
    /// The binders in scope where the binding is declared.
    binders: EnclosingBinders,
    /// How many evaluated bindings were declared before it.
    preceding: usize,
}

/// One lifted expression: the thunk's name, the node of the expression it
/// returns at `ty`, the binders in scope there, and the evaluated bindings
/// declared before it.
struct ThunkRequest {
    name: String,
    node: CheckedNodeId,
    binders: EnclosingBinders,
    ty: Ty,
    evaluated: Vec<EvaluatedBinding>,
}

/// A local `comptime` binding that denotes no parameter expression: its
/// owner and its statement node, which a thunk that reads the binding lowers
/// in its prologue.
#[derive(Clone)]
struct EvaluatedBinding {
    owner: mojito_types::origin::OwnerId,
    id: CheckedStmtId,
}

impl ComptimeThunks {
    fn for_owner(owner: &str) -> Self {
        Self {
            owner: owner.to_string(),
            ..Self::default()
        }
    }

    /// Register the condition of node `node` and give back the constraint its branch carries:
    /// the thunk applied to every binder in scope, equal to `True`.
    fn request(&mut self, node: CheckedNodeId, binders: &EnclosingBinders) -> GenericConstraint {
        GenericConstraint::Eq(
            ConstraintOperand::Expr(self.request_value(node, binders, Ty::bool())),
            ConstraintOperand::Value(CtValue::Bool(true)),
        )
    }

    /// Register a scalar expression of type `ty`, of node `node`, and give
    /// back the parameter expression that denotes its value: the thunk
    /// applied to every binder in scope.
    fn request_value(
        &mut self,
        node: CheckedNodeId,
        binders: &EnclosingBinders,
        ty: Ty,
    ) -> mojito_types::param_expr::ParamExpr {
        let meta = mojito_types::param_expr::MetaTy::value(ty.clone());
        let visible = self.evaluated.len();
        self.application(node, binders, ty, meta, visible)
    }

    /// Register a `comptime for` display of checked type `ty`, of node
    /// `node`, and give back the sequence its loop header iterates: the
    /// thunk applied to every binder in scope, a list of `element`s.
    fn request_sequence(
        &mut self,
        node: CheckedNodeId,
        binders: &EnclosingBinders,
        ty: Ty,
        element: mojito_types::param_expr::MetaTy,
    ) -> mojito_types::param_expr::ParamExpr {
        let visible = self.evaluated.len();
        self.application(
            node,
            binders,
            ty,
            mojito_types::param_expr::MetaTy::ParamList(Box::new(element)),
            visible,
        )
    }

    /// Record a local `comptime` binding of a display, declared by
    /// statement node `id`, and lift the display as the function the check named in
    /// `sequence`, its application to the binders in scope at the binding:
    /// the loops over the name iterate that sequence, and a type or a
    /// parameter argument that reads an element or the length carries it.
    /// The function's parameters are the binders the application names, in
    /// its argument order.
    fn bind_display(
        &mut self,
        id: CheckedStmtId,
        binding: &DisplayBinding<'_>,
        sequence: &mojito_types::param_expr::ParamExpr,
    ) -> Result<(), String> {
        let mojito_types::param_expr::ParamKind::Apply { function, args, .. } = sequence.kind()
        else {
            return Err(format!(
                "comptime display `{}` denotes `{sequence}`, not an application",
                binding.name
            ));
        };
        let declarations = args
            .iter()
            .map(|arg| {
                arg.as_decl_ref()
                    .and_then(|reference| {
                        binding
                            .binders
                            .declarations
                            .iter()
                            .find(|declaration| declaration.binder().id == reference.id)
                    })
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "comptime display `{}` is applied to `{arg}`, which is no binder \
                             in scope",
                            binding.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.requests.push(ThunkRequest {
            name: function.clone(),
            node: binding.value.id(),
            binders: EnclosingBinders {
                declarations,
                comptime_bindings: self.bound_values.clone(),
                lifted: true,
                ..binding.binders.clone()
            },
            ty: binding.ty.clone(),
            evaluated: self.evaluated[..binding.preceding].to_vec(),
        });
        self.bind_evaluated(binding.owner, id);
        Ok(())
    }

    /// Record that the local `comptime` binding `owner`, declared by
    /// statement node `id`, denotes no parameter expression: a thunk that
    /// reads it evaluates the statement first.
    fn bind_evaluated(&mut self, owner: mojito_types::origin::OwnerId, id: CheckedStmtId) {
        self.evaluated.push(EvaluatedBinding { owner, id });
    }

    /// Record that the local `comptime` binding `owner` denotes `value`
    /// (`comptime k = n + 1`), for the thunks that read it.
    fn bind_value(
        &mut self,
        owner: mojito_types::origin::OwnerId,
        value: mojito_types::param_expr::ParamExpr,
    ) {
        self.bound_values.push((owner, value));
    }

    /// The expression the local `comptime` binding `owner` denotes.
    fn bound_value(
        &self,
        owner: mojito_types::origin::OwnerId,
    ) -> Option<mojito_types::param_expr::ParamExpr> {
        comptime_binding_value(&self.bound_values, owner)
    }

    /// Record the binders in scope at a block being lowered.
    fn enter_scope(&mut self, binders: &EnclosingBinders) {
        let known = |scope: &EnclosingBinders| {
            scope.declarations.len() == binders.declarations.len()
                && scope
                    .declarations
                    .iter()
                    .zip(&binders.declarations)
                    .all(|(known, entered)| known.binder().id == entered.binder().id)
        };
        if !self.scopes.iter().any(known) {
            self.scopes.push(binders.clone());
        }
    }

    /// Request the function of each expression of the owner's `body` the
    /// check lifted by name (`SemanticAdjustment::ComptimeApplication`): a
    /// type or a parameter argument, or a local `comptime` value, that
    /// computes an `Int` or a `Bool` some way the parameter domain does not
    /// express. The function's parameters are the binders the application
    /// names, in its argument order, out of the narrowest scope that
    /// declares them all; it reads every local `comptime` binding of the
    /// body. An application inside a nested `def` or closure is that
    /// declaration's, which reads the bindings of its own body.
    fn request_applications(&mut self, checked: &CheckedProgram, body: &[CheckedStmtId]) {
        let tables = checked.tables();
        let mut found = Vec::new();
        tables.walk(body, &mut |reached| match reached {
            Walked::Statement(statement) => match statement.op {
                CheckedStmtOp::Def(_) | CheckedStmtOp::Struct(_) | CheckedStmtOp::Trait(_) => {
                    WalkResult::SkipBodies
                }
                _ => WalkResult::Advance,
            },
            Walked::Expression(node) => {
                found.extend(lifted_application(&node.adjustments).map(|found| (node.id, found)));
                WalkResult::Advance
            }
        });
        for (node, application) in found {
            let Some(node) = tables.expression(node) else {
                continue;
            };
            let mojito_types::param_expr::ParamKind::Apply { function, args, .. } =
                application.kind()
            else {
                continue;
            };
            // A call the check applied by the callee's own name lifts
            // nothing.
            let lifted = mojito_symbol::symbol::lifted_owner(function).is_some_and(|owner| {
                !matches!(owner, mojito_symbol::symbol::LiftedOwner::Default { .. })
            });
            if !lifted
                || self
                    .requests
                    .iter()
                    .any(|request| request.name == *function)
            {
                continue;
            }
            // A display constant's function returns the display at its own
            // type, whatever list the application denotes.
            let Some(ty) = application.meta().as_value().or(node.ty.as_ref()).cloned() else {
                continue;
            };
            let declared = |scope: &EnclosingBinders| {
                args.iter()
                    .map(|arg| {
                        let reference = arg.as_decl_ref()?;
                        scope
                            .declarations
                            .iter()
                            .find(|declaration| declaration.binder().id == reference.id)
                            .cloned()
                    })
                    .collect::<Option<Vec<_>>>()
            };
            let Some((scope, declarations)) = self
                .scopes
                .iter()
                .filter_map(|scope| Some((scope, declared(scope)?)))
                .min_by_key(|(scope, _)| scope.declarations.len())
            else {
                continue;
            };
            self.requests.push(ThunkRequest {
                name: function.clone(),
                node: node.id,
                binders: EnclosingBinders {
                    declarations,
                    comptime_bindings: self.bound_values.clone(),
                    lifted: true,
                    ..scope.clone()
                },
                ty,
                evaluated: self.evaluated.clone(),
            });
        }
    }

    /// Register the expression of node `node`, returned at `ty`, and give back
    /// its thunk applied to every binder in scope, a value of `meta`. The
    /// first `visible` evaluated bindings are the ones declared before it.
    fn application(
        &mut self,
        node: CheckedNodeId,
        binders: &EnclosingBinders,
        ty: Ty,
        meta: mojito_types::param_expr::MetaTy,
        visible: usize,
    ) -> mojito_types::param_expr::ParamExpr {
        let name = mojito_symbol::symbol::lifted_body_name(&self.owner, self.requests.len());
        self.requests.push(ThunkRequest {
            name: name.clone(),
            node,
            binders: EnclosingBinders {
                comptime_bindings: self.bound_values.clone(),
                lifted: true,
                ..binders.clone()
            },
            ty,
            evaluated: self.evaluated[..visible].to_vec(),
        });
        let context = mojito_types::param_expr::ParamContext::detached();
        let args: Vec<_> = binders
            .declarations
            .iter()
            .map(|declaration| {
                // A pack binder is referenced by its bare spelling, as every
                // other read of it is.
                let binder = declaration.binder();
                context.decl_ref(
                    binder.id.clone(),
                    binder.name.trim_start_matches('*'),
                    verify::declared_kind(declaration),
                )
            })
            .collect();
        context.apply(&name, &args, meta)
    }

    /// Lower every requested thunk, each `return <expression>` at its type
    /// over the binders its request recorded, after the evaluated bindings
    /// the expression reads.
    fn lower(
        self,
        checked: &CheckedProgram,
        overloads: &std::sync::Arc<mojito_checked::package::ProgramOverloads>,
        functions: &mut Vec<(String, MirFunction)>,
        declarations: &mut MirDeclarations,
    ) {
        for request in &self.requests {
            // A function the check named is lifted by the first owner that
            // spells its expression.
            if declarations
                .functions
                .iter()
                .any(|declared| declared.lowered_name == request.name)
            {
                continue;
            }
            let prologue = thunk_prologue(checked, request.node, &request.evaluated);
            lower_expression_thunk(
                ExpressionThunk {
                    checked,
                    overloads,
                    name: &request.name,
                    prologue: &prologue,
                    node: request.node,
                    binders: &request.binders,
                    ty: &request.ty,
                },
                functions,
                declarations,
            );
        }
    }
}

/// The statement nodes of the `evaluated` bindings a thunk returning the
/// expression of node `node` reads, in declaration order: the ones the
/// expression names, and the ones their values name in turn.
fn thunk_prologue(
    checked: &CheckedProgram,
    node: CheckedNodeId,
    evaluated: &[EvaluatedBinding],
) -> Vec<CheckedStmtId> {
    let mut read = bindings_read(checked.tables(), node);
    let mut statements = Vec::new();
    for binding in evaluated.iter().rev() {
        if read.contains(&binding.owner) {
            if let Some(CheckedStmtOp::Comptime { value, .. }) =
                checked.tables().statement(binding.id).map(|node| &node.op)
            {
                read.extend(bindings_read(checked.tables(), *value));
            }
            statements.push(binding.id);
        }
    }
    statements.reverse();
    statements
}

/// The call the check selected for an expression, among the expression's
/// `adjustments`.
fn selected_call(
    adjustments: &[mojito_checked::checked::SemanticAdjustment],
) -> Option<mojito_checked::checked::CheckedCallContract> {
    adjustments.iter().find_map(|adjustment| match adjustment {
        mojito_checked::checked::SemanticAdjustment::SelectedCall(contract) => {
            Some((**contract).clone())
        }
        _ => None,
    })
}

/// The function an expression names, among its `adjustments`: its selected
/// call's target, else the callable the check resolved it to.
fn callable_target(adjustments: &[mojito_checked::checked::SemanticAdjustment]) -> Option<String> {
    selected_call(adjustments)
        .map(|contract| contract.target)
        .or_else(|| {
            adjustments.iter().find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ResolveCallable(target) => {
                    Some(target.clone())
                }
                _ => None,
            })
        })
}

/// The parameter value the check recorded for an expression read as a
/// runtime value, among the expression's `adjustments`.
fn recorded_param_value(
    adjustments: &[mojito_checked::checked::SemanticAdjustment],
) -> Option<ParamExpr> {
    adjustments.iter().find_map(|adjustment| match adjustment {
        mojito_checked::checked::SemanticAdjustment::ParamValue { value, .. } => {
            Some(value.clone())
        }
        _ => None,
    })
}

/// Whether an expression reads a parameter value the check recorded with no
/// materialized borrow source of its own (a module constant), among the
/// expression's `adjustments`.
fn constant_param_value(adjustments: &[mojito_checked::checked::SemanticAdjustment]) -> bool {
    adjustments.iter().any(|adjustment| {
        matches!(
            adjustment,
            mojito_checked::checked::SemanticAdjustment::ParamValue {
                materialized: None,
                ..
            }
        )
    })
}

/// Whether checked type `ty` is a pointer whose provenance designates a
/// single checked place.
fn origin_bearing_pointer(ty: Option<&Ty>) -> bool {
    matches!(
        ty.map(Ty::kind),
        Some(TyKind::Pointer { origin, .. })
            if origin.as_origin().is_some() && !origin.multi_element()
    )
}

/// The application the check lifted an expression as, among the
/// expression's `adjustments`.
fn lifted_application(
    adjustments: &[mojito_checked::checked::SemanticAdjustment],
) -> Option<ParamExpr> {
    adjustments.iter().find_map(|adjustment| match adjustment {
        mojito_checked::checked::SemanticAdjustment::ComptimeApplication {
            application, ..
        } => Some(application.clone()),
        _ => None,
    })
}

/// The checked bindings the name nodes at and under node `node` read.
fn bindings_read(
    tables: &mojito_checked::checked::CheckedTables,
    node: CheckedNodeId,
) -> HashSet<mojito_types::origin::OwnerId> {
    let mut owners = HashSet::new();
    tables.walk_expression(node, &mut |reached| {
        if let Walked::Expression(node) = reached
            && matches!(node.op, CheckedOp::Identifier(_))
        {
            owners.extend(node.binding);
        }
        WalkResult::Advance
    });
    owners
}

fn checked_type_or_record(
    checked: &CheckedProgram,
    site: &AnnotationSite,
    description: &str,
    invariant_errors: &mut Vec<String>,
) -> Ty {
    if let Some(ty) = checked.checked_type_at(site) {
        ty.clone()
    } else {
        invariant_errors.push(format!("missing checked type for {description}"));
        Ty::none()
    }
}

/// A callable value parameter has storage in the callee frame although it is
/// not part of the ordinary argument ABI: the slot the body calls through. A
/// type parameter, as at Mojo, has none.
fn callable_parameter_locals(decls: &[ParamDecl]) -> Vec<(String, Ty)> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name, ty, variadic, ..
            } if matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => Some((
                name.trim_start_matches('*').to_string(),
                if *variadic {
                    TyKind::VariadicPack(ty.clone()).into()
                } else {
                    ty.clone()
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
    requested: Option<mojito_types::origin::Mutability>,
) -> Option<Ty> {
    let storage = place.ty.clone()?;
    let root = place.root_ty.as_ref()?;
    let source_mutability = match root.kind() {
        TyKind::Ref(reference) => Some(reference.mutability),
        TyKind::Pointer { origin, .. } => match origin {
            mojito_types::origin::PointerOrigin::Place { mutable, .. } => Some(if *mutable {
                mojito_types::origin::Mutability::Mutable
            } else {
                mojito_types::origin::Mutability::Immutable
            }),
            mojito_types::origin::PointerOrigin::Param { mutability, .. }
            | mojito_types::origin::PointerOrigin::SelfPlace { mutability, .. } => {
                Some(*mutability)
            }
            mojito_types::origin::PointerOrigin::Static
            | mojito_types::origin::PointerOrigin::Untracked { .. }
            | mojito_types::origin::PointerOrigin::UnsafeAny { .. } => None,
        },
        _ => None,
    };
    if source_mutability.is_some() && place.is_whole_root() {
        return Some(root.clone());
    }
    let mutability = source_mutability
        .or(requested)
        .unwrap_or(mojito_types::origin::Mutability::Mutable);
    Some(
        TyKind::Ref(mojito_types::origin::RefTy {
            referent: storage,
            origin: mojito_types::origin::Origin::Untracked {
                mutable: mutability != mojito_types::origin::Mutability::Immutable,
            },
            mutability,
        })
        .into(),
    )
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
    owner_vars: HashMap<mojito_types::origin::OwnerId, VarId>,
    /// Nested `def`s in scope (name → lifted target + captures); a call to one is
    /// rewritten to the mangled function with its captures prepended, and the
    /// nested `def` statement itself lowers to nothing.
    nested: HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    /// The program's overload sets, which name a function value, an
    /// overloaded call's lowered symbol, and a fieldwise constructor's
    /// parameters; shared by every function lowered from one program.
    overloads: std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    /// The program's checked tables, shared with HIR construction.
    checked: &'a std::sync::Arc<mojito_checked::checked::CheckedTables>,
    /// The local `comptime` display bindings in the scopes enclosing the
    /// block being lowered, in declaration order: a `try` region sees its
    /// enclosing scopes' and its own, and its own go out of scope with it,
    /// as the check scopes them.
    displays: Vec<DisplayBinding<'a>>,
    /// The slot each local-name node of the CFG being lowered reads
    /// ([`Cfg::slots`]), which spells that name in this frame.
    slots: HashMap<mojito_checked::checked::CheckedNodeId, VarId>,
    /// Local reference slot to its frozen owner place and permission.
    aliases: HashMap<VarId, MirLoan>,
    runtime_aliases: std::collections::HashSet<VarId>,
    /// Persistent owner loans carried by reference-bearing aggregate variables.
    /// The runtime value contains the handles; this map transfers their static
    /// loans when an aggregate is moved or forwarded into a new binding.
    aggregate_loans: HashMap<VarId, Vec<MirLoan>>,
    /// The loans a view-constructor conversion's `$conv_view_r` slot holds
    /// on a source that is not a named place (an accessor's reference result,
    /// an intrinsic subscript element), keyed by the converted expression's
    /// HIR address: a binding of the converted value inherits them. An entry
    /// lives as long as the lowering of one HIR expression or statement.
    conversion_view_loans: HashMap<usize, Vec<MirLoan>>,
    /// Hidden `$arg_loan_r` slots created for loan-carrying temporary call
    /// arguments while lowering the current statement. Flushed as `KeepAlive`
    /// uses at the statement's end so the anchored loans live through the
    /// consuming call — the temporary's upstream lifetime is the full
    /// statement.
    pending_argument_anchors: Vec<VarId>,
    /// The open calls' setup frames, innermost last. A frame holds the
    /// deferred reads of exactly one call's immediate by-value place
    /// arguments ([`calls::CallSetup`]).
    call_setups: Vec<calls::CallSetup>,
    /// The by-value operand being lowered whose reference result is read at
    /// its call's setup ([`calls::SetupReference`]).
    setup_reference: Option<calls::SetupReference>,
    /// How many call operands enclose the expression being lowered: a frame
    /// belongs to the operands lowered at the depth it was opened at.
    argument_depth: usize,
    /// True only while lowering the argument list of a call that carries no
    /// other channel for a temporary argument's loans — a plain, method, or
    /// callable-value call without checker-selected `ref`-argument borrows or
    /// recorded transfer effects. Constructions aggregate their arguments'
    /// loans into the result, and the excluded callees install them at their
    /// borrow or transfer destinations; anchoring there adds a conflicting
    /// duplicate borrow.
    allow_argument_anchors: bool,
    /// Accumulated transferred loans per interior destination domain
    /// (`(root, path)`), so repeated transfers into one domain merge while
    /// sibling domains keep independent generations.
    transfer_domain_loans: HashMap<(VarId, Vec<mojito_types::origin::OriginSeg>), Vec<MirLoan>>,
    /// The enclosing struct's non-callable value parameters when lowering a
    /// method: `Self.n` in a bracket slot reads the receiver's reified
    /// parameter (`self.n`) instead of reifying a binder spelling.
    receiver_value_parameters: Vec<(String, Ty)>,
    /// The enclosing struct at its own binders when lowering a static method
    /// of a value-parameterized struct: the type of the receiver-less `self`
    /// slot a `Self.n` read projects from, as the expression `Self.n` does.
    static_receiver: Option<Ty>,
    /// The enclosing struct's binders, then the function's own: a bracket
    /// argument forwarding a type binder (`hash[Self.H](key)`) records the
    /// binder it names, and one built from value binders (`f[1 + n]()`) the
    /// expression over them, the innermost declaration winning a shared
    /// spelling.
    enclosing_binders: EnclosingBinders,
    /// The `comptime if` conditions this function lifts as thunks.
    comptime_thunks: ComptimeThunks,
    /// The enclosing `comptime for` variables this function declares as
    /// compile-time parameters (a thunk lifted from a condition inside the
    /// loops), read as references to the index binders they denote.
    parameter_reads: HashMap<mojito_types::origin::OwnerId, mojito_types::param_expr::ParamRef>,
    /// Names rebound more than once, or captured by a nested `def`. A pointer
    /// variable outside this set keeps one statically known loan place for its
    /// whole live range, so deref sites may substitute the owner place.
    reassigned_names: std::collections::HashSet<String>,
    returns_reference: bool,
}

/// A `comptime for` index in scope: the binder the checker recorded on the
/// loop's iterable, and the loop variable's checked binding.
#[derive(Clone)]
struct LoopIndex {
    declaration: ParamDecl,
    binding: Option<mojito_types::origin::OwnerId>,
}

impl LoopIndex {
    /// The index of a loop the checker recorded `iteration` on, its
    /// variable checked as `binding`: a value binder of the element's type.
    fn of(
        iteration: &mojito_checked::checked::ComptimeIteration,
        binding: Option<mojito_types::origin::OwnerId>,
    ) -> Self {
        let binder = &iteration.binder;
        let ty = iteration
            .source
            .binder_meta()
            .as_value()
            .cloned()
            .unwrap_or_else(Ty::int);
        Self {
            declaration: ParamDecl::Value {
                id: binder.id.clone(),
                name: binder.name.to_string(),
                ty,
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            },
            binding,
        }
    }
}

/// The `comptime for` indices in scope at each block of a CFG, outermost
/// first: a loop's body blocks — those its body reaches without passing its
/// header or exit — see the loop's `Int` binder as a value binder of the
/// body. The binder is the one the checker recorded on the iterable.
fn loop_index_scopes(cfg: &Cfg) -> HashMap<hir::BlockId, Vec<LoopIndex>> {
    fn targets(term: Option<&Terminator>) -> Vec<hir::BlockId> {
        match term {
            Some(Terminator::Jump(to)) => vec![*to],
            Some(
                Terminator::Branch { then_b, else_b, .. }
                | Terminator::BranchOnSlot { then_b, else_b, .. }
                | Terminator::ComptimeBranch { then_b, else_b, .. },
            ) => vec![*then_b, *else_b],
            Some(Terminator::ComptimeLoop { body, exit, .. }) => vec![*body, *exit],
            Some(
                Terminator::Return(_)
                | Terminator::ReturnWithCleanup { .. }
                | Terminator::FallOff
                | Terminator::EscapeJump(_),
            )
            | None => Vec::new(),
        }
    }
    let g = &cfg.g;
    let mut scopes: HashMap<hir::BlockId, Vec<LoopIndex>> = HashMap::new();
    let mut loops: Vec<(Vec<hir::BlockId>, LoopIndex)> = Vec::new();
    for header in g.node_indices() {
        let Some(Terminator::ComptimeLoop {
            iter,
            binding,
            body,
            exit,
            ..
        }) = &g[header].term
        else {
            continue;
        };
        let Some(iteration) = comptime_iteration(&cfg.checked, iter) else {
            continue;
        };
        let mut members = Vec::new();
        let mut pending = vec![*body];
        while let Some(block) = pending.pop() {
            if block == header || block == *exit || members.contains(&block) {
                continue;
            }
            members.push(block);
            pending.extend(targets(g[block].term.as_ref()));
        }
        loops.push((members, LoopIndex::of(&iteration, *binding)));
    }
    // An enclosing loop's body holds the nested loop's, so the larger body
    // is the outer scope.
    loops.sort_by_key(|(members, _)| std::cmp::Reverse(members.len()));
    for (members, index) in loops {
        for block in members {
            scopes.entry(block).or_default().push(index.clone());
        }
    }
    scopes
}

/// The expression the local `comptime` binding `owner` denotes among
/// `bindings`.
fn comptime_binding_value(
    bindings: &[(mojito_types::origin::OwnerId, ParamExpr)],
    owner: mojito_types::origin::OwnerId,
) -> Option<ParamExpr> {
    bindings
        .iter()
        .find(|(binding, _)| *binding == owner)
        .map(|(_, value)| value.clone())
}

/// The sequence a `comptime for` iterable was checked as, when the checker
/// recorded one (`SemanticAdjustment::ComptimeIteration`).
fn comptime_iteration(
    checked: &mojito_checked::checked::CheckedTables,
    iter: &hir::HirExpr,
) -> Option<mojito_checked::checked::ComptimeIteration> {
    checked
        .expression(iter.node)?
        .adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            mojito_checked::checked::SemanticAdjustment::ComptimeIteration(iteration) => {
                Some((**iteration).clone())
            }
            _ => None,
        })
}

/// Names whose binding may change after the first assignment (or that a nested
/// `def` captures). CFG-lowered rebindings appear as `HirInstr::Bind`; opaque
/// statements — notably `try` regions, whose sub-CFGs lower separately — are
/// scanned recursively.
fn reassigned_names(
    cfg: &Cfg,
    nested: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
) -> std::collections::HashSet<String> {
    fn bump(counts: &mut HashMap<String, usize>, name: &str) {
        *counts.entry(name.to_string()).or_default() += 1;
    }
    /// Count the names statement `id` binds; `declared` is HIR's runtime name
    /// for the `ref` it declares, which only a top-level statement has.
    fn scan(
        checked: &mojito_checked::checked::CheckedTables,
        id: CheckedStmtId,
        declared: Option<&str>,
        counts: &mut HashMap<String, usize>,
    ) {
        let identifier = |node| match checked.expression(node).map(|node| &node.op) {
            Some(CheckedOp::Identifier(name)) => Some(name.as_str()),
            _ => None,
        };
        let scan_all = |body: &[CheckedStmtId], counts: &mut HashMap<String, usize>| {
            for inner in body {
                scan(checked, *inner, None, counts);
            }
        };
        let Some(node) = checked.statement(id) else {
            return;
        };
        match &node.op {
            CheckedStmtOp::Assign { name, .. } if name == "_" => {}
            CheckedStmtOp::VarDecl { name, .. }
            | CheckedStmtOp::Assign { name, .. }
            | CheckedStmtOp::Comptime { name, .. } => bump(counts, name),
            CheckedStmtOp::RefDecl { name, .. } => bump(counts, declared.unwrap_or(name)),
            CheckedStmtOp::AugAssign { place, .. } | CheckedStmtOp::SetPlace { place, .. } => {
                if let Some(name) = identifier(*place) {
                    bump(counts, name);
                }
            }
            CheckedStmtOp::Unpack { targets, .. } => {
                for name in targets.iter().filter_map(|target| identifier(*target)) {
                    bump(counts, name);
                }
            }
            CheckedStmtOp::If { branches, orelse } => {
                for (_, body) in branches {
                    scan_all(body, counts);
                }
                scan_all(orelse.as_deref().unwrap_or_default(), counts);
            }
            CheckedStmtOp::While { body, orelse, .. } => {
                scan_all(body, counts);
                scan_all(orelse.as_deref().unwrap_or_default(), counts);
            }
            CheckedStmtOp::Scope(body) => scan_all(body, counts),
            CheckedStmtOp::For {
                var, body, orelse, ..
            } => {
                bump(counts, var);
                scan_all(body, counts);
                scan_all(orelse.as_deref().unwrap_or_default(), counts);
            }
            CheckedStmtOp::Try { .. } => {
                for body in TryRegions::of(node).iter().flat_map(TryRegions::bodies) {
                    scan_all(body, counts);
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
                HirInstr::Stmt(statement) => scan(
                    &cfg.checked,
                    statement.node,
                    statement
                        .declared
                        .and_then(|slot| cfg.vars.get(slot as usize))
                        .map(String::as_str),
                    &mut counts,
                ),
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

impl<'a> Flatten<'a> {
    /// Whether a variable's slot stores a pointer *as* its value. Both
    /// backends read such a slot as the pointer it holds (`UseVar` keys on the
    /// declared type), so an assignment to the variable replaces the slot
    /// rather than writing through the handle it stores — including for a
    /// `mut p: Pointer[...]` parameter, whose slot is bound with the caller's
    /// pointer rather than aliasing the caller's variable.
    fn pointer_valued_slot(&self, var: VarId) -> bool {
        matches!(
            self.var_types.get(&var).map(Ty::kind),
            Some(TyKind::Pointer { .. })
        )
    }

    /// The statically known storage place behind a stably bound origin-bearing
    /// pointer variable. Substituting it at deref sites touches the owner at
    /// each access — the liveness contract `ref` aliases have — so ASAP
    /// destruction and loan conflicts stay exact. A reassigned or captured
    /// pointer, or a handle loaded from a field, reads through its runtime
    /// handle instead.
    fn pointer_deref_place(&mut self, object: Node<'a>) -> Option<MirPlace> {
        let name = self.name_spelled(object)?;
        if !object.is_origin_bearing_pointer() || self.reassigned_names.contains(&name) {
            return None;
        }
        // A subtree origin deliberately forgets which exact descendant the
        // pointer designates, so its loan place cannot substitute for the
        // deref target; the runtime handle carries the true projection.
        if matches!(
            object.ty().as_ref().map(Ty::kind),
            Some(TyKind::Pointer { origin, .. }) if origin.subtree()
        ) {
            return None;
        }
        let var = self.expression_var(&name, object);
        let loans = self.aggregate_loans.get(&var)?;
        let [loan] = loans.as_slice() else {
            return None;
        };
        let mut place = loan.place.clone();
        place.through = Some(var);
        Some(place)
    }

    /// The read behind a call-produced origin-bearing pointer (`idp(p)[]`):
    /// the temporary is bound to a hidden `$ptr_tmp_r` slot carrying the loans
    /// its checked origin names — exactly as `var q = idp(p)` would — so the
    /// owner outlives the read. A single-place origin substitutes the owner
    /// place; a subtree origin, which forgets the exact descendant, reads
    /// through the slot's runtime handle.
    fn temporary_pointer_deref(&mut self, e: Node<'a>, object: Node<'a>) -> Option<Reg> {
        if !matches!(
            object.op(),
            CheckedOp::Call { .. } | CheckedOp::MethodCall { .. }
        ) || !object.is_origin_bearing_pointer()
        {
            return None;
        }
        let pointer_ty = object
            .ty()
            .filter(|ty| matches!(ty.kind(), TyKind::Pointer { .. }))?;
        let subtree =
            matches!(pointer_ty.kind(), TyKind::Pointer { origin, .. } if origin.subtree());
        let loans = self.aggregate_borrows(object);
        let owner = match loans.as_slice() {
            [] => return None,
            [loan] if !subtree => Some(loan.place.clone()),
            _ if subtree => None,
            _ => return None,
        };
        let value = self.expr(object);
        let variable = self.var(&format!("$ptr_tmp_r{}", value.0));
        self.var_types.insert(variable, pointer_ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(pointer_ty.clone()),
        });
        let marker = self.fresh_typed(
            object.location(),
            loans.first().map(|loan| loan.place.root),
            Ty::none(),
        );
        self.emit(MirInstr::EstablishLoans {
            reference: variable,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(variable, loans);
        if let Some(mut place) = owner {
            place.through = Some(variable);
            let dest = self.fresh(e.location(), Some(place.root));
            self.emit(MirInstr::LoadPlace { dest, place });
            return Some(dest);
        }
        let reference = self.fresh_typed(object.location(), Some(variable), pointer_ty);
        self.emit(MirInstr::UseVar {
            dest: reference,
            var: variable,
            mode: UseMode::Copy,
        });
        let dest = self.fresh(e.location(), None);
        self.emit(MirInstr::ReadRef { dest, reference });
        Some(dest)
    }

    /// A `deinit self` call whose receiver the checker satisfied by implicit
    /// copy: an `ImplicitlyCopyable` nominal receiver runs its copy lifecycle
    /// (`__init__(copy:)`) into a fresh temporary the callee consumes, so the
    /// original place keeps its own storage and its ordinary drop. Scalars
    /// and other non-nominal values copy by plain read.
    fn copy_consuming_receiver(
        &mut self,
        receiver: Node<'a>,
        recv: Reg,
        recv_place: Option<&MirPlace>,
    ) -> Reg {
        // The copy has its source's exact type — the receiver place's when
        // the register is a place read (the verifier types that register
        // from the place): the checker's expression type can spell the same
        // nominal type with a different argument representation (a public
        // tuple binding keeps a literal element the expression defaulted),
        // which the verifier's equality check would reject.
        let ty = self
            .f
            .reg_types
            .get(&recv.0)
            .cloned()
            .or_else(|| recv_place.and_then(|place| place.ty.clone()))
            .or_else(|| receiver.ty());
        match ty {
            Some(ty) if matches!(ty.kind(), TyKind::Struct(..)) => {
                let copied = self.fresh_typed(receiver.location(), None, ty);
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: recv,
                });
                copied
            }
            _ => recv,
        }
    }

    /// The contract of the parameterized accessor call a type-keyed subscript
    /// (`v[Int]` on the self-hosted `Variant`) stands for: the checker
    /// selected the `__getitem_param__` clone at the subscript itself, so it
    /// lowers as that clone's method call on the subscript base with no
    /// runtime arguments. `None` for every other subscript.
    fn type_keyed_accessor_call(
        &self,
        expression: Node<'a>,
    ) -> Option<mojito_checked::checked::CheckedCallContract> {
        let CheckedOp::Index { index, .. } = expression.op() else {
            return None;
        };
        // The index names a type, never a runtime value (the Int-keyed
        // dependent accessors of a variadic struct index by a checked Int).
        let type_index = self.checked.expression(*index).is_some_and(|index| {
            matches!(index.op, CheckedOp::TypeValue { .. })
                || matches!(index.op, CheckedOp::Identifier(_)) && index.ty.is_none()
        });
        let contract = expression.call_contract()?;
        (type_index
            && contract.arguments.is_empty()
            && contract.target.contains(".__getitem_param__"))
        .then_some(contract)
    }

    /// Peel container slots that themselves store reference handles until one
    /// read through the returned handle yields `target`. This distinguishes
    /// writing a `List[ref T]` element's referent from replacing the stored
    /// `ref T` handle.
    fn peel_reference_handle_to(&mut self, mut handle: Reg, target: &Ty, site: &SourceSpan) -> Reg {
        while let Some(TyKind::Ref(outer)) = (self.f.reg_types.get(&handle.0).cloned())
            .as_ref()
            .map(Ty::kind)
        {
            if outer.referent == *target {
                break;
            }
            let TyKind::Ref(inner) = outer.referent.kind() else {
                break;
            };
            let dest = self.fresh_typed(site.clone(), None, TyKind::Ref(inner.clone()).into());
            self.emit(MirInstr::ReadRef {
                dest,
                reference: handle,
            });
            handle = dest;
        }
        handle
    }

    fn reference_handle(&mut self, expression: Node<'a>) -> Reg {
        // A materialized borrow-source temporary: store it in its hidden owned
        // slot (registered under the checker-minted owner) and hand back a
        // fresh handle to that slot — the auto-borrow a `ref` binding of the
        // temporary would produce. A module constant read as a receiver is
        // materialized the same way, under the constant's owner.
        let constant = expression.constant_param_value();
        let owner = mojito_checked::checked::materialized_borrow_owner(expression.adjustments())
            .or_else(|| constant.then(|| expression.owner()).flatten());
        if let Some(owner) = owner {
            let variable = if let Some(variable) = self.owner_vars.get(&owner).copied() {
                variable
            } else {
                let value = match expression.recorded_value().filter(|_| constant) {
                    Some(value) => self.param_value_register(expression, value),
                    None => self.expr_unconverted(expression),
                };
                let ty = self
                    .f
                    .reg_types
                    .get(&value.0)
                    .cloned()
                    .or_else(|| expression.ty());
                let variable = self.var(&format!("$mat_r{}", value.0));
                if let Some(ty) = ty.clone() {
                    self.var_types.insert(variable, ty);
                }
                self.emit(MirInstr::DefVar {
                    var: variable,
                    src: value,
                    binding_ty: ty,
                });
                self.owner_vars.insert(owner, variable);
                variable
            };
            let place = MirPlace::root(variable, self.var_types.get(&variable).cloned());
            let dest = self.fresh(expression.location(), Some(variable));
            self.emit(MirInstr::MakeRef {
                dest,
                place: place.clone(),
            });
            if let Some(ty) = mir_place_handle_ty(&place, None) {
                self.f.reg_types.insert(dest.0, ty);
            }
            return dest;
        }
        if let Some(reference) = expression.reference_result() {
            if let Some(place) = self.lower_projected_reference_place(expression) {
                let dest = self.fresh_typed(
                    expression.location(),
                    Some(place.root),
                    TyKind::Ref(reference).into(),
                );
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
            let mut result = self.expr_unconverted(expression);
            self.f
                .reg_types
                .insert(result.0, TyKind::Ref(reference).into());
            // A reference-returning subscript over reference-valued storage has
            // an outer handle to the container slot and an inner handle stored
            // in that slot. Peel only the outer layers that expression typing
            // read through. The returned register remains a handle whose single
            // read produces the checker's ordinary expression type.
            if let Some(checked) = expression.ty() {
                result = self.peel_reference_handle_to(result, &checked, &expression.location());
            }
            return result;
        }
        if let Some(name) = self.name_spelled(expression) {
            let var = self.expression_var(&name, expression);
            if let Some(loan) = self.aliases.get(&var).cloned() {
                let handle_ty = self
                    .var_types
                    .get(&var)
                    .filter(|ty| matches!(ty.kind(), TyKind::Ref(_)))
                    .cloned()
                    .or_else(|| {
                        mir_place_handle_ty(
                            &loan.place,
                            Some(if loan.mutable {
                                mojito_types::origin::Mutability::Mutable
                            } else {
                                mojito_types::origin::Mutability::Immutable
                            }),
                        )
                    })
                    .unwrap_or_else(Ty::error);
                let dest =
                    self.fresh_typed(expression.location(), Some(loan.place.root), handle_ty);
                self.emit(MirInstr::MakeRef {
                    dest,
                    place: loan.place,
                });
                return dest;
            }
            if self.runtime_aliases.contains(&var) {
                let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                place.through = Some(var);
                let handle_ty = mir_place_handle_ty(&place, None).unwrap_or_else(Ty::error);
                let dest = self.fresh_typed(expression.location(), Some(var), handle_ty);
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
            // An owned variable auto-borrows where storage expects a handle
            // (a `ref` ctor field fed a bare place); a variable that itself
            // stores a handle instead falls through to the plain load below.
            let var_ty = self.var_types.get(&var).cloned();
            if !matches!(var_ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_))) {
                let place = MirPlace::root(var, var_ty);
                let handle_ty = mir_place_handle_ty(
                    &place,
                    expression.borrow_mutability().map(|mutable| {
                        if mutable {
                            mojito_types::origin::Mutability::Mutable
                        } else {
                            mojito_types::origin::Mutability::Immutable
                        }
                    }),
                )
                .unwrap_or_else(Ty::error);
                let dest = self.fresh_typed(expression.location(), Some(var), handle_ty);
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
        }
        if matches!(expression.op(), CheckedOp::TypeApply { .. })
            && expression.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
                )
            })
        {
            let place = self.place(expression);
            let handle_ty = mir_place_handle_ty(
                &place,
                expression.borrow_mutability().map(|mutable| {
                    if mutable {
                        mojito_types::origin::Mutability::Mutable
                    } else {
                        mojito_types::origin::Mutability::Immutable
                    }
                }),
            )
            .unwrap_or_else(Ty::error);
            let dest = self.fresh_typed(expression.location(), Some(place.root), handle_ty);
            self.emit(MirInstr::MakeRef { dest, place });
            return dest;
        }
        if matches!(
            expression.op(),
            CheckedOp::Member { .. } | CheckedOp::Index { .. }
        ) {
            // A reference-producing projection may begin with a nominal
            // accessor call (`return self.entries[i].value`). Forward through
            // that selected call's materialized handle just as a `ref` binding
            // or `mut`/`ref` actual does; rebuilding the syntax as a raw List or
            // Dict place would bypass the checked accessor contract.
            if let Some(place) = self.lower_projected_reference_place(expression) {
                let handle_ty = mir_place_handle_ty(&place, None).unwrap_or_else(Ty::error);
                let dest = self.fresh_typed(expression.location(), Some(place.root), handle_ty);
                self.emit(MirInstr::MakeRef { dest, place });
                return dest;
            }
            let place = self.place(expression);
            let handle_ty = mir_place_handle_ty(
                &place,
                expression.borrow_mutability().map(|mutable| {
                    if mutable {
                        mojito_types::origin::Mutability::Mutable
                    } else {
                        mojito_types::origin::Mutability::Immutable
                    }
                }),
            )
            .unwrap_or_else(Ty::error);
            let stored_ty = place.ty.clone().unwrap_or_else(Ty::error);
            let storage = self.fresh_typed(expression.location(), Some(place.root), handle_ty);
            self.emit_interior_invalidations(expression, None);
            self.emit(MirInstr::MakeRef {
                dest: storage,
                place,
            });
            // A slot that stores a handle forwards it (read the stored
            // reference); an owned member/element auto-borrows, so the storage
            // borrow itself is the requested handle.
            if !matches!(stored_ty.kind(), TyKind::Ref(_)) {
                return storage;
            }
            let dest = self.fresh_typed(expression.location(), None, stored_ty);
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
    /// consumption markers, which never hold a runtime value, use `TyKind::None`.
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
            mojito_symbol::symbol::unresolved_overload_marker(name, argc)
        } else {
            name.to_string()
        }
    }

    /// Intern a variable name to a stable `VarId` (matches `hir::Lower::var`).
    /// HIR interns every local at its declaring op, so a name first interned
    /// here that names a value binder in scope is that binder's slot, typed by
    /// the binder's declared type. A struct's own binder is read through the
    /// receiver, never by a bare name.
    fn var(&mut self, name: &str) -> VarId {
        if let Some(i) = self.vars.iter().position(|n| n == name) {
            return i as VarId;
        }
        self.vars.push(name.to_string());
        let var = (self.vars.len() - 1) as VarId;
        let struct_binder = self
            .receiver_value_parameters
            .iter()
            .any(|(parameter, _)| parameter == name);
        if let Some(ty) = self
            .enclosing_binders
            .value_named(name)
            .filter(|_| !struct_binder)
        {
            self.var_types.entry(var).or_insert_with(|| ty.clone());
        }
        var
    }

    /// Intern the runtime slot for one checked binding. Same-spelled lexical
    /// declarations deliberately receive different slots.
    pub(super) fn binding_var(
        &mut self,
        binding: mojito_types::origin::OwnerId,
        name: &str,
    ) -> VarId {
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

    fn declare_binding_var(&mut self, binding: mojito_types::origin::OwnerId, name: &str) -> VarId {
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

    /// The slot of `binding` when it is a local or parameter of the function
    /// being lowered; `None` for a module declaration, which has no slot
    /// here.
    fn local_binding_var(
        &mut self,
        binding: mojito_types::origin::OwnerId,
        name: &str,
    ) -> Option<VarId> {
        if let Some(var) = self.owner_vars.get(&binding).copied() {
            return Some(var);
        }
        self.vars
            .iter()
            .take(self.f.n_params)
            .any(|candidate| candidate == name)
            .then(|| self.binding_var(binding, name))
    }

    /// The root place of slot `var`, through the loan it aliases if any.
    fn var_place(&self, var: VarId) -> MirPlace {
        self.aliases.get(&var).map_or_else(
            || MirPlace::root(var, self.var_types.get(&var).cloned()),
            |loan| {
                let mut place = loan.place.clone();
                place.through = Some(var);
                place
            },
        )
    }

    fn binding_place(&mut self, binding: mojito_types::origin::OwnerId, name: &str) -> MirPlace {
        let var = self.binding_var(binding, name);
        self.var_place(var)
    }

    /// A captured binding's place. A slot that no body expression typed —
    /// a scalar value parameter read only by a nested function — takes the
    /// checked capture's storage type.
    fn capture_place(&mut self, capture: &NestedCapture) -> MirPlace {
        let mut place = self.binding_place(capture.binding, &capture.name);
        if place.root_ty.is_none() && place.through.is_none() {
            place.root_ty = Some(capture.ty.clone());
            place.ty = Some(capture.ty.clone());
            self.var_types.insert(place.root, capture.ty.clone());
        }
        place
    }

    fn resolved_place(&mut self, name: &str) -> MirPlace {
        let var = self.var(name);
        self.var_place(var)
    }

    /// The place name `expression` roots, spelled `name` at runtime: its
    /// node's binding's slot, else the slot of that spelling.
    fn name_place_root(&mut self, name: &str, expression: Node<'a>) -> MirPlace {
        if let Some(place) = self.materialized_parameter_place(expression) {
            return place;
        }
        let node = expression.checked;
        let binding = node.binding;
        // A module constant read where a place is needed (a method's
        // receiver) is materialized into a hidden slot of its own.
        if constant_param_value(&node.adjustments)
            && let Some(value) = recorded_param_value(&node.adjustments)
            && let Some(owner) = binding
        {
            let value = self.param_value_register(expression, value);
            if let (_, Some(place)) = self.materialize_borrow_slot(expression, owner, value) {
                return place;
            }
        }
        let checked_var = binding.map(|owner| self.binding_var(owner, name));
        let var = checked_var.unwrap_or_else(|| self.var(name));
        let mut place = self.var_place(var);
        if place.root_ty.is_none() {
            // HIR's place for a name, whose type the place fact is, exists
            // only where the check bound the name.
            let ty = node
                .binding
                .and_then(|_| node.place_ty.clone())
                .or_else(|| node.ty.clone());
            place.root_ty.clone_from(&ty);
            place.ty.clone_from(&ty);
            if let Some(ty) = ty {
                self.var_types.insert(place.root, ty);
            }
        }
        place
    }

    /// Flatten one or more argument expressions to their result registers.
    fn args(&mut self, args: &[Node<'a>]) -> Vec<Reg> {
        args.iter().map(|&a| self.expr(a)).collect()
    }

    fn param_arg_reg(
        &mut self,
        call: Node<'a>,
        argument: &CheckedParamArg,
        site: &SourceSpan,
    ) -> Option<Reg> {
        let operand = match argument {
            CheckedParamArg::Value(_) => call.param_value(argument),
            CheckedParamArg::Named { value, .. } => match &**value {
                CheckedParamArg::Value(_) => call.param_value(value),
                CheckedParamArg::Attribute { .. } | CheckedParamArg::Named { .. } => return None,
            },
            CheckedParamArg::Attribute {
                value: Some(TyArg::Val(value)),
                ..
            } => return self.attribute_value_reg(value, site),
            // A type or an origin argument is compile-time data on the call.
            CheckedParamArg::Attribute { .. } => return None,
        }?;
        if operand.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument
            )
        }) {
            None
        } else {
            // An argument read off a local display binding (`g[L[0]]()`) is
            // the application the elaborator runs.
            Some(match self.display_read(operand) {
                Some((value, _)) => value,
                None => self.expr(operand),
            })
        }
    }

    /// The register of a compile-time value a type-shaped bracket argument
    /// resolved to. A value pack still a parameter, spread whole (`*vs`,
    /// `*Self.vs`), is the parameter constant of its list, which the
    /// elaborator folds per instance and a backend binds to the callee's
    /// own pack. An enclosing value binder (`Self.n`) is read where it
    /// lives: the receiver's reified parameter for the struct's own. Any
    /// other expression (a display element `L[0]`) is the parameter
    /// constant the elaborator evaluates. `None` for a binder not in scope.
    fn attribute_value_reg(&mut self, value: &CtValue, site: &SourceSpan) -> Option<Reg> {
        let CtValue::Expr(expr) = value else {
            let constant = ParamContext::detached().constant(value.clone()).ok()?;
            return Some(self.param_value_register_at(site.clone(), None, constant));
        };
        if !matches!(expr.kind(), mojito_types::param_expr::ParamKind::DeclRef(_)) {
            return Some(self.param_value_register_at(site.clone(), None, expr.clone()));
        }
        let mojito_types::param_expr::MetaTy::ParamList(element) = expr.meta() else {
            return self.enclosing_value_binder_read(expr, site);
        };
        let element = element.as_value()?.clone();
        let dest = self.fresh_typed(site.clone(), None, TyKind::VariadicPack(element).into());
        self.emit(MirInstr::Const {
            dest,
            k: Const::Param(expr.clone()),
        });
        Some(dest)
    }

    /// Whether a bracket argument occupies its slot with no run-time form.
    ///
    /// Semantic Origin/OriginSet arguments are retained in source long enough
    /// for the checker to solve reference and capture contracts, but they have
    /// no slot in `ParamDecl`.  Drop those arguments from the runtime parameter
    /// vector instead of emitting an ambiguous `None` that would shift every
    /// following callable/scalar value parameter. A type-shaped argument the
    /// check resolved to an origin (`V[Self.o]`) is one, as is a value-shaped
    /// one it marked `EraseCompileTimeArgument`. A closed value the call
    /// carries in its instantiated arguments (`folded`) is compile-time data
    /// on the call, as at Mojo's.
    fn param_arg_has_no_runtime_form(
        call: Node<'a>,
        argument: &CheckedParamArg,
        folded: bool,
    ) -> bool {
        let operand = match argument {
            CheckedParamArg::Value(_) => call.param_value(argument),
            CheckedParamArg::Named { value, .. } => match &**value {
                CheckedParamArg::Value(_) => call.param_value(value),
                CheckedParamArg::Attribute { value, .. } => return resolved_origin(value.as_ref()),
                CheckedParamArg::Named { .. } => None,
            },
            CheckedParamArg::Attribute { value, .. } => return resolved_origin(value.as_ref()),
        };
        folded
            || operand.is_some_and(|operand| {
                operand.adjustments().iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument
                    )
                })
            })
    }

    /// `Self.n` in a bracket slot, where `n` is the enclosing struct's own
    /// value parameter (`Counter[Self.length](i)`): read the receiver's
    /// reified parameter exactly as the expression `Self.n` does (the field
    /// projection `self.n`, which every backend's field navigation resolves
    /// through the struct's value parameters). A static method reads it off
    /// a receiver-less `self` slot typed as the struct at its own binders,
    /// as the expression `Self.n` there does. `None` when the frame has no
    /// receiver or `n` is not one of its value parameters.
    fn receiver_value_parameter_read(&mut self, name: &str, site: &SourceSpan) -> Option<Reg> {
        let place = self.receiver_value_parameter_place(name)?;
        let ty = place.ty.clone()?;
        let dest = self.fresh_typed(site.clone(), Some(place.root), ty);
        self.emit(MirInstr::LoadPlace { dest, place });
        Some(dest)
    }

    /// The place `Self.n` names: the projection `self.n` of the receiver,
    /// or of a static method's receiver-less `self` slot. `None` when `n` is
    /// not one of the enclosing struct's value parameters.
    fn receiver_value_parameter_place(&mut self, name: &str) -> Option<MirPlace> {
        let ty = self
            .receiver_value_parameters
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, ty)| ty.clone())?;
        if !self.vars.iter().any(|var| var == "self") {
            self.intern_static_self()?;
        }
        let mut place = self.resolved_place("self");
        place.project(Proj::Field(name.to_string()), ty);
        Some(place)
    }

    /// The receiver-less `self` slot of a static method of a
    /// value-parameterized struct, typed as the struct at its own binders: a
    /// `Self.n` read projects from it, and the VM binds it from the static
    /// call's spelled receiver. `None` outside such a method.
    fn intern_static_self(&mut self) -> Option<VarId> {
        let receiver = self.static_receiver.clone()?;
        let var = self.var("self");
        self.var_types.entry(var).or_insert(receiver);
        Some(var)
    }

    /// The expression a bracket argument built from the enclosing value
    /// binders denotes (`1 + n`, `Self.n`); none for an argument naming no
    /// binder, whose register is already its constant.
    fn forwarded_value(&self, argument: &CheckedParamArg) -> Option<ParamExpr> {
        let expr = match argument {
            CheckedParamArg::Named { value, .. } => return self.forwarded_value(value),
            // An enclosing value binder the argument names (`Self.n`).
            CheckedParamArg::Attribute {
                value: Some(TyArg::Val(CtValue::Expr(expr))),
                ..
            } => self.enclosing_binders.binder_value(expr),
            CheckedParamArg::Attribute { .. } => None,
            CheckedParamArg::Value(value) => self.value_binder_node(*value),
        }?;
        expr.as_constant().is_none().then_some(expr)
    }

    /// The local display bindings of this function the expression of node
    /// `node` reads by name, in declaration order, with the ones those
    /// displays read in turn. Such a binding has no runtime form here.
    fn displays_read(&self, node: CheckedNodeId) -> Vec<DisplayBinding<'a>> {
        if self.displays.is_empty() {
            return Vec::new();
        }
        let mut read = bindings_read(self.checked, node);
        let mut found = Vec::new();
        for display in self.displays.iter().rev() {
            if read.contains(&display.owner) {
                read.extend(bindings_read(self.checked, display.value.id()));
                found.push(display.clone());
            }
        }
        found.reverse();
        found
    }

    /// Lower a compile-time `expression` of this function's body that reads
    /// a local display binding, or that the check lifted by name
    /// ([`Self::comptime_application`]), with the parameter expression it
    /// denotes.
    /// An `Int` or a `Bool` (`L[0]`, `len(L) + 1`) is the application of the
    /// thunk lifted for it, which the elaborator runs per instance. Any
    /// other value (`(L[0], L[1])`) is computed here, where it crosses to
    /// runtime, from the displays built in place, and denotes no parameter
    /// expression. `None` for an expression that reads no display.
    fn display_read(&mut self, expression: Node<'a>) -> Option<(Reg, Option<ParamExpr>)> {
        let node = expression.id();
        let displays = self.displays_read(node);
        // An expression the check lifted denotes its application, which the
        // elaborator evaluates per instance.
        if let Some(application) = self.comptime_application(expression) {
            let value = self.param_value_register(expression, application.clone());
            return Some((value, Some(application)));
        }
        if displays.is_empty() {
            return None;
        }
        let ty = match expression.ty().as_ref().map(Ty::kind) {
            Some(TyKind::Int | TyKind::IntLiteral) => Ty::int(),
            Some(TyKind::Bool) => Ty::bool(),
            _ => {
                for display in &displays {
                    self.build_display(display);
                }
                return Some((self.expr(expression), None));
            }
        };
        let binders = &self.enclosing_binders;
        let denoted = self.comptime_thunks.request_value(node, binders, ty);
        Some((
            self.param_value_register(expression, denoted.clone()),
            Some(denoted),
        ))
    }

    /// The application the check lifted `expression` as, by the name of the
    /// function [`ComptimeThunks::request_applications`] lifts for it.
    /// `None` inside that function, which computes the expression.
    fn comptime_application(&self, expression: Node<'a>) -> Option<ParamExpr> {
        self.node_application(expression.checked)
    }

    /// [`Self::comptime_application`] of node `node`.
    fn node_application(&self, node: &mojito_checked::checked::CheckedExpr) -> Option<ParamExpr> {
        lifted_application(&node.adjustments).filter(|application| {
            !matches!(application.kind(),
                mojito_types::param_expr::ParamKind::Apply { function, .. }
                    if *function == self.comptime_thunks.owner)
        })
    }

    /// Build a local display binding into its slot, for the crossing that
    /// reads it next.
    fn build_display(&mut self, binding: &DisplayBinding<'a>) {
        self.with_statement_loans(|this| {
            let built = this.expr(binding.value);
            this.bind_comptime_value(&binding.name, binding.value, Some(binding.owner), built);
        });
    }

    /// The runtime value of a local display binding crossing whole
    /// (`materialize[L]()`): the display built where it crosses, after the
    /// displays it reads.
    fn crossing_display(&mut self, binding: &DisplayBinding<'a>) -> Reg {
        for read in self.displays_read(binding.value.id()) {
            self.build_display(&read);
        }
        self.with_statement_loans(|this| this.expr(binding.value))
    }

    /// The local display binding `expression` names.
    fn named_display(&self, expression: Node<'a>) -> Option<DisplayBinding<'a>> {
        expression
            .is_name()
            .then(|| expression.owner())
            .flatten()
            .and_then(|owner| self.display(owner))
            .cloned()
    }

    /// The display binding in scope `owner` is, if it is one.
    fn display(&self, owner: mojito_types::origin::OwnerId) -> Option<&DisplayBinding<'a>> {
        self.displays.iter().find(|display| display.owner == owner)
    }

    /// Node `id` over the enclosing value binders, through the shared typed
    /// builder: names resolve here, operator semantics and canonical form
    /// are [`ParamContext`]'s.
    fn value_binder_node(&self, id: mojito_checked::checked::CheckedNodeId) -> Option<ParamExpr> {
        use mojito_checked::checked::CheckedOp;
        let node = self.checked.expression(id)?;
        // A compile-time query the checker recorded (`Self.Ts.length`, a
        // `comptime for` index) is its parameter expression.
        if let Some(value) =
            recorded_param_value(&node.adjustments).or_else(|| self.node_application(node))
        {
            return Some(value);
        }
        let context = ParamContext::detached();
        match &node.op {
            CheckedOp::Int(value) => context
                .constant(mojito_types::ct::CtValue::IntLiteral(value.clone()))
                .ok(),
            // A binder, or a local `comptime` binding of an expression over
            // them.
            CheckedOp::Identifier(name) => self
                .enclosing_binders
                .value(&self.spelling(node, name), false)
                .or_else(|| {
                    node.binding
                        .and_then(|owner| self.comptime_thunks.bound_value(owner))
                }),
            CheckedOp::Member { object, field }
                if self.checked.expression(*object).is_some_and(|object| {
                    matches!(&object.op, CheckedOp::Identifier(name)
                        if self.spelling(object, name) == "Self")
                }) =>
            {
                self.enclosing_binders.value(field, true)
            }
            CheckedOp::Prefix(PrefixOp::Neg, value) => {
                context.neg(&self.value_binder_node(*value)?).ok()
            }
            CheckedOp::Infix(
                op @ (InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Pow
                | InfixOp::Shl),
                left,
                right,
            ) => context
                .infix(
                    *op,
                    &self.value_binder_node(*left)?,
                    &self.value_binder_node(*right)?,
                )
                .ok(),
            _ => None,
        }
    }

    /// An erased argument keeps its (empty) slot: every backend aligns the
    /// slots with the callee's declarations positionally, so a dropped slot
    /// would shift the reified arguments after it (`Dict[String, Int, H]`).
    fn param_arg_regs(&mut self, call: Node<'a>) -> Vec<MirParamArg> {
        let folded = call
            .adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::FoldedParameterArguments(
                    positions,
                ) => Some(positions),
                _ => None,
            })
            .cloned()
            .unwrap_or_default();
        self.param_arg_regs_folding(call, &folded)
    }

    /// The compile-time argument slots of an indirect call. A folded
    /// argument's value travels in the instantiated arguments, which only
    /// an instantiated callable contract carries: without one, the slot
    /// keeps its register.
    fn indirect_param_arg_regs(&mut self, call: Node<'a>) -> Vec<MirParamArg> {
        if call.instantiated_callable_contract().is_some() {
            self.param_arg_regs(call)
        } else {
            self.param_arg_regs_folding(call, &[])
        }
    }

    fn param_arg_regs_folding(&mut self, call: Node<'a>, folded: &[usize]) -> Vec<MirParamArg> {
        let site = call.location();
        let mut registers = Vec::new();
        for (position, argument) in call.param_args().iter().enumerate() {
            let name = match argument {
                CheckedParamArg::Named { name, .. } => Some(name.clone()),
                CheckedParamArg::Attribute { .. } | CheckedParamArg::Value(_) => None,
            };
            let value = if Self::param_arg_has_no_runtime_form(
                call,
                argument,
                folded.contains(&position),
            ) {
                None
            } else {
                self.param_arg_reg(call, argument, &site)
            };
            let expr = value.and_then(|_| self.forwarded_value(argument));
            registers.push(MirParamArg { name, value, expr });
        }
        registers
    }

    /// The evaluation of an `Int` parameter expression over the enclosing
    /// value binders: its constants, binder reads, the applications the
    /// elaborator answers, and the `+`, `*`, `-`, and negation it is built
    /// from. `None` for any other shape.
    fn inferred_value_register(&mut self, expr: &ParamExpr, site: &SourceSpan) -> Option<Reg> {
        // A binder of any scalar type reads its own slot; a `DType` constant
        // is its own; the arithmetic below is over `Int` alone.
        if let mojito_types::param_expr::ParamKind::DeclRef(_) = expr.kind() {
            return self.enclosing_value_binder_read(expr, site);
        }
        if let mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Dtype(
            dtype,
        )) = expr.kind()
        {
            let dest = self.fresh_typed(site.clone(), None, Ty::dtype());
            self.emit(MirInstr::Const {
                dest,
                k: Const::Dtype(*dtype),
            });
            return Some(dest);
        }
        // An element or the length of a local display binding is a
        // parameter constant the elaborator answers.
        if let (
            mojito_types::param_expr::ParamKind::ListGet { .. }
            | mojito_types::param_expr::ParamKind::Apply { .. },
            Some(ty),
        ) = (expr.kind(), expr.meta().as_value())
        {
            let dest = self.fresh_typed(site.clone(), None, ty.clone());
            self.emit(MirInstr::Const {
                dest,
                k: Const::Param(expr.clone()),
            });
            return Some(dest);
        }
        if *expr.meta() != mojito_types::param_expr::MetaTy::value(Ty::int()) {
            return None;
        }
        let (op, operands) = match expr.kind() {
            mojito_types::param_expr::ParamKind::Constant(value) => {
                let value = match value {
                    mojito_types::ct::CtValue::Int(value) => *value,
                    mojito_types::ct::CtValue::IntLiteral(value) => value.to_i64()?,
                    _ => return None,
                };
                let dest = self.fresh_typed(site.clone(), None, Ty::int());
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::Int(value),
                });
                return Some(dest);
            }
            mojito_types::param_expr::ParamKind::Op { op, operands } => (op, operands),
            _ => return None,
        };
        let infix = match op {
            mojito_types::param_expr::ParamOp::Add => InfixOp::Add,
            mojito_types::param_expr::ParamOp::Mul => InfixOp::Mul,
            mojito_types::param_expr::ParamOp::Sub => InfixOp::Sub,
            mojito_types::param_expr::ParamOp::Neg => {
                let [operand] = operands.as_slice() else {
                    return None;
                };
                let a = self.inferred_value_register(operand, site)?;
                let dest = self.fresh_typed(site.clone(), None, Ty::int());
                self.emit(MirInstr::UnOp {
                    op: PrefixOp::Neg,
                    dest,
                    a,
                });
                return Some(dest);
            }
            _ => return None,
        };
        let (first, rest) = operands.split_first()?;
        let mut accumulated = self.inferred_value_register(first, site)?;
        for operand in rest {
            let b = self.inferred_value_register(operand, site)?;
            let dest = self.fresh_typed(site.clone(), None, Ty::int());
            self.emit(MirInstr::BinOp {
                op: infix,
                dest,
                a: accumulated,
                b,
                resolved: None,
            });
            accumulated = dest;
        }
        Some(accumulated)
    }

    /// A read of the enclosing value binder `expr` names, when it is exactly
    /// such a reference: the receiver's reified parameter for the enclosing
    /// struct's own (`Self.k`), otherwise the function's local of its name.
    fn enclosing_value_binder_read(&mut self, expr: &ParamExpr, site: &SourceSpan) -> Option<Reg> {
        let mojito_types::param_expr::ParamKind::DeclRef(reference) = expr.kind() else {
            return None;
        };
        let values = &self.enclosing_binders.values;
        let ty = values
            .iter()
            .rfind(|(binder, _)| binder == reference)
            .map(|(_, ty)| ty.clone())?;
        let struct_binder = values
            .iter()
            .find(|(binder, _)| binder.name == reference.name)
            .is_some_and(|(binder, _)| binder == reference)
            && self
                .receiver_value_parameters
                .iter()
                .any(|(name, _)| name.as_str() == reference.name.as_ref());
        if struct_binder {
            return self.receiver_value_parameter_read(&reference.name, site);
        }
        let var = self.var(&reference.name);
        let dest = self.fresh_typed(site.clone(), Some(var), ty);
        self.emit(MirInstr::UseVar {
            dest,
            var,
            mode: UseMode::Copy,
        });
        Some(dest)
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
    binding: mojito_types::origin::OwnerId,
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

/// [`lower_cfg`] with a nested-`def` registry in scope: a call to a registered
/// nested `def` is rewritten to its lifted function (captures prepended) and the
/// nested `def` statement lowers to nothing. `declared` types the slots the
/// frame's declaration binds before its body runs (parameters, value
/// parameters, a named result); every other slot is typed by the op that
/// declares it as the body lowers.
#[allow(clippy::too_many_arguments)]
fn lower_cfg_nested(
    cfg: &Cfg,
    declared: HashMap<VarId, Ty>,
    nested: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    overloads: &std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    returns_reference: bool,
    reference_parameters: &[bool],
    capture_bindings: &[mojito_types::origin::OwnerId],
    receiver_value_parameters: &[(String, Ty)],
    static_receiver: Option<&Ty>,
    enclosing_binders: &EnclosingBinders,
    comptime_thunks: &mut ComptimeThunks,
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
        let fallback = Terminator::FallOff;
        let mut fl = Flatten {
            f: &mut mir,
            cur: 0,
            next_reg: 0,
            vars: cfg.vars.clone(),
            var_types: declared,
            owner_vars: capture_bindings
                .iter()
                .copied()
                .enumerate()
                .map(|(slot, binding)| (binding, slot as VarId))
                .collect(),
            nested: nested.clone(),
            receiver_value_parameters: receiver_value_parameters.to_vec(),
            static_receiver: static_receiver.cloned(),
            enclosing_binders: enclosing_binders.clone(),
            comptime_thunks: std::mem::take(comptime_thunks),
            parameter_reads: enclosing_binders.loop_bindings.iter().cloned().collect(),
            overloads: std::sync::Arc::clone(overloads),
            checked: &cfg.checked,
            displays: Vec::new(),
            slots: cfg.slots.clone(),
            aliases: HashMap::new(),
            runtime_aliases: reference_parameters
                .iter()
                .take(cfg.n_params)
                .enumerate()
                .filter_map(|(slot, reference)| reference.then_some(slot as VarId))
                .collect(),
            aggregate_loans: HashMap::new(),
            conversion_view_loans: HashMap::new(),
            pending_argument_anchors: Vec::new(),
            call_setups: Vec::new(),
            setup_reference: None,
            argument_depth: 0,
            allow_argument_anchors: false,
            transfer_domain_loans: HashMap::new(),
            reassigned_names: reassigned_names(cfg, nested),
            returns_reference,
        };
        let loop_scopes = loop_index_scopes(cfg);
        for hb in cfg.g.node_indices() {
            fl.cur = map[&hb];
            // A `comptime for` body sees the loop's index as a value binder,
            // so a bracket argument built from it (`g[i]()`) resolves.
            fl.enclosing_binders =
                enclosing_binders.with_loops(loop_scopes.get(&hb).map_or(&[], Vec::as_slice));
            fl.comptime_thunks.enter_scope(&fl.enclosing_binders);
            for instr in &cfg.g[hb].instrs {
                // At the function level the "outer" map is this function's own map
                // (a `try`'s escape targets are this function's loop blocks).
                fl.lower_instr(instr, &map);
            }
            let term = cfg.g[hb].term.as_ref().unwrap_or(&fallback);
            let mterm = fl.lower_term(term, &map, &map);
            fl.f.blocks[fl.cur].term = mterm;
        }
        fl.f.n_regs = fl.next_reg;
        // The MIR flattener may intern additional locals beyond the HIR's set
        // (short-circuit / iterator temporaries), so take the final interner.
        fl.f.n_vars = fl.vars.len();
        fl.f.var_names.clone_from(&fl.vars);
        fl.f.var_tys.clone_from(&fl.var_types);
        *comptime_thunks = std::mem::take(&mut fl.comptime_thunks);
    } // `fl` (the &mut borrow of `mir`) ends here

    mir
}

/// Translate a source parameter marker into the runtime frame layout. Named
/// `out` results are callee-local slots, so they do not consume an incoming
/// argument position; variadic collectors do consume one frame position.
#[allow(
    clippy::single_option_map,
    reason = "all 14 call sites thread Option<usize> through unchanged"
)]
fn runtime_parameter_index<P: DeclaredParam>(params: &[P], marker: Option<usize>) -> Option<usize> {
    marker.map(|index| {
        params[..index]
            .iter()
            .filter(|parameter| {
                !matches!(parameter.convention(), Some(ArgConvention::Out))
                    && parameter.kind() != ParamKind::KwVariadic
            })
            .count()
    })
}

/// `*args` is inserted among regular incoming arguments before the collector is
/// materialized, so only preceding non-`out` regular parameters determine its
/// insertion point.
#[allow(
    clippy::single_option_map,
    reason = "all 14 call sites thread Option<usize> through unchanged"
)]
fn runtime_variadic_index<P: DeclaredParam>(params: &[P], marker: Option<usize>) -> Option<usize> {
    marker.map(|index| {
        params[..index]
            .iter()
            .filter(|parameter| {
                parameter.kind() == ParamKind::Regular
                    && !matches!(parameter.convention(), Some(ArgConvention::Out))
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
fn body_parameter_ty<P: DeclaredParam>(parameter: &P, ty: Ty) -> Ty {
    match (parameter.kind(), ty.kind()) {
        (ParamKind::Variadic, TyKind::RuntimePack(elements)) => {
            TyKind::Tuple(elements.clone()).into()
        }
        (ParamKind::Variadic, _) => TyKind::VariadicPack(ty).into(),
        (ParamKind::KwVariadic, _) => TyKind::Struct(
            "StringDict".to_string(),
            vec![mojito_types::types::TyArg::Ty(ty)],
        )
        .into(),
        _ => ty,
    }
}

fn generic_callable_param_decls(callable: &Ty) -> Vec<ParamDecl> {
    match callable.kind() {
        TyKind::GenericFunc(generic) => generic.decls.clone(),
        TyKind::Param {
            callable_bound: Some(bound),
            ..
        } => generic_callable_param_decls(bound),
        _ => Vec::new(),
    }
}

#[derive(Clone, PartialEq, Eq)]
struct NestedCapture {
    name: String,
    binding: mojito_types::origin::OwnerId,
    ty: Ty,
    kind: mojito_ast::ast::CaptureKind,
}

/// The runtime spelling of name node `node`: the slot HIR resolved it to,
/// a `$contextual` root's resolved base type, else `None`, for a name
/// spelled as written or a node that is not a name.
fn name_spelling(
    slots: &HashMap<mojito_checked::checked::CheckedNodeId, VarId>,
    vars: &[String],
    node: &mojito_checked::checked::CheckedExpr,
) -> Option<String> {
    if let Some(slot) = slots.get(&node.id) {
        return vars.get(*slot as usize).cloned();
    }
    match &node.op {
        mojito_checked::checked::CheckedOp::Identifier(name)
            if name == mojito_ast::ast::CONTEXTUAL_SENTINEL =>
        {
            node.contextual_base.clone()
        }
        _ => None,
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
                instr if let Some(ty) = instr.call_error() => {
                    return Some(ty.clone());
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
/// are typed `TyKind::None`. Reference-handle permission follows the place root: a
/// capability root is forwarded (and projections cannot recover stronger
/// permission), while borrowing ordinary storage adds an outer reference layer
/// even when the stored value is itself a reference. Handle loan bookkeeping
/// lives in `EstablishLoans`/`through` metadata, not in the synthetic register
/// origin.
#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
fn close_register_types(
    name: &str,
    f: &mut MirFunction,
    declarations: &MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    fn deref(ty: Ty) -> Ty {
        match ty.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => ty,
        }
    }
    fn declared_return(declarations: &MirDeclarations, callee: &str) -> Option<Ty> {
        declarations
            .function(callee)
            .map(|declaration| declaration.ret_ty.clone())
    }
    fn struct_field(declarations: &MirDeclarations, base: &Ty, field: &str) -> Option<Ty> {
        let TyKind::Struct(name, _) = base.kind() else {
            return None;
        };
        declarations
            .structure(name)?
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
            for instr in &block.instrs {
                if let MirInstr::TryNext { yielded, .. } = instr {
                    changed |= record(reg_types, yielded, Some(Ty::bool()));
                }
                let derived: Option<(&Reg, Option<Ty>)> = match instr {
                    MirInstr::Const { dest, k } => {
                        let ty = match k {
                            Const::Int(value) => {
                                const_ints.insert(dest.0, *value);
                                Some(Ty::int())
                            }
                            Const::Float(_) => Some(Ty::float64()),
                            Const::IntLiteral(_) => Some(Ty::int_literal()),
                            Const::FloatLiteral(_) => Some(Ty::float_literal()),
                            Const::Bool(_) => Some(Ty::bool()),
                            Const::Str(_) => Some(Ty::string_literal()),
                            Const::None => Some(Ty::none()),
                            Const::Dtype(_) => Some(Ty::dtype()),
                            // A callable constant's type needs the checked
                            // expression; report rather than reconstruct.
                            Const::Function(_) => None,
                            Const::Param(value) => value.meta().as_value().cloned(),
                            Const::Value(value) => {
                                mojito_types::param_expr::MetaTy::of_value(value)
                                    .as_value()
                                    .cloned()
                            }
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
                        reg_types
                            .get(&reference.0)
                            .cloned()
                            .map(|ty| match ty.kind() {
                                TyKind::Ref(reference) => reference.referent.clone(),
                                TyKind::Pointer { element, .. } => (*element).clone(),
                                _ => ty,
                            }),
                    )),
                    MirInstr::CopyValue { dest, value } => {
                        Some((dest, reg_types.get(&value.0).cloned()))
                    }
                    MirInstr::UnOp { op, dest, a } => Some((
                        dest,
                        match op {
                            PrefixOp::Not => Some(Ty::bool()),
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
                            | InfixOp::NotIn
                            | InfixOp::Is
                            | InfixOp::IsNot => Some(Ty::bool()),
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
                        reg_types.get(&base.0).and_then(|base| match base.kind() {
                            TyKind::Pointer { element, .. } => Some(element.clone()),
                            TyKind::Simd { dtype, .. } => Some(
                                TyKind::Simd {
                                    dtype: dtype.clone(),
                                    width: mojito_types::types::SimdWidth::Known(1),
                                }
                                .into(),
                            ),
                            // A tuple element type needs the compile-time
                            // index the checker already validated.
                            TyKind::Tuple(elements) => const_ints
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
                            .and_then(mojito_types::types::callable_contract_ty)
                            .and_then(|ty| match ty.kind() {
                                TyKind::Func(func) => Some(func.ret.clone()),
                                _ => None,
                            }),
                    )),
                    MirInstr::MakeTuple {
                        dest,
                        element_types: Some(elements),
                        ..
                    } => Some((dest, Some(TyKind::Tuple(elements.clone()).into()))),
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
                        Some((
                            dest,
                            elements.map(|elements| TyKind::Tuple(elements).into()),
                        ))
                    }
                    MirInstr::MakeVariant {
                        dest, alternatives, ..
                    } => Some((dest, Some(TyKind::Variant(alternatives.clone()).into()))),
                    MirInstr::MakeSimd {
                        dest, dtype, width, ..
                    }
                    | MirInstr::SimdCast {
                        dest, dtype, width, ..
                    }
                    | MirInstr::SimdBitcast {
                        dest, dtype, width, ..
                    } => Some((
                        dest,
                        mojito_types::types::simd_ty_from_slots(dtype.clone(), width.clone()).ok(),
                    )),
                    // A shuffle keeps the source dtype at a known mask's
                    // width; a template's result is its checked type.
                    MirInstr::SimdShuffle {
                        dest, value, mask, ..
                    } => {
                        let dtype = match reg_types.get(&value.0).map(Ty::kind) {
                            Some(TyKind::Simd { dtype, .. }) => Some(dtype.clone()),
                            _ => None,
                        };
                        Some((
                            dest,
                            dtype.zip(mask.known()).map(|(dtype, mask)| {
                                TyKind::Simd {
                                    dtype,
                                    width: mojito_types::types::SimdWidth::Known(mask.len() as i64),
                                }
                                .into()
                            }),
                        ))
                    }
                    MirInstr::VariantIs { dest, .. } => Some((dest, Some(Ty::bool()))),
                    MirInstr::VariantSetInitWith { dest, .. }
                    | MirInstr::VariantDeinitWith { dest, .. } => Some((dest, Some(Ty::none()))),
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
                        reg_types.get(&variant.0).and_then(|ty| match ty.kind() {
                            TyKind::Variant(alternatives) => index
                                .known()
                                .and_then(|index| alternatives.get(index))
                                .cloned(),
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
                        place.ty.as_ref().and_then(|ty| match ty.kind() {
                            TyKind::Variant(alternatives) => output_index
                                .known()
                                .and_then(|index| alternatives.get(index))
                                .cloned(),
                            _ => None,
                        }),
                    )),
                    MirInstr::HasNext { dest, .. } => Some((dest, Some(Ty::bool()))),
                    MirInstr::Next { dest, .. } => {
                        // Compiler-private storage has no call contract, so the
                        // element type comes from the slot the element binds.
                        let result = blocks
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
                            });
                        Some((dest, result))
                    }
                    MirInstr::TryNext { dest, call, .. } => {
                        Some((dest, Some(call.result_ty.clone())))
                    }
                    MirInstr::EstablishLoans { marker, .. }
                    | MirInstr::InvalidateInteriors { marker, .. }
                    | MirInstr::ConsumePlace { marker, .. } => Some((marker, Some(Ty::none()))),
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

/// Give each trait default wrapper's declaration its member's parameter
/// defaults, as upstream's wrapper takes its trait function's signature
/// (`getTraitFunctionSignature`): the member's declaration holds each
/// default once, for every conformer. An evaluated default is applied to
/// the wrapper's instantiation of the member, which its forwarding call
/// records, so the default's function binds its binders from the wrapper's.
fn adopt_member_defaults(
    checked: &CheckedProgram,
    declarations: &mut MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    for wrapper in checked.default_wrappers() {
        match member_defaults(checked, declarations, wrapper) {
            Ok((defaults, required)) => {
                if let Some(declaration) = declarations
                    .functions
                    .iter_mut()
                    .find(|declaration| declaration.lowered_name == wrapper.lowered_name)
                {
                    declaration.defaults = defaults;
                    declaration.required = required;
                }
            }
            Err(error) => invariant_errors.push(format!(
                "trait default wrapper '{}' of member '{}': {error}",
                wrapper.lowered_name, wrapper.member
            )),
        }
    }
}

/// The defaults and required flags `wrapper` adopts from its member's
/// declaration: every parameter's after the member's leading `self`.
fn member_defaults(
    checked: &CheckedProgram,
    declarations: &MirDeclarations,
    wrapper: &mojito_checked::checked::CheckedDefaultWrapper,
) -> Result<(Vec<Option<CheckedConst>>, Vec<bool>), String> {
    let own = declarations
        .function(&wrapper.lowered_name)
        .ok_or("its declaration was not lowered")?;
    let member = declarations
        .function(&wrapper.member)
        .ok_or("the member's declaration is not reached")?;
    if member.has_receiver || member.param_names.first().map(String::as_str) != Some("self") {
        return Err("the member does not take its receiver first".into());
    }
    if member.param_names[1..] != own.param_names[..] {
        return Err(format!(
            "parameters {:?} differ from the member's {:?}",
            own.param_names,
            &member.param_names[1..]
        ));
    }
    let call = wrapper
        .body
        .first()
        .and_then(|statement| match &statement.kind {
            StmtKind::Return(Some(call)) | StmtKind::Expr(call) => Some(call),
            _ => None,
        })
        .ok_or("its body is not one forwarding call")?;
    let instantiation = checked
        .generic_instantiations()
        .get(&call.source_span())
        .map_or(&[][..], |instantiation| &instantiation.arguments[..]);
    let defaults = member.defaults[1..]
        .iter()
        .map(|default| match default {
            Some(CheckedConst::Evaluate {
                function,
                arguments,
            }) if arguments.is_empty() => {
                let binders = declarations
                    .function(function)
                    .map_or(&[][..], |thunk| &thunk.param_decls[..]);
                let arguments = binders
                    .iter()
                    .map(|binder| {
                        member
                            .param_decls
                            .iter()
                            .position(|own| own.id() == binder.id())
                            .and_then(|position| instantiation.get(position))
                            .cloned()
                            .ok_or_else(|| {
                                format!(
                                    "the call binds no argument for `{}` of `{function}`",
                                    binder.name()
                                )
                            })
                    })
                    .collect::<Result<_, _>>()?;
                Ok(Some(CheckedConst::Evaluate {
                    function: function.clone(),
                    arguments,
                }))
            }
            default => Ok(default.clone()),
        })
        .collect::<Result<_, String>>()?;
    Ok((defaults, member.required[1..].to_vec()))
}

/// The checked bodies and handler binder of one structured `try`, read from
/// its statement node. Keeping these parts together makes the primary HIR and
/// fallback lowering paths share one region-lowering contract.
struct TryRegions<'a> {
    body: &'a [CheckedStmtId],
    except: Option<(Option<&'a str>, &'a [CheckedStmtId])>,
    orelse: Option<&'a [CheckedStmtId]>,
    finalbody: Option<&'a [CheckedStmtId]>,
    handler_binding: Option<mojito_types::origin::OwnerId>,
}

impl<'a> TryRegions<'a> {
    /// The regions of `node`, when it is a `try`.
    fn of(node: &'a CheckedStmt) -> Option<Self> {
        let CheckedStmtOp::Try {
            body,
            except,
            orelse,
            finalbody,
        } = &node.op
        else {
            return None;
        };
        Some(Self {
            body,
            except: except
                .as_ref()
                .map(|(name, body)| (name.as_deref(), body.as_slice())),
            orelse: orelse.as_deref(),
            finalbody: finalbody.as_deref(),
            handler_binding: node.binding,
        })
    }

    /// Every statement list the `try` runs.
    fn bodies(&self) -> impl Iterator<Item = &'a [CheckedStmtId]> {
        std::iter::once(self.body)
            .chain(self.except.map(|(_, body)| body))
            .chain(self.orelse)
            .chain(self.finalbody)
    }
}

/// The invariant construction state shared by every recursive level of a
/// collection comprehension. Only the clause cursor varies as the nested
/// control-flow tree is emitted.
struct ComprehensionPlan<'p, 'a> {
    collection: VarId,
    target: &'p Ty,
    insert: &'p str,
    key: Option<Node<'a>>,
    value: Node<'a>,
    /// The checked clauses, which decide each clause's kind, binder, and
    /// iterable or condition node.
    clauses: &'p [mojito_checked::checked::CheckedComprehensionClause],
    /// The comprehension's handle, whose operands the clauses name.
    node: Node<'a>,
    /// The checked binder of each `for` clause, in clause order.
    bindings: &'p [mojito_checked::checked::CheckedComprehensionBinding],
}

impl<'a> ComprehensionPlan<'_, 'a> {
    /// The handle of clause operand `id`, an operand of the comprehension.
    fn operand(&self, id: mojito_checked::checked::CheckedNodeId) -> Node<'a> {
        self.node
            .operand_of(id)
            .expect("a comprehension clause's node is its operand")
    }
}

/// The binders in scope at a declaration: the enclosing struct's and
/// functions', then its own.
#[derive(Clone, Default)]
struct EnclosingBinders {
    types: Vec<mojito_types::param_expr::ParamRef>,
    /// The non-callable value binders, each with its declared type.
    values: Vec<(mojito_types::param_expr::ParamRef, Ty)>,
    /// The declarations of `types` and `values`, in scope order.
    declarations: Vec<ParamDecl>,
    /// The enclosing `comptime for` loops' variables, each checked binding
    /// with the index binder it denotes.
    loop_bindings: Vec<(
        mojito_types::origin::OwnerId,
        mojito_types::param_expr::ParamRef,
    )>,
    /// In a lifted thunk, the local `comptime` bindings of a parameter
    /// expression in the function it was lifted from (`comptime k = n +
    /// 1`), each checked binding with the expression it denotes.
    comptime_bindings: Vec<(mojito_types::origin::OwnerId, ParamExpr)>,
    /// Whether these are a lifted thunk's binders: its body binds each
    /// local `comptime` binding it begins with as an ordinary value.
    lifted: bool,
}

impl EnclosingBinders {
    fn of<'a>(declarations: impl IntoIterator<Item = &'a ParamDecl>) -> Self {
        Self::default().with(declarations)
    }

    /// These binders, then those of a declaration nested inside them.
    fn with<'a>(&self, declarations: impl IntoIterator<Item = &'a ParamDecl>) -> Self {
        let mut binders = self.clone();
        for declaration in declarations {
            match declaration {
                ParamDecl::Type { .. } => binders.types.push(declaration.binder()),
                ParamDecl::Value {
                    ty,
                    variadic: false,
                    callable_default: None,
                    ..
                } if !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => {
                    binders.values.push((declaration.binder(), (*ty).clone()));
                }
                // A value pack has no frame local; a thunk reads it through
                // the parameter constants it binds.
                ParamDecl::Value {
                    ty,
                    variadic: true,
                    callable_default: None,
                    ..
                } if !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => {}
                ParamDecl::Value { .. } => continue,
            }
            binders.declarations.push(declaration.clone());
        }
        binders
    }

    /// The declared type of the innermost value binder named `name`.
    fn value_named(&self, name: &str) -> Option<&Ty> {
        self.values
            .iter()
            .rfind(|(binder, _)| binder.name.as_ref() == name)
            .map(|(_, ty)| ty)
    }

    /// These binders, then the indices of the `comptime for` loops
    /// enclosing a block, outermost first.
    fn with_loops(&self, indices: &[LoopIndex]) -> Self {
        let mut binders = self.with(indices.iter().map(|index| &index.declaration));
        binders.loop_bindings.extend(
            indices
                .iter()
                .filter_map(|index| Some((index.binding?, index.declaration.binder()))),
        );
        binders
    }

    /// Whether checked node `node` of `tables` spells `Self` or one of these
    /// binders: a name or a call it reaches, a bracket argument or a type
    /// in value position the check resolved over one of them, or a lambda
    /// whose checked function type mentions one.
    fn named_by(
        &self,
        tables: &mojito_checked::checked::CheckedTables,
        node: CheckedNodeId,
    ) -> bool {
        fn attribute(argument: &CheckedParamArg) -> Option<&TyArg> {
            match argument {
                CheckedParamArg::Attribute { value, .. } => value.as_ref(),
                CheckedParamArg::Named { value, .. } => attribute(value),
                CheckedParamArg::Value(_) => None,
            }
        }
        let mut names: Vec<String> = Vec::new();
        let mut resolved = false;
        tables.walk_expression(node, &mut |walked| {
            let Walked::Expression(expression) = walked else {
                return WalkResult::Advance;
            };
            if let CheckedOp::Identifier(name) | CheckedOp::Call { name, .. } = &expression.op {
                names.push(name.clone());
            }
            resolved |= match &expression.op {
                CheckedOp::TypeValue { ty, .. } => {
                    ty.as_ref().is_some_and(|ty| self.mentioned_in(ty))
                }
                CheckedOp::Lambda { .. } => expression
                    .ty
                    .as_ref()
                    .is_some_and(|ty| self.mentioned_in(ty)),
                CheckedOp::Call { param_args, .. }
                | CheckedOp::Invoke { param_args, .. }
                | CheckedOp::TypeApply {
                    args: param_args, ..
                } => param_args
                    .iter()
                    .filter_map(attribute)
                    .any(|argument| match argument {
                        TyArg::Ty(ty) => self.mentioned_in(ty),
                        TyArg::Val(value) => self.mentioned_in_value(value),
                        TyArg::Origin(_) => false,
                    }),
                _ => false,
            };
            WalkResult::Advance
        });
        resolved
            || names.iter().any(|name| {
                name == "Self"
                    || self
                        .types
                        .iter()
                        .chain(self.values.iter().map(|(binder, _)| binder))
                        .any(|binder| binder.name.as_ref() == name)
            })
    }

    /// Whether resolved type `ty` mentions one of these binders.
    fn mentioned_in(&self, ty: &Ty) -> bool {
        let mut referenced = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut referenced);
        mojito_types::types::mentions(
            ty,
            &|ty| matches!(ty.kind(), TyKind::Param { binder, .. } if self.types.contains(binder)),
        ) || self.names_any(&referenced)
    }

    /// Whether resolved compile-time value `value` reads one of these
    /// binders.
    fn mentioned_in_value(&self, value: &CtValue) -> bool {
        match value {
            CtValue::Expr(expr) => {
                let mut referenced = HashSet::new();
                expr.referenced_parameters(&mut referenced);
                self.names_any(&referenced)
            }
            CtValue::Type(ty) => self.mentioned_in(ty),
            _ => false,
        }
    }

    /// Whether any of `names` spells one of these binders.
    fn names_any(&self, names: &HashSet<String>) -> bool {
        self.types
            .iter()
            .chain(self.values.iter().map(|(binder, _)| binder))
            .any(|binder| names.contains(binder.name.as_ref()))
    }

    /// `expr` when it is the reference to one of these value binders.
    fn binder_value(&self, expr: &ParamExpr) -> Option<ParamExpr> {
        let mojito_types::param_expr::ParamKind::DeclRef(reference) = expr.kind() else {
            return None;
        };
        self.values
            .iter()
            .any(|(binder, _)| binder == reference)
            .then(|| expr.clone())
    }

    /// The expression the local `comptime` binding `owner` denotes.
    fn comptime_value(&self, owner: mojito_types::origin::OwnerId) -> Option<ParamExpr> {
        comptime_binding_value(&self.comptime_bindings, owner)
    }

    /// The reference to `binder`, one of these value binders.
    fn value_of(&self, binder: &mojito_types::param_expr::ParamRef) -> Option<ParamExpr> {
        let (_, ty) = self
            .values
            .iter()
            .find(|(candidate, _)| candidate.id == binder.id)?;
        Some(ParamContext::detached().decl_ref(
            binder.id.clone(),
            &binder.name,
            mojito_types::param_expr::MetaTy::value(ty.clone()),
        ))
    }

    /// The reference a value binder's spelling denotes: `Self.n` names the
    /// struct's, declared first; a bare `n` the innermost declaration's.
    fn value(&self, name: &str, of_self: bool) -> Option<ParamExpr> {
        let mut binders = self.values.iter();
        let named =
            |(binder, _): &&(mojito_types::param_expr::ParamRef, Ty)| binder.name.as_ref() == name;
        let (binder, ty) = if of_self {
            binders.find(named)
        } else {
            binders.rfind(named)
        }?;
        Some(ParamContext::detached().decl_ref(
            binder.id.clone(),
            name,
            mojito_types::param_expr::MetaTy::value(ty.clone()),
        ))
    }
}

mod nested;

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use nested::*;
use node::Node;

/// Whether a type-shaped bracket argument resolved to an origin.
const fn resolved_origin(value: Option<&TyArg>) -> bool {
    matches!(value, Some(TyArg::Origin(_)))
}

fn sorted_map<K: Ord + serde::Serialize, V: serde::Serialize, S: serde::Serializer>(
    map: &HashMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &map.iter().collect::<std::collections::BTreeMap<_, _>>(),
        serializer,
    )
}
