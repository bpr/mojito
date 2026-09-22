//! Re-selection of a struct construction's constructor for an instance.
//!
//! A construction records no contract: `infer_construction` selects an
//! `__init__` from the argument types, retargets it to the instance's
//! constructor clone where one exists, and reaches the struct application.
//! The grammar admits a construction whose arguments are closed scalars,
//! whole values, or the `copy:` of a named place, so the selected member
//! binds every argument exactly under every instance, and what is left to
//! decide per instance is the clone target ([`Checker::realize_construction`],
//! obligation 18 of `realize_instance_facts`).

use super::{Occurrence, fact_at};
use crate::checker::{Checker, MethodSig, StructInfo};
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::templates::{CheckedBodyFacts, OccurrenceId};
use mojito_types::types::{ParamDecl, Ty, TyArg, TySubst, substitute};
use std::collections::HashMap;

impl Checker {
    /// Realize one construction for an instance, as `infer_construction`
    /// decides it on the substituted constructed type.
    ///
    /// A `copy:` construction and a fieldwise one select nothing and record no
    /// target. A hand-written constructor family keeps the template's
    /// member, whose parameters the arguments' types still match exactly
    /// under the struct's own substitution, and takes the instance's clone of
    /// that member as its target (`constructor_clone_target`), the template's
    /// spelling where the instance mints none. A family the instance
    /// collapses, a constructor with binders of its own, and an argument
    /// whose type no longer matches its parameter refuse.
    pub(super) fn realize_construction(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
        substitution: &TySubst,
    ) -> Result<(), &'static str> {
        let occurrence = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .filter(|occurrence| occurrence.callee.is_some())
            .ok_or("a construction is not a direct call in the instance")?;
        let Some(Ty::Struct(name, arguments)) = fact_at(&facts.expression_types, id) else {
            return Err("a construction's recorded type is not a struct");
        };
        if occurrence.callee.as_deref() != Some(name.as_str()) {
            return Err("a construction's recorded type is not the struct it names");
        }
        let (name, arguments) = (name.clone(), arguments.clone());
        let info = self
            .structs
            .get(&name)
            .ok_or("a constructed struct is not declared")?;
        let copied = occurrence.arguments.is_empty()
            && matches!(occurrence.keywords.as_slice(), [(keyword, _)] if keyword == "copy");
        if copied {
            return Ok(());
        }
        let struct_substitution = struct_substitution(info, &arguments)?;
        let at = |syntax| OccurrenceId {
            syntax,
            copy: id.copy,
        };
        let argument_ty = |syntax| {
            fact_at(&facts.expression_types, at(syntax))
                .cloned()
                .ok_or("a construction's argument has no retained type")
        };
        let positional = occurrence
            .arguments
            .iter()
            .map(|syntax| argument_ty(*syntax))
            .collect::<Result<Vec<_>, _>>()?;
        let keywords = occurrence
            .keywords
            .iter()
            .map(|(keyword, syntax)| argument_ty(*syntax).map(|ty| (keyword.as_str(), ty)))
            .collect::<Result<Vec<_>, _>>()?;
        let Some(family) = info.methods.get("__init__") else {
            return realize_fieldwise(info, &struct_substitution, &positional, &keywords);
        };
        let recorded = fact_at(&facts.overload_targets, id).cloned();
        let self_ty = self.self_instance_ty(&name);
        let declared = match family.as_slice() {
            [only] => only,
            members => {
                let Some(recorded) = &recorded else {
                    return Err("an overloaded constructor recorded no selection");
                };
                match members.iter().find(|member| {
                    crate::checker::overload_support::method_lowered_name(
                        &name,
                        "__init__",
                        member,
                        self_ty.as_ref(),
                    ) == *recorded
                }) {
                    Some(member) => member,
                    // A struct closed already in the template selected its
                    // clone there, on the arguments a clone check ranks too.
                    None if self
                        .instance_method_clone(&name, "__init__", &arguments)
                        .is_some_and(|clone| super::names_method(recorded, &name, &clone)) =>
                    {
                        return Ok(());
                    }
                    None => return Err("a construction's selected overload is not declared"),
                }
            }
        };
        if !declared.decls.is_empty() {
            return Err("a constructor has binders of its own");
        }
        let closed_family = family.iter().all(|member| {
            member
                .params
                .iter()
                .chain(member.variadic.as_deref())
                .all(|ty| !mojito_types::types::is_symbolic(ty))
        });
        if !closed_family && !exact_binding(declared, &struct_substitution, &positional, &keywords)?
        {
            return Err("a constructor overload declares a parameter of a parameter type");
        }
        let target =
            self.constructor_clone_target(&name, &arguments, declared, &struct_substitution);
        match target {
            Some(target) => match facts
                .overload_targets
                .iter_mut()
                .find(|(site, _)| *site == id)
            {
                Some(entry) => entry.1 = target,
                None => facts.overload_targets.push((id, target)),
            },
            None if self
                .instance_method_clone(&name, "__init__", &arguments)
                .is_some() =>
            {
                return Err("a constructor's clone family has no member for the selected overload");
            }
            // No clone of the constructor: the template's spelling stands, an
            // overloaded member's lowered name or nothing at all. Its `where`
            // clause, which a clone would have met to exist, is owed here.
            None => {
                let bound: HashMap<String, TyArg> = info
                    .decls
                    .iter()
                    .zip(&arguments)
                    .map(|(decl, argument)| {
                        (
                            decl.name().trim_start_matches('*').to_string(),
                            argument.clone(),
                        )
                    })
                    .collect();
                if !substitution.is_empty() && !self.method_constraints_apply(declared, &bound) {
                    return Err("a constructor's availability condition fails for the instance");
                }
            }
        }
        Ok(())
    }
}

/// The struct's own type binders bound to the constructed type's arguments,
/// the scope its constructor's parameter types are declared in.
fn struct_substitution(info: &StructInfo, arguments: &[TyArg]) -> Result<TySubst, &'static str> {
    info.decls
        .iter()
        .zip(arguments)
        .map(|(decl, argument)| match (decl, argument) {
            (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => Ok((id.clone(), ty.clone())),
            _ => Err("a constructed struct has a parameter that is not a plain type"),
        })
        .collect()
}

/// A fieldwise construction: each argument initializes the field at its
/// position, whose declared type it must still equal under the instance. A
/// reference field takes a handle, whose recorded type is the referent.
fn realize_fieldwise(
    info: &StructInfo,
    struct_substitution: &TySubst,
    positional: &[Ty],
    keywords: &[(&str, Ty)],
) -> Result<(), &'static str> {
    if !info.fieldwise_init || !keywords.is_empty() || positional.len() != info.fields.len() {
        return Err("a construction matches no constructor of its struct");
    }
    let exact = positional
        .iter()
        .zip(&info.fields)
        .all(
            |(argument, (_, field))| match substitute(field, struct_substitution) {
                Ty::Ref(reference) => *argument == *reference.referent,
                field => binds(argument, &field),
            },
        );
    if !exact {
        return Err("a construction's argument does not match its field for the instance");
    }
    Ok(())
}

/// Whether every argument's type is exactly its parameter's under the struct's
/// substitution, so no other member of the family can outrank the selected
/// one. A parameter left to its default must have a closed type: the default
/// is evaluated in the callee's scope, once, whatever the instance.
fn exact_binding(
    declared: &MethodSig,
    struct_substitution: &TySubst,
    positional: &[Ty],
    keywords: &[(&str, Ty)],
) -> Result<bool, &'static str> {
    let keyword_names: Vec<&str> = keywords.iter().map(|(name, _)| *name).collect();
    let slots = match_call_slots(
        &declared.names,
        &declared.required,
        declared.positional_only,
        declared.keyword_only,
        positional.len(),
        &keyword_names,
        CallVariadics {
            positional: declared.variadic.is_some(),
            keyword: declared.kw_variadic.is_some(),
        },
    )
    .map_err(|_| "a construction's arguments do not bind its constructor's parameters")?;
    if !slots.positional_overflow.is_empty() || !slots.keyword_overflow.is_empty() {
        return Err("a construction binds a variadic constructor parameter");
    }
    Ok(slots
        .slots
        .iter()
        .zip(&declared.params)
        .all(|(slot, parameter)| {
            let parameter = substitute(parameter, struct_substitution);
            match slot {
                ArgSlot::Positional(position) => binds(&positional[*position], &parameter),
                ArgSlot::Keyword(position) => binds(&keywords[*position].1, &parameter),
                ArgSlot::Default => !mojito_types::types::is_symbolic(&parameter),
            }
        }))
}

/// Whether an argument of the recorded type binds a parameter of `parameter`'s
/// type exactly: the same type, or a literal materializing to a closed one,
/// which the template recorded and an instance derives
/// (`SemanticAdjustment::MaterializeLiteral`).
fn binds(argument: &Ty, parameter: &Ty) -> bool {
    argument == parameter
        || (matches!(argument, Ty::IntLiteral | Ty::FloatLiteral)
            && !mojito_types::types::is_symbolic(parameter)
            && mojito_types::types::coerces(argument, parameter))
}
