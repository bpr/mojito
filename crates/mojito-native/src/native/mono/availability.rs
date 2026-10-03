//! Whether a conditional member belongs to an instance.
//!
//! A member's `where` clauses reach MIR on its declaration, and each struct
//! carries its conformance rows. The elaborator decides a clause under an
//! instance's bindings from those alone: it selects nothing and ranks
//! nothing.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::StructConformance;
use mojito_types::param_expr::fold::integer_value;
use mojito_types::types::{
    ConstraintOperand, GenericConstraint, PackPredicateRef, TrivialLifecycle,
    trivial_predicate_name, trivial_predicate_spelling, tstring_elements, tuple_elements,
    uninit_storage_element,
};

/// The verdict of a member's availability clauses under an instance's
/// bindings.
pub(super) enum Availability {
    Proven,
    /// A clause does not hold, with the message it declares.
    Disproven(Option<String>),
    /// A clause these bindings do not close: an operand left symbolic, a
    /// pack predicate through an alias, or a struct with no row for the
    /// trait.
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
            Conforms { param, trait_name } => {
                self.type_conforms(bindings.types.get(param)?, trait_name, visiting)
            }
            ConformsPack { param, trait_name } => every(
                pack_types(param, bindings)?
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
        match ty {
            Ty::Struct(..) => self.type_conforms(ty, trivial_predicate_spelling(kind), visiting),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => every(
                elements
                    .iter()
                    .map(|element| self.trivially(kind, element, visiting)),
            ),
            Ty::ComptimeList(element) => self.trivially(kind, element, visiting),
            _ if is_symbolic(ty) => None,
            _ => self.type_conforms(ty, capability, visiting),
        }
    }

    /// Whether any of a conformance row's conditions holds.
    fn any_holds(
        &self,
        conditions: &[GenericConstraint],
        bindings: &Bindings,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        some(
            conditions
                .iter()
                .map(|condition| self.constraint_holds(condition, bindings, visiting)),
        )
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
        let Some(template) = self.structs.get(nominal_template(name)).copied() else {
            return self.elements_conform(ty, trait_name, visiting);
        };
        let row = template
            .conformances
            .iter()
            .find(|row| row.trait_name == trait_name)?;
        let mut bindings = self.base_bindings();
        bind_struct_arguments(&template.param_decls, arguments, &mut bindings)?;
        // A conformance that depends on itself proves nothing.
        let key = (name.clone(), trait_name.to_string());
        if !visiting.insert(key.clone()) {
            return Some(false);
        }
        let holds = self.any_holds(&row.conditions, &bindings, visiting);
        visiting.remove(&key);
        holds
    }

    /// A variadic `Tuple` or `TString` whose specialization does not exist
    /// answers by its elements, as the checker's `conforms_to` does: the
    /// lifecycle traits and `Writable` hold when every element's does, and a
    /// `TString` never copies. `None` for any other type or trait.
    fn elements_conform(
        &self,
        ty: &Ty,
        trait_name: &str,
        visiting: &mut HashSet<(String, String)>,
    ) -> Option<bool> {
        let tstring = tstring_elements(ty);
        let copies = matches!(trait_name, "Copyable" | "ImplicitlyCopyable");
        if tstring.is_some() && copies {
            return Some(false);
        }
        let elements = tstring.or_else(|| tuple_elements(ty))?;
        if let Some(kind) = trivial_predicate_name(trait_name) {
            return every(
                elements
                    .into_iter()
                    .map(|element| self.trivially(kind, element, visiting)),
            );
        }
        if !copies && !matches!(trait_name, "Movable" | "Deinitable" | "Writable") {
            return None;
        }
        every(
            elements
                .into_iter()
                .map(|element| self.type_conforms(element, trait_name, visiting)),
        )
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
                associated_types: Rc::clone(&bindings.associated_types),
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
    let CtValue::Tuple(elements) = bindings.values.get(param)? else {
        return None;
    };
    elements
        .iter()
        .map(|element| match element {
            CtValue::Type(ty) => Some((**ty).clone()),
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
                [TyArg::Val(packed @ CtValue::Tuple(_))] => packed.clone(),
                rest => CtValue::Tuple(
                    rest.iter()
                        .map(|argument| match argument {
                            TyArg::Ty(ty) => Some(CtValue::Type(Box::new(ty.clone()))),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
            };
            bindings.values.insert(pack.binder(), elements);
            Some(())
        }
        _ if arguments.len() < decls.len() => None,
        _ => bind_ty_args(decls, &arguments, bindings).ok(),
    }
}
