//! Whether a conditional member belongs to an instance.
//!
//! A member's `where` clauses reach MIR on its declaration, and each struct
//! carries its conformance rows. The elaborator decides a clause under an
//! instance's bindings from those alone: it selects nothing and ranks
//! nothing.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::StructConformance;
use mojito_types::types::GenericConstraint;

/// The verdict of a member's availability clauses under an instance's
/// bindings.
pub(super) enum Availability {
    Proven,
    /// A clause does not hold, with the message it declares.
    Disproven(Option<String>),
    /// A clause in a form these bindings do not decide here: a trivial
    /// lifecycle predicate, a pack predicate, a parameter expression.
    Undecided,
}

impl Specializer<'_> {
    pub(super) fn availability(
        &self,
        declaration: &MirFunctionDeclaration,
        bindings: &Bindings,
    ) -> Availability {
        let mut verdict = Availability::Proven;
        for clause in &declaration.availability {
            match self.constraint_holds(clause, bindings, &mut HashSet::new()) {
                Some(true) => {}
                Some(false) => return Availability::Disproven(clause_message(clause)),
                None => verdict = Availability::Undecided,
            }
        }
        verdict
    }

    /// The rows of the instance of `template` that `bindings` describe, each
    /// decided. A row these bindings do not decide is left out.
    pub(super) fn instance_conformances(
        &self,
        template: &MirStructDeclaration,
        bindings: &Bindings,
    ) -> Vec<StructConformance> {
        template
            .conformances
            .iter()
            .filter_map(|row| {
                let holds = self.any_holds(&row.conditions, bindings, &mut HashSet::new())?;
                Some(StructConformance {
                    trait_name: row.trait_name.clone(),
                    conditions: holds
                        .then_some(GenericConstraint::Bool(true))
                        .into_iter()
                        .collect(),
                })
            })
            .collect()
    }

    fn constraint_holds(
        &self,
        constraint: &GenericConstraint,
        bindings: &Bindings,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        match constraint {
            GenericConstraint::WithMessage(condition, _) => {
                self.constraint_holds(condition, bindings, visiting)
            }
            GenericConstraint::Bool(value) => Some(*value),
            GenericConstraint::Not(condition) => self
                .constraint_holds(condition, bindings, visiting)
                .map(|holds| !holds),
            GenericConstraint::And(left, right) => {
                match (
                    self.constraint_holds(left, bindings, visiting),
                    self.constraint_holds(right, bindings, visiting),
                ) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }
            }
            GenericConstraint::Or(left, right) => {
                match (
                    self.constraint_holds(left, bindings, visiting),
                    self.constraint_holds(right, bindings, visiting),
                ) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }
            }
            GenericConstraint::Conforms { param, trait_name } => {
                self.type_conforms(bindings.types.get(param)?, trait_name, visiting)
            }
            GenericConstraint::ConformsPack { param, trait_name } => {
                let CtValue::Tuple(elements) = bindings.values.get(param)? else {
                    return None;
                };
                let mut all = Some(true);
                for element in elements {
                    let CtValue::Type(ty) = element else {
                        return None;
                    };
                    match self.type_conforms(ty, trait_name, visiting) {
                        Some(true) => {}
                        Some(false) => return Some(false),
                        None => all = None,
                    }
                }
                all
            }
            _ => None,
        }
    }

    /// Whether any of a conformance row's conditions holds.
    fn any_holds(
        &self,
        conditions: &[GenericConstraint],
        bindings: &Bindings,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let mut any = Some(false);
        for condition in conditions {
            match self.constraint_holds(condition, bindings, visiting) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => any = None,
            }
        }
        any
    }

    /// Whether the concrete type `ty` conforms to `trait_name`: a struct by
    /// its template's row under the instance's arguments, any other type by
    /// its shape.
    fn type_conforms(
        &self,
        ty: &Ty,
        trait_name: &str,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let Ty::Struct(name, arguments) = ty else {
            if self
                .source
                .declarations
                .traits
                .iter()
                .any(|declared| declared == trait_name)
            {
                // No compiler-known type conforms to a declared trait.
                return mojito_types::conformance::leaf_conforms(ty, trait_name, &mut |_, _| true)
                    .map(|_| false);
            }
            let mut undecided = false;
            let conforms = mojito_types::conformance::leaf_conforms(
                ty,
                trait_name,
                &mut |element, required| {
                    self.type_conforms(element, required, visiting)
                        .unwrap_or_else(|| {
                            undecided = true;
                            true
                        })
                },
            )?;
            // An undecided element counted as conforming, so only a
            // conforming aggregate is in doubt.
            return (!conforms || !undecided).then_some(conforms);
        };
        let template = self.structs.get(nominal_template(name)).copied()?;
        let row = template
            .conformances
            .iter()
            .find(|row| row.trait_name == trait_name)?;
        if arguments.len() < template.param_decls.len() {
            return None;
        }
        // A conformance that depends on itself proves nothing.
        let key = (name.clone(), trait_name.to_string());
        if !visiting.insert(key.clone()) {
            return Some(false);
        }
        let mut bindings = self.base_bindings();
        let holds = bind_ty_args(&template.param_decls, arguments, &mut bindings)
            .ok()
            .and_then(|()| self.any_holds(&row.conditions, &bindings, visiting));
        visiting.remove(&key);
        holds
    }
}

fn clause_message(clause: &GenericConstraint) -> Option<String> {
    match clause {
        GenericConstraint::WithMessage(_, message) => Some(message.clone()),
        _ => None,
    }
}
