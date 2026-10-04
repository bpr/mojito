//! Certificate grammar for packs and folded compile-time values: pack
//! storage, accessors, element construction, and tuple elements.

use super::{BodyShape, LocalKind, TUPLE_ELEMENT_ACCESSOR, fact_at, names_method, push_unique};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::MethodFeatures;
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// `self.storage[i]`: the element of a pack struct's `Tuple[*Ts]` field
    /// at the method's own `Int` binder, in a variadic struct's
    /// index-keyed accessor (`def __getitem__[i: Int](self) -> Self.Ts[i]`).
    ///
    /// The template selected `Tuple.__getitem_param__[i]` as a reference
    /// call yielding the dependent `Ts[i]` and requested its per-call clone
    /// keyed by the binder. Each unrolled instance folds the binder to its
    /// position, where the check selects the generated Tuple's accessor for
    /// that position instead (`__getitem_param__$k`), a reference call on
    /// the same field ([`Checker::realize_pack_accessors`]). It is admitted
    /// where a reference call is: returned as the method's reference result,
    /// or the receiver of a call through the element's bound.
    pub(super) fn pack_accessor(&self, expr: &Expr) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        let admitted = !self.keyed
            && self.pack_struct.is_some()
            && self.receiver_field(object)
            && self.folded_value(index)
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let storage = fact_at(&facts.expression_types, self.occurrence(object))
                    .is_some_and(|ty| {
                        matches!(ty, Ty::Struct(name, arguments)
                        if name == mojito_types::types::TUPLE_TYPE_NAME
                            && mojito_types::types::pack_spread_argument(arguments).is_some())
                    });
                let requested = fact_at(&facts.method_instantiations, id).is_some_and(|request| {
                    request.owner == mojito_types::types::TUPLE_TYPE_NAME
                        && request.method == TUPLE_ELEMENT_ACCESSOR
                        && request.owner_arguments.is_empty()
                        && request.arguments.len() == 1
                });
                let element = fact_at(&facts.reference_results, id).is_some_and(|reference| {
                    matches!(&reference.referent, Ty::Dependent(dependent)
                    if dependent.pack_element().is_some_and(|(_, index)| {
                        matches!(index.kind(), mojito_types::param_expr::ParamKind::DeclRef(_))
                    }))
                });
                let contract = fact_at(&facts.selected_calls, id).is_some_and(|call| {
                    call.contract.target
                        == format!(
                            "{}.{TUPLE_ELEMENT_ACCESSOR}",
                            mojito_types::types::TUPLE_TYPE_NAME
                        )
                        && call.contract.receiver_requires_place
                        && call.contract.raises.is_none()
                        && call.contract.arguments.is_empty()
                        && call.contract.captures.is_empty()
                        && call.invalidations.is_empty()
                });
                storage
                    && requested
                    && element
                    && contract
                    && fact_at(&facts.parameterized_method_calls, id).is_some()
            });
        if admitted {
            self.references.borrow_mut().push(self.occurrence(expr));
        }
        admitted
            && self.holds(MethodFeatures::REFERENCE_CALLS)
            && self.holds(MethodFeatures::PARAMETERIZED_CALLS)
    }

    /// `local[k]`: an element of a tuple-typed `var` local at a literal
    /// index, read as a scalar operand or by value into a `var` or the
    /// result ([`Self::tuple_element_value`]).
    ///
    /// The index is the element's position, which no instance changes, and
    /// the element's type substitutes. The template either typed the element
    /// from the tuple's arguments and recorded nothing else there, or
    /// selected the generated Tuple's accessor for that position
    /// (`__getitem_param__$k`) as a closed reference call read by copy, which
    /// an instance records again on its own Tuple
    /// ([`Checker::realize_tuple_elements`]).
    pub(super) fn tuple_element(&self, expr: &Expr) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        // Without facts a whole-value declaration reads as a scalar one;
        // the tuple's recorded type rules a scalar local out.
        let local = matches!(&object.kind, ExprKind::Identifier(name)
            if matches!(self.local_kind(name), Some(LocalKind::Value | LocalKind::Scalar)));
        if self.keyed || !local || !matches!(index.kind, ExprKind::Int(_)) {
            return false;
        }
        let id = self.occurrence(expr);
        let Some(facts) = self.facts else {
            return true;
        };
        let Some(tuple @ Ty::Struct(owner, _)) =
            fact_at(&facts.expression_types, self.occurrence(object))
        else {
            return false;
        };
        if mojito_types::types::tuple_elements(tuple).is_none() {
            return false;
        }
        let Some(call) = fact_at(&facts.selected_calls, id) else {
            return fact_at(&facts.overload_targets, id).is_none()
                && fact_at(&facts.operation_adjustments, id).is_none();
        };
        let accessor = names_method(&call.contract.target, owner, TUPLE_ELEMENT_ACCESSOR)
            && mojito_checked::templates::closed_reference_contract(call)
            && facts.copyable_reference_result_reads.contains(&id);
        if accessor {
            self.references.borrow_mut().push(id);
        }
        accessor && self.holds(MethodFeatures::REFERENCE_CALLS)
    }

    /// A tuple element read by value into a `var` or the result, whatever
    /// its type. The template either typed it from the tuple's arguments or
    /// selected the accessor as a copyable reference read, and an instance
    /// records the accessor read again and owes the copy at its own element
    /// type ([`Checker::realize_tuple_elements`]).
    pub(super) fn tuple_element_value(&self, expr: &Expr) -> bool {
        self.tuple_element(expr) && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// `__RuntimeTuple(*args^)` or `Tuple(*args^)`: a pack struct's storage
    /// built from the initializer's own pack collector, moved whole into
    /// the private storage, or element by element into the public tuple.
    /// The template typed the storage and the collector over the symbolic
    /// pack, which an instance substitutes element by element.
    pub(super) fn pack_storage(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let [argument] = args.as_slice() else {
            return false;
        };
        let ExprKind::Spread(transfer) = &argument.kind else {
            return false;
        };
        let ExprKind::Transfer(pack) = &transfer.kind else {
            return false;
        };
        let param = match &pack.kind {
            ExprKind::Identifier(pack) if self.packs.contains(&pack.as_str()) => {
                self.params.iter().position(|param| param == pack)
            }
            _ => None,
        };
        let Some(param) = param
            .filter(|_| param_args.is_empty() && kwargs.is_empty() && self.pack_struct.is_some())
        else {
            return false;
        };
        let (call, spread, transfer, pack) = (
            self.occurrence(expr),
            self.occurrence(argument),
            self.occurrence(transfer),
            self.occurrence(pack),
        );
        match name.as_str() {
            "__RuntimeTuple" => push_unique(
                &mut self.pack_relocations.borrow_mut(),
                mojito_checked::templates::PackRelocation {
                    call,
                    spread,
                    transfer,
                    pack,
                    param,
                },
            ),
            // The public tuple, as the check typed it: a user `Tuple` would
            // type the call as its own struct.
            "Tuple"
                if self.facts.is_none_or(|facts| {
                    matches!(fact_at(&facts.expression_types, call),
                        Some(Ty::Struct(tuple, _)) if tuple == "Tuple")
                }) =>
            {
                push_unique(
                    &mut self.pack_spreads.borrow_mut(),
                    mojito_checked::templates::PackSpread {
                        call,
                        spread,
                        transfer,
                        pack,
                        param,
                    },
                );
            }
            _ => return false,
        }
        true
    }

    /// `self.storage[i] = Self.Ts[i]()`: one element of a pack struct's
    /// storage on a writable `self`, built at the innermost `comptime for`
    /// variable from the pack element's own default construction
    /// ([`Self::element_construction`]), the place typed as the same
    /// dependent element.
    pub(super) fn element_initialization(&self, place: &Expr, value: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        self.self_writable()
            && self.receiver_field(object)
            && self.at_loop(index)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_place_types, self.occurrence(place))
                    == fact_at(&facts.expression_types, self.occurrence(value))
            })
            && self.element_construction(value)
    }

    /// `Self.Ts[i]()`, or a `def`'s own `Ts[i]()`: a pack element's default
    /// construction at the innermost `comptime for` variable, stored to the
    /// element's own storage ([`Self::element_initialization`]), handed to
    /// `print`, or bound to a local.
    ///
    /// The template typed the construction as the dependent element `Ts[i]`,
    /// which the availability clause or the pack's bound proves
    /// `Defaultable`, and recorded nothing else at it but that construction
    /// (`ConstructPackElement`). The elaborator writes
    /// the element's concrete construction there, closed syntax naming
    /// nothing the body binds, whose facts an instance records from its own
    /// check of that construction alone
    /// ([`Checker::element_construction_facts`]). What the statement around
    /// it decides from its type — the store, the print's `Writable` proof,
    /// the local's binding — each instance decides at its own element.
    pub(super) fn element_construction(&self, value: &Expr) -> bool {
        let (pack, param_args, args, kwargs) = match &value.kind {
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => (
                matches!(&callee.kind, ExprKind::Member { object, field }
                    if self.pack_struct == Some(field.as_str())
                        && matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")),
                param_args,
                args,
                kwargs,
            ),
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => (
                self.pack_binders.contains(&name.as_str()),
                param_args,
                args,
                kwargs,
            ),
            _ => return false,
        };
        let dependent_element = |ty: &Ty| {
            matches!(ty, Ty::Dependent(dependent)
            if dependent.pack_element().is_some_and(|(_, index)| {
                matches!(index.kind(), mojito_types::param_expr::ParamKind::DeclRef(_))
            }))
        };
        let admitted = pack
            && matches!(param_args.as_slice(),
                [mojito_ast::ast::ParamArg::Value(index)] if self.at_loop(index))
            && args.is_empty()
            && kwargs.is_empty()
            && self.facts.is_none_or(|facts| {
                let built = self.occurrence(value);
                fact_at(&facts.expression_types, built).is_some_and(dependent_element)
                    && fact_at(&facts.operation_adjustments, built).is_none_or(|adjustment| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::ConstructPackElement { .. }
                        )
                    })
                    && fact_at(&facts.conversions, built).is_none()
                    && fact_at(&facts.overload_targets, built).is_none()
            });
        if admitted {
            push_unique(
                &mut self.element_constructions.borrow_mut(),
                self.occurrence(value),
            );
        }
        admitted
    }

    /// Whether every instance folds `expr` to a literal: a folded value, or
    /// arithmetic, a comparison, a negation, or an inversion over folded
    /// values and literals that names at least one folded value.
    pub(super) fn folding(&self, expr: &Expr) -> bool {
        fn literal_tree(shape: &BodyShape<'_>, expr: &Expr) -> Option<bool> {
            use mojito_ast::ast::InfixOp::{
                Add, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, Le, Lt, Mod, Mul, Ne, Pow,
                Shl, Shr, Sub,
            };
            match &expr.kind {
                ExprKind::Int(_) => Some(false),
                ExprKind::Identifier(name) if shape.module_constant(name) => Some(false),
                ExprKind::Identifier(_) => shape.folds(expr).then_some(true),
                ExprKind::Prefix(
                    mojito_ast::ast::PrefixOp::Neg | mojito_ast::ast::PrefixOp::Invert,
                    value,
                ) => literal_tree(shape, value),
                ExprKind::Infix(
                    Add | Sub | Mul | Div | FloorDiv | Mod | Pow | Shl | Shr | BitAnd | BitOr
                    | BitXor | Eq | Ne | Lt | Le | Gt | Ge,
                    left,
                    right,
                ) => Some(literal_tree(shape, left)? | literal_tree(shape, right)?),
                _ => None,
            }
        }
        literal_tree(self, expr) == Some(true)
    }

    /// A folded compile-time value read where it stands, as an `Int` or a
    /// `Bool`: the instance's literal materializes to exactly that type
    /// ([`folded_literals`]).
    /// `Self.Ts.length` over the struct's own type pack, which the
    /// elaborator folds to the instance's element count
    /// (`folded_literals`): the template typed it `Int` and recorded at most
    /// the parameter value the fold answers, which no instance keeps.
    pub(super) fn pack_length(&self, expr: &Expr) -> bool {
        let ExprKind::Member { object, field } = &expr.kind else {
            return false;
        };
        let pack = matches!(&object.kind, ExprKind::Member { object, field: pack }
            if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")
                && self.pack_struct == Some(pack.as_str()));
        field == "length"
            && pack
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                fact_at(&facts.expression_types, id) == Some(&Ty::Int)
                    && fact_at(&facts.operation_adjustments, id).is_none_or(|adjustment| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::ParamValue { .. }
                        )
                    })
            })
            && self.holds(MethodFeatures::STATEMENTS)
            && self.holds(MethodFeatures::COMPTIME_CONTROL)
    }

    /// A module's exact integer constant, read where no local or parameter
    /// shadows it: its binding and its `IntLiteral` type are the same under
    /// every instance, as a literal's are.
    pub(super) fn module_constant(&self, name: &str) -> bool {
        self.local_kind(name).is_none()
            && !self.params.contains(&name)
            && self.constants.contains_key(name)
    }

    pub(super) fn folded_value(&self, expr: &Expr) -> bool {
        self.folds(expr)
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                matches!(
                    fact_at(&facts.expression_types, id),
                    Some(Ty::Int | Ty::Bool)
                ) && fact_at(&facts.expression_bindings, id).is_some()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            })
    }

    /// `pack[i]`: an element of a pack-typed parameter at the innermost
    /// `comptime for` variable, or of a pack struct's storage field read on
    /// `self` or on a parameter of the struct (`other.storage[i]`).
    ///
    /// The template typed the element once, as the dependent `Ts[i]` over
    /// the loop's own binder, and recorded nothing else there: no place, no
    /// adjustment, no borrow. The elaborator folds `i` to the iteration's
    /// literal in each unrolled copy, which the instance reads back to fix
    /// the element ([`Checker::realize_instance_facts`]).
    pub(super) fn pack_element(&self, expr: &Expr) -> bool {
        self.pack_element_read(expr, false)
    }

    /// [`Self::pack_element`] handed to a checker builtin that reads it
    /// where it lies (`writer.write(self.storage[i])`): the builtin's borrow
    /// of the element is decided by its syntax, as a named place's is.
    pub(super) fn lent_pack_element(&self, expr: &Expr) -> bool {
        self.pack_element_read(expr, true)
    }

    /// Whether `expr` names a compile-time value the elaborator folds to a
    /// literal in every instance: a `comptime for` variable, or a value
    /// parameter, that no local shadows.
    fn folds(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Identifier(name)
            if self.local_kind(name).is_none()
                && !self.params.contains(&name.as_str())
                && (self.values.contains(&name.as_str())
                    || self.loop_vars.borrow().iter().any(|var| var == name)))
    }

    fn pack_element_read(&self, expr: &Expr, lent: bool) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        let collection = match &object.kind {
            ExprKind::Identifier(name) => self.packs.contains(&name.as_str()),
            ExprKind::Member { .. } => {
                self.pack_struct.is_some()
                    && (self.receiver_field(object) || self.parameter_field(object))
            }
            _ => false,
        };
        // The innermost loop variable, or a method's own index binder
        // (`__getitem_param__[index: Int]`), which every instance folds.
        let named = collection
            && matches!(&index.kind, ExprKind::Identifier(name)
                if self.loop_vars.borrow().last() == Some(name)
                    || (self.loop_vars.borrow().is_empty()
                        && self.local_kind(name).is_none()
                        && self.values.contains(&name.as_str())));
        named
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let ty = fact_at(&facts.expression_types, id);
                let dependent_element = |ty: &Ty| {
                    matches!(ty, Ty::Dependent(dependent)
                    if dependent.pack_element().is_some_and(|(_, index)| {
                        matches!(index.kind(), mojito_types::param_expr::ParamKind::DeclRef(_))
                    }))
                };
                // An erased `rebind[T](self.storage[i])` retypes the element
                // to its target, and asserts the two equal
                // (`TemplateObligation::RebindEqualities`).
                let element = ty.is_some_and(dependent_element)
                    || fact_at(&facts.rebind_assertions, id)
                        .is_some_and(|assertion| dependent_element(&assertion.operand));
                // The element is a place of the collector, read where it
                // lies. A lent one is kept by the builtin that reads it
                // (`repr`), which admits the place it keeps
                // ([`Self::references_recorded`]).
                element
                    && fact_at(&facts.expression_place_types, id)
                        .is_none_or(|place| Some(place) == ty)
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && (lent
                        || (!facts.call_place_uses.contains(&id)
                            && !facts.borrowed_read_call_places.contains(&id)))
            })
    }
}
