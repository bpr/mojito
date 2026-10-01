//! Certificate grammar for stores: whole-value, field, and element
//! stores, and in-place updates.

use super::{BodyShape, fact_at, grammar_scalar, without_struct_origins};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::{MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// A named place a copying write reads where it lies: `self`, a
    /// parameter, a local, a field of any of them, or a pointer slot.
    pub(super) fn copied_place(&self, place: &Expr) -> bool {
        match &place.kind {
            ExprKind::Identifier(name) => {
                self.receiver_itself(place)
                    || (self.params.contains(&name.as_str())
                        && !self.callable_params.contains(&name.as_str()))
                    || self.declared(name)
            }
            ExprKind::Member { .. } => {
                self.receiver_field(place) || self.local_field(place) || self.parameter_field(place)
            }
            _ => self.slot(place),
        }
    }

    /// A whole value stored to a field of a writable `self` or of a `var`
    /// local whose declared type is the value's own, so neither check
    /// converts it. An element offset from a pointer is such a value, a
    /// temporary of the pointer's own type.
    pub(super) fn whole_store(&self, place: &Expr, value: &Expr) -> bool {
        let scalar = || self.expression(value) && self.scalar(value);
        let offset = || matches!(value.kind, ExprKind::MethodCall { .. }) && self.pointer(value);
        self.moved_result.is_some()
            && ((self.self_writable() && self.receiver_field(place))
                || self.local_field(place)
                || self.slot(place))
            && (self.whole_value(value) || scalar() || offset())
            && self.facts.is_none_or(|facts| {
                let stored = fact_at(&facts.expression_place_types, self.occurrence(place));
                stored.is_some()
                    && stored == fact_at(&facts.expression_types, self.occurrence(value))
            })
    }

    /// A whole value stored to a `mut` parameter. The check accepts the
    /// store only where the value has the parameter's type or converts to it,
    /// and a conversion is a fact no derivation carries, so the two types are
    /// equal in the template and stay equal under substitution.
    pub(super) fn parameter_store(&self, value: &Expr) -> bool {
        let scalar = self.expression(value) && self.scalar(value);
        scalar || self.moved_result.is_some_and(|_| self.whole_value(value))
    }

    /// Whether the local an annotated `var` binds to `value` holds a closed
    /// scalar.
    pub(super) fn scalar_binding(&self, value: &Expr) -> bool {
        self.facts.is_none_or(|facts| {
            fact_at(&facts.binding_types, self.occurrence(value)).is_some_and(grammar_scalar)
        })
    }

    /// Whether an annotated `var`'s value reaches the declared type either
    /// as it is or through a recorded conversion, which an instance selects
    /// again at its own types. A view conversion to an annotation whose
    /// origin is left to inference (`Span[Self.T, _]`) is one: it borrows
    /// the source place as the template's did.
    pub(super) fn annotated_binding(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        self.facts.is_none_or(|facts| {
            fact_at(&facts.binding_types, id).is_some_and(|declared| {
                self.typed(value, declared) || fact_at(&facts.conversions, id).is_some()
            })
        })
    }

    /// Whether the recorded type of `expr` is exactly `ty`, but for the
    /// origin arguments of a struct: a retained type keeps those slots
    /// unbound, and a `return` reconciles a value's origin tail with the
    /// declared one on the places alone (`reconcile_return_origin_tails`),
    /// which no instance changes.
    pub(super) fn typed(&self, expr: &Expr, ty: &Ty) -> bool {
        self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|recorded| {
                without_struct_origins(recorded) == without_struct_origins(ty)
            })
        })
    }

    /// A closed scalar field of a writable `self`, as the target of a store.
    ///
    /// A field reached through a subscript (`self.entries[i].hits`) makes the
    /// subscript a place, which records its index shape: one plain index, and
    /// whether the struct's setter takes the value by keyword. Both are read
    /// off the syntax and the setter's declaration, so an instance inherits
    /// the entry.
    pub(super) fn scalar_field_place(&self, place: &Expr) -> bool {
        let admitted = ((self.self_writable() && self.receiver_field(place))
            || self.local_field(place)
            || self.reference_member(place))
            && self.scalar(place);
        let through = match &place.kind {
            ExprKind::Member { object, .. } if matches!(object.kind, ExprKind::Index { .. }) => {
                Some(object)
            }
            _ => None,
        };
        if admitted && let Some(object) = through {
            self.subscript(self.occurrence(object));
        }
        admitted && (through.is_none() || self.holds(MethodFeatures::SUBSCRIPT_STORES))
    }

    /// A value stored to an element of a writable base
    /// ([`Self::element_base`]), where the subscripted struct declares a
    /// setter (`self.counts[i] = n`, `self.index[b] = entries^`,
    /// `self[k] = v^`, `other.items[i] = x^` on a `mut other`).
    ///
    /// The store is a call of `__setitem__` recorded at the subscript, under
    /// the contract a sibling call has: the index and the value are its
    /// arguments ([`Self::argument`]), a closed scalar or a whole value bound
    /// by value to a parameter of exactly its own type, so an instance changes
    /// the target and, by substitution, the parameter types alone.
    pub(super) fn element_store(&self, place: &Expr, value: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        let admitted = self.element_base(object)
            && self.argument(place, index)
            && self.argument(place, value)
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, place, object, "__setitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
            });
        if admitted {
            self.subscript(self.occurrence(place));
        }
        admitted
            && self.holds(MethodFeatures::SIBLING_CALLS)
            && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// A closed scalar element of a writable `self` or of one of its fields,
    /// stored through the mutable reference its getter yields
    /// (`self.counts[i] += 1`, or `self.cells[i] = n` on a struct that
    /// declares no setter).
    ///
    /// The getter is a reference call, and the store records no second
    /// contract: the checker writes the computed value back through the
    /// reference. Its record is the getter's contract beside the element's
    /// type, kept apart as `augmented_subscripts`, and the reference's
    /// mutability is the receiver binding's, which no instance changes.
    pub(super) fn reference_element(&self, place: &Expr) -> bool {
        self.through_reference(place)
            && self.scalar(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place))
                    .is_some_and(|store| store.inplace.is_none())
            })
    }

    /// A closed scalar element of a writable `self` or of one of its fields
    /// whose struct declares a value getter and a setter, stored augmented
    /// (`self.table[i] += 1`).
    ///
    /// The setter is the call recorded at the subscript, binding the index
    /// and the computed value, which the check keys at the subscript too.
    /// The getter is kept beside it in `augmented_subscripts`, and an
    /// instance realizes it on its own subscripted value as it realizes the
    /// setter (`realize_element_getters`).
    pub(super) fn setter_element(&self, place: &Expr) -> bool {
        self.through_setter(place)
            && self.scalar(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place))
                    .is_some_and(|store| store.inplace.is_none())
            })
    }

    /// A struct element stored augmented through its in-place dunder
    /// (`self.counters[i] += 3` → `__iadd__`), read through a mutable
    /// reference getter or through a value getter and a setter.
    ///
    /// On an element of a closed type the dunder is selected on that type,
    /// so the contract is kept in `augmented_subscripts` as it stands, and
    /// binds the operand, a closed scalar, by value. On an element of a
    /// bare parameter type the dunder is dispatched through the parameter's
    /// bound, and an instance selects its witness on its own element type
    /// (`realize_element_dunders`); the operand is then a value of the
    /// element's own type, bound by value to the witness's `Self`.
    pub(super) fn inplace_element(&self, place: &Expr, value: &Expr) -> bool {
        (self.through_reference(place) || self.through_setter(place))
            && self.expression(value)
            && self.facts.is_none_or(|facts| {
                let store = fact_at(&facts.augmented_subscripts, self.occurrence(place));
                store.is_some_and(|store| {
                    store.inplace.as_ref().is_some_and(|inplace| {
                        let closed = !mojito_types::types::is_symbolic(&store.operand_ty)
                            && self.scalar(value)
                            && mojito_checked::templates::closed_method_contract(inplace);
                        let dispatched = mojito_symbol::symbol::is_trait_dispatch_symbol(
                            &inplace.contract.target,
                        ) && mojito_checked::templates::value_method_contract(
                            inplace,
                        ) && self.typed(value, &store.operand_ty);
                        closed || dispatched
                    })
                })
            })
    }

    /// A place of a struct or bare parameter type updated through its
    /// in-place dunder (`self.total += x`, `self.meter += Meter(1)`,
    /// `into += x`, `m += Meter(3)`): a field of a writable `self`, a `var`
    /// local holding a whole value, or a `mut` parameter.
    ///
    /// The contract is kept at the place in `inplace_updates`, and the
    /// operand is judged against it as a method call's argument. The dunder
    /// may raise in a method declared `raises`. A dunder dispatched through a
    /// bare parameter's bound is re-selected on the instance's type of the
    /// place ([`Checker::realize_embedded_dispatch`]), whose witness of a
    /// raising requirement may not raise.
    /// A struct's own dunder is realized as any method call on that struct
    /// is, which names the instance's clone of a struct built over the
    /// parameter (`realize_inplace_updates`).
    pub(super) fn inplace_place(&self, place: &Expr, value: &Expr) -> bool {
        let writable = (self.self_writable() && self.receiver_field(place))
            || self.value_local(place)
            || matches!(&place.kind, ExprKind::Identifier(name)
                if self.mut_params.contains(&name.as_str()));
        writable
            && self.argument(place, value)
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(place);
                fact_at(&facts.inplace_updates, id).is_some_and(|call| {
                    let dispatched =
                        mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target);
                    let raising = mojito_checked::templates::raising_method_contract(call)
                        && self.holds(MethodFeatures::RAISES);
                    (mojito_checked::templates::value_method_contract(call) || raising)
                        && match fact_at(&facts.expression_types, id) {
                            Some(Ty::Param { .. }) => dispatched,
                            Some(Ty::Struct(..)) => !dispatched,
                            _ => false,
                        }
                })
            })
    }

    /// Note that the subscript `id` is the base of a store.
    pub(super) fn subscript(&self, id: OccurrenceId) {
        let mut subscripts = self.subscripts.borrow_mut();
        if !subscripts.contains(&id) {
            subscripts.push(id);
        }
    }

    /// The subscript `place` of a writable base ([`Self::element_base`]),
    /// stored through the mutable reference its getter yields.
    fn through_reference(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, .. } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.element_base(object)
            && self.reference_call(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, id).is_some_and(|store| store.getter.is_none())
                    && fact_at(&facts.selected_calls, id)
                        .and_then(|call| call.reference_result.as_ref())
                        .is_some_and(|reference| {
                            reference.mutability == mojito_types::origin::Mutability::Mutable
                        })
            });
        if admitted {
            self.subscript(id);
        }
        admitted && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// The subscript `place` of a writable base ([`Self::element_base`]),
    /// read through a value getter and written back through a setter that
    /// takes the element by value. Both change per instance only in their
    /// targets and, by substitution, their types.
    fn through_setter(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.element_base(object)
            && self.argument(place, index)
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, place, object, "__setitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
                    && fact_at(&facts.augmented_subscripts, id)
                        .and_then(|store| store.getter.as_ref())
                        .is_some_and(mojito_checked::templates::value_method_contract)
            });
        if admitted {
            self.subscript(id);
        }
        admitted
            && self.holds(MethodFeatures::SIBLING_CALLS)
            && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// Whether `object` is a base whose elements a store may write: `self`
    /// or one of its fields in a body that may write `self`, or a `mut`
    /// parameter holding a struct or one of its fields. Either is bound to
    /// the instance's argument and written where it lies, and a field has its
    /// declared type under its base's recorded arguments in a template and a
    /// clone alike.
    fn element_base(&self, object: &Expr) -> bool {
        let mut_parameter = |expr: &Expr| {
            matches!(&expr.kind, ExprKind::Identifier(name)
                if self.mut_params.contains(&name.as_str()))
                && self.parameter_receiver(expr)
        };
        (self.self_writable() && (self.receiver_field(object) || self.receiver_itself(object)))
            || mut_parameter(object)
            || matches!(&object.kind, ExprKind::Member { object, .. } if mut_parameter(object))
    }
}
