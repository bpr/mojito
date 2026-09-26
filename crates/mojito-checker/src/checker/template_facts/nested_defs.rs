//! Re-derivation of a nested `def`'s declaration facts for an instance.
//!
//! Checking a nested `def` records facts keyed by the statement's own
//! identity rather than by an occurrence: its parameter, return, and callable
//! types (`declaration_types`), its compile-time parameters
//! (`generic_parameters`), its declared effect (`declaration_effects`), and
//! each parameter's deletability. Its capture list (`declaration_captures`)
//! is keyed by the statement, and names each captured binding by checker
//! identity. The recipe (`TemplateNestedDef`) keeps all of them at the
//! statement's occurrence, every binding by template owner, and an instance
//! writes them again under its own statement and bindings
//! ([`Checker::install_nested_defs`]), judging each parameter's deletability
//! at its own type as the declaration's check does (`check_def_inner`).
//!
//! A call of a capturing nested `def` records the places its environment
//! reads and writes (`CallableCaptureAccesses`); those are rooted at the
//! captured bindings, so the bundle keeps them apart by template place.

use super::{
    Occurrence, UNKEYED_STORES, bind_struct_origins, checked_origin, names_place, template_origin,
    unbound_struct_origins,
};
use crate::checker::Checker;
use mojito_ast::ast::{Stmt, StmtKind};
use mojito_checked::checked::{AnnotationSite, CheckedCapture, GenericSite, SemanticAdjustment};
use mojito_checked::templates::{
    CheckedBodyFacts, IncompleteReason, OccurrenceId, TemplateCapture, TemplateCaptureOrigin,
    TemplateNestedDef, TemplateOwner, TemplatePlace,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::origin::{CaptureOrigin, OriginPlace, OwnerId};
use mojito_types::types::Ty;

/// A body's nested `def` recipes, and the capture accesses of their calls.
type NestedDefRecipes = (
    Vec<(OccurrenceId, TemplateNestedDef)>,
    Vec<(OccurrenceId, Vec<TemplateCaptureOrigin>)>,
);

impl Checker {
    /// How many entries of each store `unkeyed_fact_entries` watches are
    /// keyed by one of `body`'s nested `def` statements, which the recipe
    /// accounts for.
    pub(super) fn nested_def_entries(&self, body: &[Stmt]) -> [usize; UNKEYED_STORES] {
        let sites: Vec<SourceSpan> = nested_defs(body)
            .into_iter()
            .map(|(site, _, _)| site)
            .collect();
        if sites.is_empty() {
            return [0; UNKEYED_STORES];
        }
        let annotated = |site: &AnnotationSite| match site {
            AnnotationSite::FunctionParam {
                module,
                declaration,
                syntax,
                ..
            }
            | AnnotationSite::FunctionReturn {
                module,
                declaration,
                syntax,
            }
            | AnnotationSite::FunctionType {
                module,
                declaration,
                syntax,
            } => sites.contains(&SourceSpan::syntax(module.clone(), *declaration, *syntax)),
            _ => false,
        };
        let generic = |site: &GenericSite| match site {
            GenericSite::Function {
                module,
                declaration,
                syntax,
            } => sites.contains(&SourceSpan::syntax(module.clone(), *declaration, *syntax)),
            GenericSite::Struct { .. } | GenericSite::Method { .. } => false,
        };
        let deletability = self.explicit_destroy_deletability.borrow();
        [
            self.declaration_types
                .borrow()
                .keys()
                .filter(|site| annotated(site))
                .count(),
            self.generic_parameters
                .borrow()
                .keys()
                .filter(|site| generic(site))
                .count(),
            self.declaration_effects
                .borrow()
                .keys()
                .filter(|site| annotated(site))
                .count(),
            0,
            deletability
                .declarations
                .iter()
                .filter(|site| annotated(site))
                .count(),
            deletability
                .linear_declarations
                .iter()
                .filter(|site| annotated(site))
                .count(),
        ]
    }

    /// The recipe of each nested `def` the body declares, keyed by its
    /// statement's occurrence, and the captured places each call of a
    /// capturing one reaches, by template place.
    ///
    /// Capture reads every fact the declaration's check recorded under the
    /// statement's identity. It declares no compile-time parameters, its
    /// types name no place but its callable's environment, which is kept by
    /// template place, and each capture's binding and origins are the
    /// body's own.
    pub(super) fn captured_nested_defs(
        &self,
        body: &[Stmt],
        occurrences: &[Occurrence],
        local_owner: &dyn Fn(OwnerId) -> Result<TemplateOwner, IncompleteReason>,
        local_place: &dyn Fn(&OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
    ) -> Result<NestedDefRecipes, IncompleteReason> {
        let recipes = nested_defs(body)
            .into_iter()
            .map(|(site, name, arity)| {
                let id = occurrences
                    .iter()
                    .find(|occurrence| occurrence.span == site)
                    .map(|occurrence| occurrence.id)
                    .ok_or(IncompleteReason::NestedDefRecipe)?;
                self.captured_nested_def(&site, name, arity, local_owner, local_place)
                    .map(|recipe| (id, recipe))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let adjustments = self.operation_adjustments.borrow();
        let accesses = occurrences
            .iter()
            .filter_map(|occurrence| match adjustments.get(&occurrence.span) {
                Some(SemanticAdjustment::CallableCaptureAccesses(accesses)) => Some(
                    template_capture_origins(accesses, local_place)
                        .map(|accesses| (occurrence.id, accesses)),
                ),
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((recipes, accesses))
    }

    /// Substitute each nested `def`'s types.
    pub(super) fn realize_nested_defs(
        facts: &mut CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) {
        for (_, recipe) in &mut facts.nested_defs {
            for ty in &mut recipe.param_types {
                *ty = substitute(ty);
            }
            recipe.return_ty = substitute(&recipe.return_ty);
            recipe.function_ty = substitute(&recipe.function_ty);
            for capture in &mut recipe.captures {
                capture.ty = substitute(&capture.ty);
            }
        }
    }

    /// Write each nested `def`'s declaration facts under the instance's own
    /// statement, with every binding the instance's own, and each call's
    /// capture accesses at the call.
    pub(super) fn install_nested_defs(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        owner: &dyn Fn(&TemplateOwner) -> Result<OwnerId, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<OriginPlace, TypeError>,
    ) -> Result<(), TypeError> {
        let lost = || {
            TypeError::InvariantViolation(
                "template derivation lost a nested def's statement identity".to_string(),
            )
        };
        for (id, recipe) in &facts.nested_defs {
            let site = span(id)?;
            let syntax = site.syntax.ok_or_else(lost)?;
            let (module, declaration) = (site.source.clone(), site.span);
            let parameter = |param| AnnotationSite::FunctionParam {
                module: module.clone(),
                declaration,
                syntax,
                param,
            };
            let returned = AnnotationSite::FunctionReturn {
                module: module.clone(),
                declaration,
                syntax,
            };
            {
                let mut types = self.declaration_types.borrow_mut();
                for (param, ty) in recipe.param_types.iter().enumerate() {
                    types.insert(parameter(param), ty.clone());
                }
                types.insert(returned.clone(), recipe.return_ty.clone());
                types.insert(
                    AnnotationSite::FunctionType {
                        module: module.clone(),
                        declaration,
                        syntax,
                    },
                    bind_struct_origins(&recipe.function_ty, &recipe.function_origins, rooted)?,
                );
            }
            self.generic_parameters.borrow_mut().insert(
                GenericSite::Function {
                    module: module.clone(),
                    declaration,
                    syntax,
                },
                Vec::new(),
            );
            self.declaration_effects
                .borrow_mut()
                .insert(returned, recipe.effect.clone());
            for (param, ty) in recipe.param_types.iter().enumerate() {
                let deinitable = self.is_deinitable(ty);
                let mut deletability = self.explicit_destroy_deletability.borrow_mut();
                if matches!(ty, Ty::Param { .. }) && !deinitable {
                    deletability.linear_declarations.insert(parameter(param));
                } else if deinitable {
                    deletability.declarations.insert(parameter(param));
                }
            }
            let captures = recipe
                .captures
                .iter()
                .map(|capture| {
                    Ok(CheckedCapture {
                        name: capture.name.clone(),
                        binding: owner(&capture.owner)?,
                        ty: capture.ty.clone(),
                        kind: capture.kind,
                        origins: checked_capture_origins(&capture.origins, rooted)?,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            self.declaration_captures
                .borrow_mut()
                .insert(site.clone(), captures);
            let params = recipe
                .params
                .iter()
                .map(owner)
                .collect::<Result<Vec<_>, _>>()?;
            self.nested_def_params.borrow_mut().insert(site, params);
        }
        for (id, accesses) in &facts.capture_accesses {
            self.operation_adjustments.borrow_mut().insert(
                span(id)?,
                SemanticAdjustment::CallableCaptureAccesses(checked_capture_origins(
                    accesses, rooted,
                )?),
            );
        }
        Ok(())
    }

    fn captured_nested_def(
        &self,
        site: &SourceSpan,
        name: String,
        arity: usize,
        local_owner: &dyn Fn(OwnerId) -> Result<TemplateOwner, IncompleteReason>,
        local_place: &dyn Fn(&OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
    ) -> Result<TemplateNestedDef, IncompleteReason> {
        let (module, declaration) = (site.source.clone(), site.span);
        let syntax = site.syntax.ok_or(IncompleteReason::NestedDefRecipe)?;
        let returned = AnnotationSite::FunctionReturn {
            module: module.clone(),
            declaration,
            syntax,
        };
        let types = self.declaration_types.borrow();
        let declared = |site: &AnnotationSite| {
            types
                .get(site)
                .cloned()
                .ok_or(IncompleteReason::NestedDefRecipe)
        };
        let param_types = (0..arity)
            .map(|param| {
                declared(&AnnotationSite::FunctionParam {
                    module: module.clone(),
                    declaration,
                    syntax,
                    param,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = declared(&returned)?;
        if param_types.iter().chain([&return_ty]).any(names_place) {
            return Err(IncompleteReason::ExternalBinding);
        }
        let callable = declared(&AnnotationSite::FunctionType {
            module: module.clone(),
            declaration,
            syntax,
        })?;
        let (function_ty, function_origins) =
            unbound_struct_origins(&callable, local_place)?.unwrap_or((callable, Vec::new()));
        let generic = self
            .generic_parameters
            .borrow()
            .get(&GenericSite::Function {
                module,
                declaration,
                syntax,
            })
            .is_some_and(Vec::is_empty);
        let effect = self
            .declaration_effects
            .borrow()
            .get(&returned)
            .cloned()
            .filter(|_| generic)
            .ok_or(IncompleteReason::NestedDefRecipe)?;
        let params = self
            .nested_def_params
            .borrow()
            .get(site)
            .filter(|params| params.len() == arity)
            .ok_or(IncompleteReason::NestedDefRecipe)?
            .iter()
            .map(|param| local_owner(*param))
            .collect::<Result<Vec<_>, _>>()?;
        let captures = self
            .declaration_captures
            .borrow()
            .get(site)
            .ok_or(IncompleteReason::NestedDefRecipe)?
            .iter()
            .map(|capture| {
                if names_place(&capture.ty) {
                    return Err(IncompleteReason::ExternalBinding);
                }
                Ok(TemplateCapture {
                    name: capture.name.clone(),
                    owner: local_owner(capture.binding)?,
                    ty: capture.ty.clone(),
                    kind: capture.kind,
                    origins: template_capture_origins(&capture.origins, local_place)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TemplateNestedDef {
            name,
            params,
            param_types,
            return_ty,
            function_ty,
            function_origins,
            effect,
            captures,
        })
    }
}

/// Each nested `def` statement of `body`, at any depth, with its name and
/// parameter count.
fn nested_defs(body: &[Stmt]) -> Vec<(SourceSpan, String, usize)> {
    struct Defs(Vec<(SourceSpan, String, usize)>);

    impl mojito_ast::visit::Visitor for Defs {
        fn visit_stmt(&mut self, statement: &Stmt) {
            if let StmtKind::Def { name, params, .. } = &statement.kind {
                self.0
                    .push((statement.source_span(), name.clone(), params.len()));
            }
        }
    }

    let mut defs = Defs(Vec::new());
    mojito_ast::visit::walk_block(&mut defs, body);
    defs.0
}

fn template_capture_origins(
    origins: &[CaptureOrigin],
    local_place: &dyn Fn(&OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
) -> Result<Vec<TemplateCaptureOrigin>, IncompleteReason> {
    origins
        .iter()
        .map(|capture| {
            Ok(TemplateCaptureOrigin {
                origin: template_origin(&capture.origin, local_place)?,
                access: capture.access,
            })
        })
        .collect()
}

fn checked_capture_origins(
    origins: &[TemplateCaptureOrigin],
    rooted: &dyn Fn(&TemplatePlace) -> Result<OriginPlace, TypeError>,
) -> Result<Vec<CaptureOrigin>, TypeError> {
    origins
        .iter()
        .map(|capture| {
            Ok(CaptureOrigin {
                origin: checked_origin(&capture.origin, rooted)?,
                access: capture.access,
            })
        })
        .collect()
}
