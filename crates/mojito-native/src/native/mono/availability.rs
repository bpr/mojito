//! Whether a conditional member belongs to an instance.
//!
//! A member's `where` clauses reach MIR on its declaration, and each struct
//! carries its declared conformances and its synthesized lifecycle rows.
//! The elaborator decides a clause under an instance's bindings from those
//! and the trait declarations' parents when the clause asks it, as upstream
//! decides a conformance from a type's declared conformances and the trait
//! refinement graph: it selects nothing and ranks nothing.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::StructConformance;
use mojito_types::conformance::{StructTraitRule, compiler_trait_refines, struct_trait_rule};
use mojito_types::param_expr::fold::integer_value;
use mojito_types::types::TyKind;
use mojito_types::types::{
    ConstraintOperand, GenericConstraint, PackPredicateRef, TrivialLifecycle,
    is_uninit_storage_name, trivial_predicate_spelling, uninit_storage_element,
};

/// The verdict of a member's availability clauses under an instance's
/// bindings.
pub(super) enum Availability {
    Proven,
    /// A clause does not hold, with the message it declares.
    Disproven(Option<String>),
    /// A clause these bindings do not close: an operand left symbolic, a
    /// pack predicate through an alias, or a struct with no lifecycle row
    /// for the trait.
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

    /// `rows`, a template's conformance rows or declared conformances, for
    /// the instance `bindings` describe, each decided. A row these bindings
    /// do not decide is left out.
    pub(super) fn instance_conformances(
        &self,
        rows: &[StructConformance],
        bindings: &Bindings,
    ) -> Vec<StructConformance> {
        rows.iter()
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

    pub(super) fn constraint_holds(
        &self,
        constraint: &GenericConstraint,
        bindings: &Bindings,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
        match constraint {
            WithMessage(condition, _) => self.constraint_holds(condition, bindings, visiting),
            Bool(value) => Some(*value),
            Not(condition) => self
                .constraint_holds(condition, bindings, visiting)
                .map(|holds| !holds),
            And(left, right) => every([
                self.constraint_holds(left, bindings, visiting),
                self.constraint_holds(right, bindings, visiting),
            ]),
            Or(left, right) => some([
                self.constraint_holds(left, bindings, visiting),
                self.constraint_holds(right, bindings, visiting),
            ]),
            Conforms {
                operand,
                trait_name,
            } => match operand_value(operand, bindings)? {
                TyArg::Ty(ty) => self.type_conforms(&ty, trait_name, visiting),
                TyArg::Val(_) | TyArg::Origin(_) => Some(false),
            },
            ConformsPack {
                operand,
                trait_name,
            } => every(
                pack_operand_types(operand, bindings)?
                    .iter()
                    .map(|ty| self.type_conforms(ty, trait_name, visiting)),
            ),
            // A predicate alias's body is the checker's; it does not reach
            // MIR.
            PackPredicate {
                param,
                predicate: PackPredicateRef::Trivial(kind),
                all,
            } => {
                let elements = pack_types(param, bindings)?;
                let verdicts = elements
                    .iter()
                    .map(|ty| self.trivially(*kind, ty, visiting));
                if *all {
                    every(verdicts)
                } else {
                    some(verdicts)
                }
            }
            PackPredicate { .. } => None,
            PackContains { param, element } => match operand_value(element, bindings)? {
                TyArg::Ty(needle) => Some(
                    pack_types(param, bindings)?
                        .iter()
                        .any(|ty| ty_equal_modulo_origins(ty, &needle)),
                ),
                TyArg::Val(_) | TyArg::Origin(_) => Some(false),
            },
            Trivial(kind, operand) => match operand_value(operand, bindings)? {
                TyArg::Ty(ty) => self.trivially(*kind, &ty, visiting),
                TyArg::Val(_) | TyArg::Origin(_) => Some(false),
            },
            Eq(left, right) | Ne(left, right) => {
                let equal = match (
                    operand_value(left, bindings)?,
                    operand_value(right, bindings)?,
                ) {
                    (TyArg::Val(left), TyArg::Val(right)) => {
                        match (integer_value(&left), integer_value(&right)) {
                            (Some(left), Some(right)) => left == right,
                            _ => left == right,
                        }
                    }
                    (TyArg::Ty(left), TyArg::Ty(right)) => ty_equal_modulo_origins(&left, &right),
                    _ => false,
                };
                Some(equal == matches!(constraint, Eq(..)))
            }
            Lt(left, right) | Le(left, right) | Gt(left, right) | Ge(left, right) => {
                let (TyArg::Val(left), TyArg::Val(right)) = (
                    operand_value(left, bindings)?,
                    operand_value(right, bindings)?,
                ) else {
                    return Some(false);
                };
                let (Some(left), Some(right)) = (integer_value(&left), integer_value(&right))
                else {
                    return Some(false);
                };
                Some(match constraint {
                    Lt(..) => left < right,
                    Le(..) => left <= right,
                    Gt(..) => left > right,
                    _ => left >= right,
                })
            }
        }
    }

    /// `IsTrivially*[ty]` for a concrete `ty`, as the checker's
    /// `is_trivially` answers it: a struct by its row, an aggregate by its
    /// elements, the inline uninit storage by its payload, and any other type
    /// by its shape.
    fn trivially(
        &self,
        kind: TrivialLifecycle,
        ty: &Ty,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let capability = match kind {
            TrivialLifecycle::Movable => "Movable",
            TrivialLifecycle::Copyable => "Copyable",
            TrivialLifecycle::Deinitable => "Deinitable",
        };
        if let Some(payload) = uninit_storage_element(ty) {
            // The storage never runs its payload's destructor.
            return match kind {
                TrivialLifecycle::Deinitable => Some(true),
                TrivialLifecycle::Movable | TrivialLifecycle::Copyable => {
                    self.trivially(kind, payload, visiting)
                }
            };
        }
        match ty.kind() {
            TyKind::Struct(..) => {
                self.type_conforms(ty, trivial_predicate_spelling(kind), visiting)
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
                every(
                    elements
                        .iter()
                        .map(|element| self.trivially(kind, element, visiting)),
                )
            }
            TyKind::ComptimeList(element) | TyKind::PopArray { element, .. } => {
                self.trivially(kind, element, visiting)
            }
            _ if is_symbolic(ty) => None,
            _ => self.type_conforms(ty, capability, visiting),
        }
    }

    /// Whether any of a conformance row's conditions holds.
    fn any_holds<'c>(
        &self,
        conditions: impl IntoIterator<Item = &'c GenericConstraint>,
        bindings: &Bindings,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        some(
            conditions
                .into_iter()
                .map(|condition| self.constraint_holds(condition, bindings, visiting)),
        )
    }

    /// Whether the concrete type `ty` conforms to `trait_name`: a struct by
    /// its template's declaration under the instance's arguments, by
    /// `mojito_types::conformance::struct_trait_rule`, any other type by its
    /// shape.
    pub(super) fn type_conforms(
        &self,
        ty: &Ty,
        trait_name: &str,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let key = (ty.clone(), trait_name.to_string());
        if let Some(answer) = self.conformance_answers.borrow().get(&key) {
            return *answer;
        }
        // Only an answer no enclosing conformance's cycle cut shaped is
        // every asker's answer.
        let unconditional = visiting.is_empty();
        let answer = self.decide_type_conforms(ty, trait_name, visiting);
        if unconditional {
            self.conformance_answers.borrow_mut().insert(key, answer);
        }
        answer
    }

    /// Whether the concrete type `ty` conforms to `trait_name`, decided
    /// afresh; see [`Specializer::type_conforms`].
    fn decide_type_conforms(
        &self,
        ty: &Ty,
        trait_name: &str,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let declarations = &self.source.declarations;
        let TyKind::Struct(name, arguments) = ty.kind() else {
            if declarations.declares_trait(trait_name) {
                // A compiler-known type conforms to a declared trait only as
                // the upstream struct it models declares.
                return mojito_types::conformance::leaf_conforms(ty, trait_name, &mut |_, _| true)
                    .map(|_| mojito_types::conformance::modelled_declares(ty, trait_name));
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
        // The inline uninit storage answers by its elements, which its
        // declaration does not say.
        if is_uninit_storage_name(name) {
            return None;
        }
        let template = self.structs.get(nominal_template(name))?;
        let conditions: Vec<&GenericConstraint> =
            match struct_trait_rule(trait_name, declarations.declares_trait(trait_name)) {
                StructTraitRule::Lifecycle => template
                    .conformances
                    .iter()
                    .find(|row| row.trait_name == trait_name)?
                    .conditions
                    .iter()
                    .collect(),
                StructTraitRule::Declared => template
                    .declared
                    .iter()
                    .filter(|declared| {
                        declared.trait_name == trait_name
                            || declarations.trait_refines(&declared.trait_name, trait_name)
                            || compiler_trait_refines(&declared.trait_name, trait_name)
                    })
                    .flat_map(|declared| &declared.conditions)
                    .collect(),
                StructTraitRule::Never => return Some(false),
                StructTraitRule::Always => return Some(true),
            };
        let mut bindings = self.base_bindings();
        bind_struct_arguments(&template.param_decls, arguments, &mut bindings)?;
        // A conformance that depends on itself proves nothing.
        let key = (name.clone(), trait_name.to_string());
        if !visiting.insert(key.clone()) {
            return Some(false);
        }
        let holds = self.any_holds(conditions, &bindings, visiting);
        visiting.remove(&key);
        holds
    }
}

/// Every verdict holds: one disproof decides, and otherwise one undecided
/// verdict leaves the whole undecided.
fn every(verdicts: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut all = Some(true);
    for verdict in verdicts {
        match verdict {
            Some(true) => {}
            Some(false) => return Some(false),
            None => all = None,
        }
    }
    all
}

/// The concrete type or value a clause operand denotes under `bindings`, or
/// `None` while it stays symbolic. A type keeps its template spelling, so two
/// operands compare as the checker compares them.
fn operand_value(operand: &ConstraintOperand, bindings: &Bindings) -> Option<TyArg> {
    match operand {
        ConstraintOperand::Param(param) => bindings
            .types
            .get(param)
            .cloned()
            .map(TyArg::Ty)
            .or_else(|| bindings.values.get(param).cloned().map(TyArg::Val)),
        ConstraintOperand::Value(CtValue::Expr(expression))
        | ConstraintOperand::Expr(expression) => eval_ct(expression, bindings).ok().map(TyArg::Val),
        ConstraintOperand::Value(value) => Some(TyArg::Val(value.clone())),
        ConstraintOperand::Type(ty) if !is_symbolic(ty) => Some(TyArg::Ty(ty.clone())),
        ConstraintOperand::Type(ty) => {
            let spelled = Bindings {
                types: bindings.types.clone(),
                values: bindings.values.clone(),
                associated: bindings.associated.clone(),
                structs: Rc::clone(&bindings.structs),
                applications: bindings.applications.clone(),
                ..Bindings::default()
            };
            substitute_ty(ty, &spelled)
                .ok()
                .filter(|ty| !is_symbolic(ty))
                .map(TyArg::Ty)
        }
        ConstraintOperand::PackLength(param) => {
            let length = i64::try_from(pack_types(param, bindings)?.len()).ok()?;
            Some(TyArg::Val(CtValue::Int(length)))
        }
    }
}

/// The element types a pack binder is bound to.
fn pack_types(param: &ParamRef, bindings: &Bindings) -> Option<Vec<Ty>> {
    type_list(bindings.values.get(param)?)
}

/// The element types of a pack operand: a pack binder's bound list (its
/// value binding, not the runtime pack its type binding is), or a list the
/// operand itself holds.
fn pack_operand_types(operand: &ConstraintOperand, bindings: &Bindings) -> Option<Vec<Ty>> {
    match operand {
        ConstraintOperand::Param(param) => pack_types(param, bindings),
        operand => match operand_value(operand, bindings)? {
            TyArg::Val(list) => type_list(&list),
            TyArg::Ty(_) | TyArg::Origin(_) => None,
        },
    }
}

fn type_list(value: &CtValue) -> Option<Vec<Ty>> {
    let CtValue::Tuple(elements) = value else {
        return None;
    };
    elements
        .iter()
        .map(|element| match element {
            CtValue::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .collect()
}

/// Some verdict holds: one proof decides, and otherwise one undecided
/// verdict leaves the whole undecided.
fn some(verdicts: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    every(
        verdicts
            .into_iter()
            .map(|verdict| verdict.map(|holds| !holds)),
    )
    .map(|none| !none)
}

fn clause_message(clause: &GenericConstraint) -> Option<String> {
    match clause {
        GenericConstraint::WithMessage(_, message) => Some(message.clone()),
        _ => None,
    }
}

/// Bind a struct's parameters to an application's arguments. A trailing
/// variadic type parameter takes the remaining type arguments as its pack.
fn bind_struct_arguments(
    decls: &[ParamDecl],
    arguments: &[TyArg],
    bindings: &mut Bindings,
) -> Option<()> {
    let arguments: Vec<TyArg> = arguments
        .iter()
        .filter(|argument| !matches!(argument, TyArg::Origin(_)))
        .cloned()
        .collect();
    match decls.split_last() {
        Some((pack @ ParamDecl::Type { variadic: true, .. }, leading))
            if arguments.len() >= leading.len() =>
        {
            let (prefix, rest) = arguments.split_at(leading.len());
            bind_ty_args(leading, prefix, bindings).ok()?;
            let elements = match rest {
                [TyArg::Val(CtValue::Tuple(packed))] => packed
                    .iter()
                    .map(|element| match element {
                        CtValue::Type(ty) => Some(ty.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?,
                rest => rest
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => Some(ty.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?,
            };
            bind_pack(&pack.binder(), elements, bindings);
            Some(())
        }
        _ if arguments.len() < decls.len() => None,
        _ => bind_ty_args(decls, &arguments, bindings).ok(),
    }
}
