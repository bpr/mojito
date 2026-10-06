//! Certificate grammar for values a body builds: struct and binder
//! constructions, displays, slices, and conversions.

use super::{
    BodyShape, LocalKind, closed_scalar, conversion_target, fact_at, push_unique, type_argument,
};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::{MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// `slice.indices(n)` on a slice parameter or local: the built-in
    /// normalization, which reads a closed slice and a closed scalar length
    /// and yields a tuple of `Int`s. It selects nothing, so the call records
    /// only closed types, alike under every instance.
    pub(super) fn slice_indices(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let receiver = matches!(&object.kind, ExprKind::Identifier(name)
            if self.params.contains(&name.as_str()) || self.local_kind(name).is_some());
        let shape = !self.keyed
            && method == "indices"
            && receiver
            && kwargs.is_empty()
            && matches!(args, [length] if self.expression(length) && self.scalar(length));
        shape
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let slice = fact_at(&facts.expression_types, self.occurrence(object)).is_some_and(
                    |ty| matches!(ty, Ty::Struct(name, arguments)
                        if arguments.is_empty()
                            && matches!(name.as_str(), "Slice" | "StridedSlice" | "ContiguousSlice")),
                );
                slice
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.call_parameters, id).is_none()
            })
    }

    /// `__mojito_fieldwise_copy(self)`, the synthesized `copy`'s result: a
    /// copy of the receiver whole, which the declaration's `Copyable`
    /// clause guarantees every instance
    /// ([`TemplateObligation::DeclarationConstraints`]).
    pub(super) fn fieldwise_copy(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "__mojito_fieldwise_copy"
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [receiver] if self.receiver_itself(receiver)))
    }

    /// A parameter, a local, or a field of `self` that an `@implicit`
    /// constructor reads in place: no copy is recorded, and an instance
    /// selecting a constructor that consumes its source refuses
    /// ([`Checker::realize_conversion`]).
    pub(super) fn converted_place(&self, expr: &Expr) -> bool {
        let named = match &expr.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(expr),
            _ => false,
        };
        named
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.conversions, self.occurrence(expr)).is_some())
            && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// `String(value)` of one value other than a string literal
    /// (`String(fspath[byte=:i])`, `String(err)`, `String(self.w)` over `W:
    /// Writable`): the stringify builtin, which reads the value where it
    /// lies to write it through its `Writable` conformance and wraps the text
    /// as the nominal `String`.
    ///
    /// The call routes to the builtin (an overload target of `"String"`) and
    /// records the wrap's conversion at itself, neither chosen by the
    /// argument's type. A value other than a scalar is written through its
    /// `Writable` conformance, which keeps its place, admitted here
    /// ([`Self::references_recorded`]). A named place of a symbolic type is
    /// kept so in the template, and an instance keeps it or, at a numeric
    /// or `Bool` type, releases it ([`Checker::realize_stringify`]).
    pub(super) fn stringify(&self, expr: &Expr) -> bool {
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
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let id = self.occurrence(expr);
        let value = self.occurrence(argument);
        let scalar = self.expression(argument) && self.scalar(argument);
        let kept = self
            .facts
            .is_some_and(|facts| facts.call_place_uses.contains(&value));
        let admitted = !self.keyed
            && mojito_types::types::is_stdlib_string_struct(name)
            && param_args.is_empty()
            && kwargs.is_empty()
            && (scalar || named || self.whole_value(argument))
            && (self.closed(argument) || (named && kept))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.overload_targets, id).is_some_and(|target| target == "String")
                    && fact_at(&facts.conversions, id).is_some()
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.expression_types, value) != Some(&Ty::StringLiteral)
                    && (scalar || kept)
            });
        if admitted {
            if kept {
                push_unique(&mut self.places.borrow_mut(), value);
            }
            push_unique(&mut self.stringified.borrow_mut(), id);
        }
        admitted && self.holds(MethodFeatures::STRINGIFY)
    }

    /// A tuple display (`return head, tail`): a temporary `Tuple` built
    /// from its elements, each a closed scalar or a whole value the display
    /// takes as it stands, converted by nothing an instance selects.
    ///
    /// The display records its `Tuple` type as a collection construction,
    /// which an instance substitutes element by element.
    pub(super) fn tuple_display(&self, expr: &Expr) -> bool {
        let ExprKind::TupleLit(elements) = &expr.kind else {
            return false;
        };
        !self.keyed
            && !elements.is_empty()
            && elements.iter().all(|element| {
                ((self.expression(element) && self.scalar(element)) || self.whole_value(element))
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.conversions, self.occurrence(element)).is_none()
                    })
            })
            && self.facts.is_none_or(|facts| {
                matches!(
                    fact_at(&facts.operation_adjustments, self.occurrence(expr)),
                    Some(
                        mojito_checked::checked::SemanticAdjustment::ConstructCollection {
                            insert: None,
                            ..
                        }
                    )
                )
            })
    }

    /// A list, set, or dict display the expected type makes a collection
    /// (`[1, 2, 3]` handed to a `List[Int]` parameter): a temporary built
    /// through the collection's insert method, from elements each a closed
    /// scalar, a whole value the display takes as it stands, or a string
    /// literal converted into a nominal element type.
    ///
    /// The display records its collection type as a construction, which an
    /// instance substitutes, and a literal's conversion beside it, which an
    /// instance selects again at its own element type
    /// ([`Checker::realize_conversion`]). A display with no expected
    /// collection type is a fixed-size `Array`, which records its variadic
    /// constructor instead and stays outside.
    pub(super) fn collection_display(&self, expr: &Expr) -> bool {
        let elements: Vec<&Expr> = match &expr.kind {
            ExprKind::ListLit(elements) => elements.iter().collect(),
            ExprKind::BraceLit(entries) => entries
                .iter()
                .flat_map(|(key, value)| std::iter::once(key).chain(value))
                .collect(),
            _ => return false,
        };
        let id = self.occurrence(expr);
        !self.keyed
            && elements.iter().all(|element| {
                let converted = self.facts.is_some_and(|facts| {
                    fact_at(&facts.conversions, self.occurrence(element)).is_some()
                });
                if converted {
                    return matches!(element.kind, ExprKind::Str(_));
                }
                (self.expression(element) && self.scalar(element))
                    || matches!(element.kind, ExprKind::Str(_))
                    || self.whole_value(element)
            })
            && self.facts.is_none_or(|facts| {
                matches!(
                    fact_at(&facts.operation_adjustments, id),
                    Some(
                        mojito_checked::checked::SemanticAdjustment::ConstructCollection {
                            insert: Some(_),
                            target,
                            ..
                        }
                    ) if fact_at(&facts.expression_types, id) == Some(target)
                )
            })
    }

    /// A keyword slice of a local or a parameter of a closed type
    /// (`fspath[byte=:i]`), with closed scalar bounds: a view temporary its
    /// getter returns over the place, whose origin an instance roots at its
    /// own binding of the place.
    ///
    /// The getter is selected on the place's recorded struct, which every
    /// instance shares, and binds the slice descriptor by value; the
    /// descriptor the check keys at the subscript depends only on the
    /// bounds' syntax.
    pub(super) fn keyword_slice(&self, expr: &Expr) -> bool {
        use mojito_ast::ast::SubscriptArg;
        let ExprKind::MultiIndex { object, args } = &expr.kind else {
            return false;
        };
        let named = matches!(&object.kind, ExprKind::Identifier(name)
            if self.declared(name) || self.params.contains(&name.as_str()));
        let bounds = !args.is_empty()
            && args.iter().all(|argument| match argument {
                SubscriptArg::KeywordSlice {
                    lower, upper, step, ..
                } => [lower, upper, step]
                    .into_iter()
                    .flatten()
                    .all(|bound| self.expression(bound) && self.scalar(bound)),
                _ => false,
            });
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && named
            && self.closed(object)
            && bounds
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, expr, object, "__getitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
                    && fact_at(&facts.subscript_descriptors, id).is_some()
            });
        if admitted {
            self.subscript(id);
        }
        admitted && self.holds(MethodFeatures::SLICE_VIEWS)
    }

    /// A construction of one of the method's own trait-bounded binders
    /// (`H()`): a temporary of the binder's type, which every clone keeps
    /// symbolic, so each records the same `ConstructTypeParam`.
    ///
    /// The adjustment names the binder it constructs: a binder of the
    /// enclosing struct is another type under each instance, and stays
    /// outside.
    pub(super) fn binder_construction(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && param_args.is_empty()
            && args.is_empty()
            && kwargs.is_empty()
            && self.binders.contains(&name.as_str())
            && self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.operation_adjustments, id),
                    Some(mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param })
                        if *param.name == **name
                            && matches!(fact_at(&facts.expression_types, id),
                            Some(Ty::Param { binder, .. })
                                if binder == param && !self.struct_binders.contains(&&binder.id)))
            });
        if admitted {
            self.binder_constructions.borrow_mut().push(id);
        }
        admitted && self.holds(MethodFeatures::BOUND_BINDERS)
    }

    /// A construction of one of the enclosing struct's type binders
    /// (`Self.T()`): a temporary of the binder's type, recorded as the
    /// `ConstructTypeParam` of that binder.
    ///
    /// The template's own later passes keep the binder symbolic and reuse
    /// the adjustment as recorded; a clone that binds it substitutes the
    /// recorded type, whose adjustment then has no recipe
    /// (`kept_binder_construction`), so the clone keeps its own check.
    pub(super) fn struct_binder_construction(&self, expr: &Expr) -> bool {
        let ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && self.receiver
            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
            && args.is_empty()
            && kwargs.is_empty()
            && self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.operation_adjustments, id),
                    Some(mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param })
                        if *param.name == **method
                            && matches!(fact_at(&facts.expression_types, id),
                            Some(Ty::Param { binder, .. })
                                if binder == param && self.struct_binders.contains(&&binder.id)))
            });
        if admitted {
            self.binder_constructions.borrow_mut().push(id);
        }
        admitted
    }

    /// A construction of a declared struct: a temporary of the constructed
    /// type, whose constructor an instance re-selects on its own arguments.
    ///
    /// Its compile-time arguments are types, so it binds no origin
    /// immutably and folds no value. Each argument is a closed scalar, a
    /// whole value, or the `copy:` of a named place, so what the call records
    /// at an argument is decided by the argument's syntax and the
    /// constructor's conventions, and the template's selection binds every
    /// argument exactly under every instance. A hand-written constructor's
    /// `ref` parameter takes a named place or a reference, which it lends.
    pub(super) fn construction(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        if self.keyed || !self.structs.contains_key(name) {
            return false;
        }
        // A binding of that name would shadow the struct: the recorded type
        // says the call constructed it.
        let id = self.occurrence(expr);
        // The stringify builtin types as the struct it wraps its text in
        // ([`Self::stringify`]), and is routed to the builtin.
        let constructed = self.facts.is_none_or(|facts| {
            matches!(fact_at(&facts.expression_types, id),
                Some(Ty::Struct(constructed, _)) if constructed == name)
                && fact_at(&facts.overload_targets, id).is_none_or(|target| target != "String")
        });
        let named_place = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                (self.receiver && name == "self")
                    || self.params.contains(&name.as_str())
                    || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(place),
            _ => false,
        };
        let copied = args.is_empty()
            && matches!(kwargs.as_slice(), [copy] if copy.name == "copy" && named_place(&copy.value));
        // A fieldwise struct's reference field takes a `ref` local as the
        // handle it is: the argument records the referent's read-through
        // facts and that it is kept as a handle, both decided by the field's
        // declaration and the binding's kind.
        let info = &self.structs[name];
        let reference_field = |field: Option<&(String, Ty)>, argument: &Expr| {
            info.fieldwise_init
                && !info.methods.contains_key("__init__")
                && field.is_some_and(|(_, ty)| matches!(ty, Ty::Ref(_)))
                && matches!(&argument.kind, ExprKind::Identifier(name) if self.reference_local(name))
        };
        // A hand-written constructor's `ref` parameter borrows the place it is
        // handed, a named place or one reached through a reference. Which
        // positions lend is the selected constructor's declaration, and the
        // loan's mutability the place's own, so neither changes per instance.
        let lent = |position: usize| {
            self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.operation_adjustments, id),
                    Some(mojito_checked::checked::SemanticAdjustment::BorrowRefArguments {
                        arguments,
                        materialized: None,
                    }) if arguments.iter().any(|(lent, _)| *lent == position))
            })
        };
        let lent_place = |position: usize, argument: &Expr| {
            lent(position) && (named_place(argument) || self.reference_argument(argument))
        };
        // An untracked pointer is plain data under every instance: a read
        // parameter reads it where it lies and a `var` one copies it, as the
        // template did.
        let value = |field: Option<&(String, Ty)>, argument: &Expr| {
            (self.expression(argument) && self.scalar(argument))
                || self.whole_value(argument)
                || self.unconverted_string_literal(argument)
                || self.pointer(argument)
                || self.receiver_pointer(argument)
                || reference_field(field, argument)
        };
        let admitted = constructed
            && param_args.iter().all(type_argument)
            && (copied
                || (args.iter().enumerate().all(|(position, argument)| {
                    value(info.fields.get(position), argument) || lent_place(position, argument)
                }) && kwargs.iter().all(|keyword| {
                    let field = info.fields.iter().find(|(name, _)| *name == keyword.name);
                    value(field, &keyword.value)
                })));
        if admitted {
            for (position, argument) in args.iter().enumerate() {
                if !value(info.fields.get(position), argument) && lent_place(position, argument) {
                    let mut places = self.places.borrow_mut();
                    let id = self.occurrence(argument);
                    if !places.contains(&id) {
                        places.push(id);
                    }
                }
            }
            let fields = args
                .iter()
                .enumerate()
                .map(|(position, argument)| (info.fields.get(position), argument));
            let keyed = kwargs.iter().map(|keyword| {
                let field = info.fields.iter().find(|(name, _)| *name == keyword.name);
                (field, &keyword.value)
            });
            for (field, argument) in fields.chain(keyed) {
                if reference_field(field, argument) {
                    self.handle(self.occurrence(argument));
                }
            }
            let mut constructions = self.constructions.borrow_mut();
            if !constructions.contains(&id) {
                constructions.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::CONSTRUCTIONS)
    }

    /// A built-in scalar conversion of one value of a closed type
    /// (`Int(key_hash)`, or `Bool(result)` of a closed struct place, which
    /// its conversion dunder reads in place, or of a lane comparison's mask
    /// local, [`LocalKind::Mask`]), of a value-shaped vector a
    /// keyed body holds (`Int(Scalar[dt](v))`), or of a place whose type
    /// binder carries the conversion's bound (`Int(mode)` on `mode: intable`
    /// with `intable: Intable`, `Int(self.value)` on `value: Self.T`), whose
    /// witness each instance realizes (`realize_bound_conversion`). It selects no callee and
    /// records only its closed result type, which no instance changes; a
    /// declaration of that name would record a selection at the call, and
    /// such a call is not this.
    pub(super) fn scalar_conversion(
        &self,
        id: OccurrenceId,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let [argument] = args else {
            return false;
        };
        let place = match &argument.kind {
            ExprKind::Identifier(local) => {
                self.params.contains(&local.as_str())
                    || self.declared(local)
                    || self.reference_local(local)
                    || (name == "Bool" && self.local_kind(local) == Some(LocalKind::Mask))
            }
            ExprKind::Member { .. } => self.receiver_field(argument),
            _ => false,
        };
        let bounded = place
            && conversion_target(name)
                .and_then(|target| crate::checker::builtins::conversion_protocol(&target))
                .is_some_and(|(_, bound)| {
                    self.facts.is_some_and(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(argument))
                            .is_some_and(|ty| crate::checker::builtins::param_has_bound(ty, bound))
                    })
                });
        let comparison = name == "Bool" && self.lane_comparison(argument);
        matches!(name, "Int" | "UInt" | "Bool" | "Float64")
            && param_args.is_empty()
            && kwargs.is_empty()
            && (self.expression(argument) || place || comparison)
            && (self.closed(argument) || self.value_shaped(argument) || bounded)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id).is_some_and(closed_scalar)
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
            })
    }

    /// A string literal a construction takes as the `StringLiteral` it is
    /// (`String("x")`): a temporary of a closed type no instance changes,
    /// converted to nothing.
    fn unconverted_string_literal(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        matches!(expr.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
                    && fact_at(&facts.conversions, id).is_none()
            })
    }
}
