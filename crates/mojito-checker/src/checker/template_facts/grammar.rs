//! The certificate grammar's core walk: statements, expressions, whole
//! values, and the local and receiver predicates every class shares.

use super::{
    BodyShape, GrammarNotes, LocalKind, comparison, fact_at, grammar_scalar, type_argument,
};
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;
use std::collections::HashSet;

impl BodyShape<'_> {
    /// What the certificate hands the instance from the grammar's walk: the
    /// occurrences it must dispatch or prove itself.
    pub(super) fn grammar_notes(&self) -> GrammarNotes {
        GrammarNotes {
            operators: self.operators.borrow().clone(),
            bound_builtins: self.bound_builtins.borrow().clone(),
            constructions: self.constructions.borrow().clone(),
            callable_calls: self.callable_calls.borrow().clone(),
            repr_calls: self.repr_calls.borrow().clone(),
            print_calls: self.print_calls.borrow().clone(),
            simd_to_bits: self.simd_to_bits.borrow().clone(),
            simd_casts: self.simd_casts.borrow().clone(),
            simd_lengths: self.simd_lengths.borrow().clone(),
            pack_relocations: self.pack_relocations.borrow().clone(),
            pack_spreads: self.pack_spreads.borrow().clone(),
            element_constructions: self.element_constructions.borrow().clone(),
        }
    }

    /// Note that the body holds `feature`.
    pub(super) fn holds(&self, feature: MethodFeatures) -> bool {
        self.features.set(self.features.get().union(feature));
        true
    }

    /// A runtime condition: a scalar expression whose recorded type is
    /// exactly `Bool`, which `expect_bool` accepts without a truthiness fact,
    /// a closed bool lane tested through `Bool(x)` (`n != 0` over a
    /// `UInt32`), whose mark no instance changes, or a place tested through
    /// `Bool(x)` ([`Self::truthiness_condition`]).
    pub(super) fn condition(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        let expression = self.expression(expr);
        let boolean = expression
            && self.facts.is_none_or(|facts| {
                facts
                    .expression_types
                    .iter()
                    .any(|(site, ty)| *site == id && *ty == Ty::Bool)
            });
        let lane = || {
            expression
                && self.scalar(expr)
                && self.facts.is_none_or(|facts| {
                    facts.truthiness_conditions.contains(&id)
                        && fact_at(&facts.conversions, id).is_none()
                        && fact_at(&facts.operation_adjustments, id).is_none()
                })
                && self.holds(MethodFeatures::TRUTHINESS)
        };
        let comparison = || {
            self.lane_comparison(expr)
                && self
                    .facts
                    .is_none_or(|facts| facts.truthiness_conditions.contains(&id))
                && self.holds(MethodFeatures::TRUTHINESS)
        };
        boolean || lane() || comparison() || self.truthiness_condition(expr)
    }

    /// A whole value of any type, moved or copied out of a parameter, a
    /// local, or a field of `self`, or a `var` receiver moved out whole. It is never an operand, a receiver, or a
    /// condition, and as an argument it binds a parameter of its own type
    /// ([`Self::argument`]), so nothing dispatches on its type.
    ///
    /// A `^` transfer owes `Movable` at the instance's type. A bare place is
    /// admitted only where the template recorded the copy, which the instance
    /// then owes: a type that is not copyable records nothing there, and its
    /// clone check would.
    pub(super) fn whole_value(&self, expr: &Expr) -> bool {
        let source = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.declared(name)
                    || self.reference_local(name)
                    || self.receiver_itself(place)
            }
            ExprKind::Member { .. } => {
                self.receiver_field(place)
                    || self.local_field(place)
                    || self.parameter_field(place)
                    || self.reference_member(place)
            }
            _ => false,
        };
        // A place the body only borrows is copied, never moved out of.
        let borrowed = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                self.borrowed_params.contains(&name.as_str())
                    || self.reference_local(name)
                    || (self.receiver_itself(place) && !self.self_owned())
            }
            ExprKind::Member { object, .. } => {
                let torn_down = matches!(&object.kind, ExprKind::Identifier(name)
                    if self.deinit_params.contains(&name.as_str()));
                !(self.receiver_field(place) || (torn_down && self.parameter_field(place)))
            }
            _ => false,
        };
        let admitted = match &expr.kind {
            ExprKind::Transfer(inner) => {
                (source(inner) && !borrowed(inner)) || self.call_result(inner)
            }
            _ if self.call_result(expr)
                || self.pack_storage(expr)
                || self.fieldwise_copy(expr)
                || self.construction(expr)
                || self.binder_construction(expr)
                || self.struct_binder_construction(expr)
                || self.receiver_pointer(expr)
                || self.operator_value(expr)
                || self.comprehension(expr)
                || self.tuple_display(expr)
                || self.collection_display(expr)
                || self.stringify(expr)
                || self.closed_operator(expr) =>
            {
                true
            }
            // The pointee, taken out of its slot: a temporary.
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                method == "unsafe_take_pointee"
                    && args.is_empty()
                    && kwargs.is_empty()
                    && self.pointer(object)
            }
            _ => {
                let id = self.occurrence(expr);
                (source(expr) || self.slot(expr))
                    && self
                        .facts
                        .is_none_or(|facts| facts.copy_place_value_uses.contains(&id))
            }
        };
        admitted && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// A nested block: its locals go out of scope with it.
    pub(super) fn block(&self, statements: &[Stmt]) -> bool {
        let outer = self.locals.borrow().len();
        let admitted = statements.iter().all(|statement| self.statement(statement));
        self.locals.borrow_mut().truncate(outer);
        admitted
    }

    /// Whether `expr` is a field of `self` in a method body: `self.<field>`,
    /// or a field of such a field holding a struct (`self.scaler.base`). A
    /// field has its declared type under its base's recorded arguments, so
    /// every instance reads the same path and only `self`'s binding changes.
    pub(super) fn receiver_field(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
                    || (self.receiver_field(object) && self.nominal(object)))
    }

    /// Whether `expr` is `Self.<value>` in a method body, reading the
    /// struct's own scalar value binder: a runtime read of the reified
    /// parameter on the erased path, and in a generator's per-instantiation
    /// clone a literal it folds under the name's identity, with no binding
    /// of its own either way.
    pub(super) fn struct_value(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                    && self.struct_values.contains(&field.as_str()))
    }

    /// Whether `expr` is `self` itself in a method body.
    pub(super) fn receiver_itself(&self, expr: &Expr) -> bool {
        self.receiver && matches!(&expr.kind, ExprKind::Identifier(name) if name == "self")
    }

    /// Whether the body may write `self`'s fields.
    pub(super) const fn self_writable(&self) -> bool {
        use mojito_ast::ast::ArgConvention;
        matches!(
            self.self_convention,
            Some(
                ArgConvention::Mut
                    | ArgConvention::Var
                    | ArgConvention::Out
                    | ArgConvention::Deinit
            )
        )
    }

    /// Whether `name` is a scalar local.
    pub(super) fn local(&self, name: &str) -> bool {
        self.local_kind(name) == Some(LocalKind::Scalar)
    }

    /// Whether `name` is a `var` local of any type.
    pub(super) fn declared(&self, name: &str) -> bool {
        matches!(
            self.local_kind(name),
            Some(LocalKind::Scalar | LocalKind::Value)
        )
    }

    /// Whether `expr` names a `var` local holding a whole value, which a
    /// method call or `len` reads or writes in place. With no facts, a local
    /// taken as a scalar may hold a whole value too (`var m = self.meter`):
    /// its declaration's syntax alone cannot tell, and the check with facts
    /// judges its kind from the recorded types.
    pub(super) fn value_local(&self, expr: &Expr) -> bool {
        let ExprKind::Identifier(name) = &expr.kind else {
            return false;
        };
        match self.local_kind(name) {
            Some(LocalKind::Value) => true,
            Some(LocalKind::Scalar) => self.facts.is_none(),
            _ => false,
        }
    }

    /// Whether `expr` is a field of a `var` local holding a whole value. The
    /// local is the body's own, so the body reads and writes the field in
    /// place as it does a field of a writable `self`, and the field has its
    /// declared type under the local's recorded arguments in a template and a
    /// clone alike.
    pub(super) fn local_field(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Member { object, .. } if self.value_local(object))
    }

    /// Whether `expr` names a parameter holding a struct, such as `value` of a
    /// `value: Box[Self.T]` parameter: the parameter is bound to the
    /// instance's argument and read where it lies, as `self` is, and its
    /// recorded type is the declared one under the instance's arguments.
    pub(super) fn parameter_receiver(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Identifier(name)
                if (self.params.contains(&name.as_str())
                    && !self.callable_params.contains(&name.as_str())
                    && self.local_kind(name).is_none()))
            && self.nominal(expr)
    }

    /// Whether `expr` is a field of a parameter holding a struct, such as
    /// `entry._hash` of a `var entry` parameter: the parameter is bound to
    /// the instance's argument, and the field has its declared type under
    /// the parameter's recorded arguments in a template and a clone alike.
    pub(super) fn parameter_field(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if self.parameter_receiver(object))
    }

    /// Whether `name` is a `ref` local.
    pub(super) fn reference_local(&self, name: &str) -> bool {
        self.local_kind(name) == Some(LocalKind::Reference)
    }

    /// The innermost local of that name.
    pub(super) fn local_kind(&self, name: &str) -> Option<LocalKind> {
        self.locals
            .borrow()
            .iter()
            .rev()
            .find(|(local, _)| local == name)
            .map(|(_, kind)| *kind)
    }

    pub(super) fn expression(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
            // A `ref` local is read through its handle, as the scalar every
            // use site of `expression` also demands.
            ExprKind::Identifier(name) => match self.local_kind(name) {
                Some(kind) => !matches!(kind, LocalKind::Value | LocalKind::Mask),
                None => {
                    self.params.contains(&name.as_str())
                        || self.folded_value(expr)
                        || self.module_constant(name)
                }
            },
            // A field of `self`, admitted where its recorded type is a closed
            // scalar: every use site of `expression` also demands `scalar`.
            ExprKind::Member { .. } => {
                self.pack_length(expr)
                    || (self.receiver_field(expr)
                        || self.local_field(expr)
                        || self.parameter_field(expr)
                        || self.result_field(expr)
                        || self.reference_member(expr)
                        || self.struct_value(expr)
                        || self.struct_vector(expr)
                        || self.simd_intrinsic(expr))
                        && self.scalar(expr)
            }
            ExprKind::Index { .. } => {
                (self.tuple_element(expr) || self.simd_intrinsic(expr)) && self.scalar(expr)
            }
            ExprKind::MultiIndex { .. } => self.keyword_slice(expr),
            // A call of a method on `self`, on one of its fields, or on a
            // `var` local, passing scalars, whose recorded contract changes
            // per instance only in its target and its substituted result
            // (`closed_method_contract`).
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                self.method_call(expr, object, method, args, kwargs)
                    || self.slice_indices(expr, object, method, args, kwargs)
                    || self.static_call(expr, object, method, args, kwargs, false)
                    || self.consuming_call(expr, object, method, args, kwargs)
                    || self.bound_dispatch(expr, object, args, kwargs)
                    || self.bound_builtin(expr, object, method, args, kwargs)
                    || self.simd_intrinsic(expr)
                    || self.lane_float_method(expr, object, method, args, kwargs)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.parameterized_call(expr, callee, param_args, args, kwargs)
                    || self.simd_intrinsic(expr)
            }
            ExprKind::Prefix(_, value) => {
                (self.folding(expr) || !self.folding(value))
                    && self.expression(value)
                    && self.scalar(value)
            }
            // A folded operand is a literal in every instance. Over literals
            // alone an operator folds too, to a literal the template's `Int`
            // or `Float64` materializes or to a `Bool` ([`folded_literals`]);
            // anything else keeps a runtime value on the operand's other
            // side.
            ExprKind::Infix(op, left, right) => {
                let runtime = |operand: &Expr| {
                    !self.folding(operand)
                        && self.facts.is_none_or(|facts| {
                            fact_at(&facts.expression_types, self.occurrence(operand)).is_some_and(
                                |ty| grammar_scalar(ty) || self.value_shaped_scalar(ty),
                            )
                        })
                };
                let folds = self.folding(expr)
                    || ((!self.folding(left) || runtime(right))
                        && (!self.folding(right) || runtime(left)));
                // A comparison over a value-shaped operand is a
                // `SIMD[DType.bool, 1]` mask while the lane is a vector, but
                // a `Bool` where it folds to a native scalar
                // (`Scalar[DType.int]` is `Int`): its value is admitted only
                // where each instance reads it alike
                // ([`Self::lane_comparison`]).
                let lane_comparison = comparison(*op)
                    && [left, right].iter().any(|operand| {
                        self.facts.is_some_and(|facts| {
                            fact_at(&facts.expression_types, self.occurrence(operand))
                                .is_some_and(|ty| self.value_shaped_scalar(ty))
                        })
                    });
                !lane_comparison
                    && ((folds
                        && self.expression(left)
                        && self.expression(right)
                        && self.scalar(left)
                        && self.scalar(right))
                        || self.operator(expr, *op, left, right))
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                let id = self.occurrence(expr);
                let known = self.facts.is_none_or(|facts| {
                    facts.call_parameters.iter().any(|(call, _)| *call == id)
                        || (facts.builtin_len_calls.contains(&id) && args.len() == 1)
                });
                if self.callable_params.contains(&name.as_str()) {
                    return self.callable_call(id, param_args, args, kwargs, known);
                }
                if self.callable_binders.contains(&name.as_str()) && self.local_kind(name).is_none()
                {
                    return self.callable_binder_call(id, param_args, args, kwargs, known);
                }
                if name == "_unqualified_type_name" || name == "repr" {
                    return self.string_builtin(id, name, param_args, args, kwargs);
                }
                if self.scalar_conversion(id, name, param_args, args, kwargs)
                    || self.simd_construction(id, name, args, kwargs)
                    || self.foreign_call(id, name, param_args, args, kwargs)
                {
                    return true;
                }
                // The built-in `len` reads its operand in place and realizes
                // its witness per instance, so a method may hand it a field of
                // `self`, a `var` local, or a parameter holding a struct or its
                // field, of any type, not only a scalar one.
                let builtin_len = self
                    .facts
                    .is_none_or(|facts| facts.builtin_len_calls.contains(&id));
                // A generic callee applied to types only
                // (`unsafe_alloc[Self.T](n)`) records its application,
                // which the instance substitutes.
                let applied = param_args.is_empty()
                    || (param_args.iter().all(type_argument)
                        && !self.structs.contains_key(name)
                        && self.facts.is_none_or(|facts| {
                            fact_at(&facts.generic_instantiations, id).is_some()
                        }));
                known && applied && kwargs.is_empty() && args.iter().all(|argument| {
                    let held = matches!(&argument.kind, ExprKind::Identifier(name)
                            if self.reference_local(name));
                    let on_self = self.receiver
                        && matches!(&argument.kind, ExprKind::Identifier(name) if name == "self");
                    self.expression(argument)
                        || (builtin_len
                            && (held
                                || on_self
                                || self.receiver_field(argument)
                                || self.value_local(argument)
                                || self.parameter_receiver(argument)
                                || self.parameter_field(argument)))
                        || self.direct_call_value(argument)
                        || self.generic_call_place(id, argument)
                })
            }
            _ => false,
        }
    }

    /// A template occurrence is always copy zero.
    pub(super) fn occurrence(&self, expr: &Expr) -> OccurrenceId {
        OccurrenceId {
            syntax: self.origins.origin(expr.syntax_id),
            copy: 0,
        }
    }

    pub(super) fn occurrence_of(&self, statement: &Stmt) -> OccurrenceId {
        OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        }
    }

    /// Whether the recorded type of `expr` mentions no parameter.
    pub(super) fn closed(&self, expr: &Expr) -> bool {
        self.closed_value(self.occurrence(expr))
    }

    /// Whether the type recorded at `id` mentions no parameter.
    pub(super) fn closed_value(&self, id: OccurrenceId) -> bool {
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && !mojito_types::types::is_symbolic(ty))
        })
    }

    /// Whether the recorded type of `expr` is a closed scalar or a
    /// value-shaped vector ([`Self::value_shaped_scalar`]).
    /// With no facts yet, the syntax alone never rules a type out.
    pub(super) fn scalar(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts.expression_types.iter().any(|(site, ty)| {
                *site == id && (grammar_scalar_or_literal(ty) || self.value_shaped_scalar(ty))
            })
        })
    }

    /// A value-shaped vector a body holds as a scalar (`SIMD[DType.int64,
    /// w]`, over a keyed `def`'s or a method's own binders, or a struct
    /// lane): every instance folds the binders its slots
    /// name, and an operator, a reduction, or a lane read over it records
    /// nothing its dimensions decide, so the instance's facts are the
    /// template's under the folded dimensions.
    pub(super) fn value_shaped_scalar(&self, ty: &Ty) -> bool {
        self.value_shaped_simd(ty) || self.struct_lane_simd(ty)
    }

    /// A `SIMD` type whose open slots name only the struct's lane binders,
    /// which every per-instantiation clone folds, and
    /// the declaration's own value binders, folded as a keyed body's are
    /// (`SIMD[dt, Self.n]` in `rep[dt: DType]` of `Width[n: Int]`).
    pub(super) fn struct_lane_simd(&self, ty: &Ty) -> bool {
        let mut named = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut named);
        matches!(ty, Ty::Simd { .. })
            && named
                .iter()
                .any(|name| self.struct_lanes.contains(&name.as_str()))
            && named.iter().all(|name| {
                self.struct_lanes.contains(&name.as_str()) || self.values.contains(&name.as_str())
            })
    }

    /// Whether the recorded type of `expr` is a value-shaped vector the
    /// body holds ([`Self::value_shaped_scalar`]).
    pub(super) fn value_shaped(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, id).is_some_and(|ty| self.value_shaped_scalar(ty))
        })
    }
    fn statement(&self, statement: &Stmt) -> bool {
        match &statement.kind {
            StmtKind::Return(value) if self.reference_result.is_some() => value
                .as_ref()
                .is_some_and(|value| self.returned_place(value)),
            StmtKind::Return(Some(value)) => {
                (self.expression(value) && self.scalar(value))
                    || self.moved_result.is_some_and(|result| {
                        (self.whole_value(value)
                            || self.reference_read(value)
                            || self.tuple_element_value(value))
                            && self.typed(value, result)
                    })
            }
            StmtKind::Pass => true,
            // An unrolled body is copied once per iteration, every copy
            // sharing the bindings around the loop; a local declared inside
            // is one binding per copy (`renumber_locals`). The loop variable
            // folds to a literal wherever it survives, which `folded_value`
            // admits where the literal's facts are the name's.
            StmtKind::ComptimeFor { var, body, .. } if self.keyed => {
                self.loop_vars.borrow_mut().push(var.clone());
                let admitted = self.block(body);
                self.loop_vars.borrow_mut().pop();
                admitted
            }
            StmtKind::VarDecl { name, ty, value } if self.keyed => {
                let scalar =
                    (self.expression(value) && self.scalar(value)) || self.simd_value(value);
                let mask = !scalar && ty.is_none() && self.lane_comparison(value);
                let element = !scalar && !mask && ty.is_none() && self.element_construction(value);
                let kind = if mask {
                    LocalKind::Mask
                } else if element {
                    LocalKind::Value
                } else {
                    LocalKind::Scalar
                };
                self.locals.borrow_mut().push((name.clone(), kind));
                scalar || mask || (element && self.holds(MethodFeatures::ELEMENT_CONSTRUCTIONS))
            }
            // A condition is typed, never evaluated, by the check that
            // produced these facts. A `def`'s template keeps the region and
            // its branch carries the condition to the elaborator; a
            // validated method keeps its compile-time control flow too, and
            // an instance keeps the arms the elaborator selected, once per
            // unrolled copy, and drops the rest with their facts
            // (`COMPTIME_CONTROL`).
            StmtKind::ComptimeIf { branches, orelse } => {
                branches
                    .iter()
                    .map(|(_, arm)| arm)
                    .chain(orelse)
                    .all(|arm| self.block(arm))
                    && (self.keyed
                        || (self.holds(MethodFeatures::STATEMENTS)
                            && self.holds(MethodFeatures::COMPTIME_CONTROL)))
            }
            StmtKind::ComptimeFor { var, body, .. } => {
                self.loop_vars.borrow_mut().push(var.clone());
                let admitted = self.block(body);
                self.loop_vars.borrow_mut().pop();
                admitted
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::COMPTIME_CONTROL)
            }
            // A runtime statement is checked once, whatever runs it, so it
            // neither drops nor copies an occurrence. A scalar local is one
            // binding wherever it is declared. An annotated local holds its
            // declared type, which the value may convert to.
            StmtKind::VarDecl { name, ty, value } if !self.keyed => {
                let closed =
                    (self.expression(value) && self.scalar(value)) || self.lane_local_value(value);
                let scalar = closed && (ty.is_none() || self.scalar_binding(value));
                let mask = !scalar && ty.is_none() && self.lane_comparison(value);
                let moved = !scalar
                    && !mask
                    && self.moved_result.is_some()
                    && (closed
                        || self.whole_value(value)
                        || self.simd_value(value)
                        || self.reference_read(value)
                        || self.tuple_element_value(value)
                        || (ty.is_none()
                            && self.element_construction(value)
                            && self.holds(MethodFeatures::ELEMENT_CONSTRUCTIONS))
                        || (ty.is_some() && self.converted_place(value)))
                    && (ty.is_none() || self.annotated_binding(value));
                let kind = if scalar {
                    LocalKind::Scalar
                } else if mask {
                    LocalKind::Mask
                } else {
                    LocalKind::Value
                };
                self.locals.borrow_mut().push((name.clone(), kind));
                (scalar || mask || moved) && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::RefDecl { name, value } if !self.keyed => {
                let bound = self.bound_place(statement, value);
                self.locals
                    .borrow_mut()
                    .push((name.clone(), LocalKind::Reference));
                bound && self.holds(MethodFeatures::REFERENCE_LOCALS)
            }
            StmtKind::Raise(value) if !self.keyed => {
                self.raised(value) && self.holds(MethodFeatures::RAISES)
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue if !self.keyed => {
                self.holds(MethodFeatures::STATEMENTS)
            }
            // A runtime `if` is checked once, or once per unrolled copy in
            // a keyed body, each copy's condition folding its loop variable
            // as the copy's other statements do.
            StmtKind::If { branches, orelse } => {
                branches
                    .iter()
                    .all(|(condition, arm)| self.condition(condition) && self.block(arm))
                    && orelse.as_ref().is_none_or(|arm| self.block(arm))
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
            }
            // A runtime loop in a method selects its iterator protocol from
            // the iterable's type, which an instance selects again. The loop
            // variable is one local wherever the loop runs, of the kind its
            // recorded binding type makes it.
            StmtKind::For {
                var,
                iter,
                body,
                orelse: None,
                ..
            } if !self.keyed
                && (self.moved_result.is_some() || self.reference_result.is_some()) =>
            {
                let iterable = self.iterable(iter);
                let kind = self.loop_local(statement);
                self.locals.borrow_mut().push((var.clone(), kind));
                iterable
                    && self.block(body)
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::ITERATION)
            }
            StmtKind::With { items, body } if !self.keyed && self.moved_result.is_some() => {
                self.with_statement(statement, items, body)
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::WITH_STATEMENTS)
            }
            // A `with` desugar's guard: the body, then its cleanup, and for
            // an error exit a handler binding the body's error.
            StmtKind::Try {
                body,
                except,
                orelse: None,
                finalbody: Some(finalbody),
            } if self.desugar_depth.get() > 0 => {
                self.block(body)
                    && except.as_ref().is_none_or(|(binder, handler)| {
                        let Some(binder) = binder else {
                            return false;
                        };
                        let scope = self.locals.borrow().len();
                        self.locals
                            .borrow_mut()
                            .push((binder.clone(), LocalKind::Value));
                        self.error_binders.borrow_mut().push(binder.clone());
                        let admitted = self.block(handler);
                        self.error_binders.borrow_mut().pop();
                        self.locals.borrow_mut().truncate(scope);
                        admitted
                    })
                    && self.block(finalbody)
            }
            // A runtime guard: the body, then a handler, bare or binding
            // the body's error, which it may raise again.
            StmtKind::Try {
                body,
                except: Some((binder, handler)),
                orelse: None,
                finalbody: None,
            } if !self.keyed && self.moved_result.is_some() => {
                let scope = self.locals.borrow().len();
                let admitted = self.block(body) && {
                    if let Some(binder) = binder {
                        self.locals
                            .borrow_mut()
                            .push((binder.clone(), LocalKind::Value));
                        self.error_binders.borrow_mut().push(binder.clone());
                    }
                    let handled = self.block(handler);
                    if binder.is_some() {
                        self.error_binders.borrow_mut().pop();
                    }
                    handled
                };
                self.locals.borrow_mut().truncate(scope);
                admitted
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::TRY_STATEMENTS)
            }
            StmtKind::Unpack {
                targets,
                value,
                declares,
            } if !self.keyed => {
                self.tuple_unpack(targets, value, *declares)
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            // A runtime `while` is checked once, or once per unrolled copy in
            // a keyed body, as a runtime `if` is.
            StmtKind::While {
                cond,
                body,
                orelse: None,
            } if self.keyed => {
                self.runtime_loops.set(self.runtime_loops.get() + 1);
                let admitted = self.condition(cond) && self.block(body);
                self.runtime_loops.set(self.runtime_loops.get() - 1);
                admitted
            }
            StmtKind::Break | StmtKind::Continue if self.keyed => self.runtime_loops.get() > 0,
            StmtKind::While {
                cond,
                body,
                orelse: None,
            } => self.condition(cond) && self.block(body) && self.holds(MethodFeatures::STATEMENTS),
            StmtKind::SetPlace { place, value } if self.element_initialization(place, value) => {
                self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::ELEMENT_CONSTRUCTIONS)
            }
            // A scalar field of a writable `self`: the store is a plain
            // scalar write, never an in-place operator of the field's type.
            StmtKind::SetPlace { place, value } if !self.keyed => {
                let scalar = (self.scalar_field_place(place) || self.reference_element(place))
                    && self.expression(value)
                    && self.scalar(value);
                (scalar || self.whole_store(place, value) || self.element_store(place, value))
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            // A discarded value. That it is not read is the statement's
            // syntax; what the call itself recorded is the call's to answer.
            StmtKind::Expr(value) => {
                let call = matches!(
                    value.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Invoke { .. }
                ) && self.expression(value)
                    && self.closed(value);
                call || (self.moved_result.is_some() && self.pointer_statement(value))
                    || (!self.keyed && self.abort(value))
                    || (self.desugar_depth.get() > 0 && self.keep_alive(value))
                    || (self.print_call(value)
                        && (self.keyed || self.holds(MethodFeatures::STATEMENTS)))
            }
            // A discarded value: a closed scalar, or a whole value copied or
            // moved out as any other ([`Self::whole_value`]) and destroyed
            // at the instance's type as any temporary is.
            StmtKind::Assign { name, value } if name == "_" => {
                (self.expression(value) && self.scalar(value))
                    || (!self.keyed
                        && self.whole_value(value)
                        && self.holds(MethodFeatures::STATEMENTS))
            }
            // A scalar local takes a closed scalar. Any `var` local may be
            // rebound whole to a value of its own type: the old value's
            // destruction is the local's, and the new one is moved or a
            // temporary, converted by nothing an instance selects.
            StmtKind::Assign { name, value } if self.declared(name) => {
                let scalar = self.local(name) && self.expression(value) && self.scalar(value);
                let rebound = !self.keyed
                    && self.moved_result.is_some()
                    && self.whole_value(value)
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.conversions, self.occurrence(value)).is_none()
                    });
                (scalar || rebound) && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
            }
            // A whole store to a `mut` parameter, of the parameter's own type.
            StmtKind::Assign { name, value } => {
                self.mut_params.contains(&name.as_str())
                    && self.parameter_store(value)
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::AugAssign { place, value, .. } => {
                let local = matches!(&place.kind, ExprKind::Identifier(name)
                    if self.local(name) || self.mut_params.contains(&name.as_str()));
                let scalar = (local
                    || (!self.keyed
                        && (self.scalar_field_place(place)
                            || self.reference_element(place)
                            || self.setter_element(place))))
                    && self.scalar(place)
                    && self.expression(value)
                    && self.scalar(value)
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS));
                scalar
                    || (!self.keyed
                        && (self.inplace_element(place, value) || self.inplace_place(place, value))
                        && self.holds(MethodFeatures::STATEMENTS))
            }
            _ => false,
        }
    }

    /// A condition that reads a parameter, a local, or a field of `self`
    /// whole and tests it through `__bool__`.
    ///
    /// The read records only the place's type and binding, and the mark is
    /// decided by that type alone, which an instance judges again
    /// (`realize_instance_facts`). Nothing else may be recorded at the
    /// condition: it is neither copied nor converted.
    fn truthiness_condition(&self, expr: &Expr) -> bool {
        let place = match &expr.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.declared(name)
                    || self.reference_local(name)
                    || self.local_kind(name) == Some(LocalKind::Mask)
            }
            ExprKind::Member { .. } => self.receiver_field(expr) || self.reference_member(expr),
            _ => false,
        };
        let id = self.occurrence(expr);
        let admitted = place
            && self.facts.is_none_or(|facts| {
                facts.truthiness_conditions.contains(&id)
                    && !facts.copy_place_value_uses.contains(&id)
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            });
        admitted && self.holds(MethodFeatures::TRUTHINESS)
    }

    /// `Self.<v>` of a closed vector value binder (`Self.key`), which every
    /// per-instantiation clone folds to its vector's construction under the name's
    /// identity (`construct_folded_vectors`): the template typed it as that
    /// closed vector and recorded nothing else there.
    fn struct_vector(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Member { object, field }
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && self.struct_vectors.contains(&field.as_str()))
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                fact_at(&facts.expression_bindings, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.expression_place_types, id).is_none()
            })
    }

    /// Whether the body owns `self` whole and may transfer it out.
    const fn self_owned(&self) -> bool {
        matches!(
            self.self_convention,
            Some(mojito_ast::ast::ArgConvention::Var)
        )
    }

    /// Whether `expr` is a field of a call's whole-value result holding a
    /// struct, such as `self.bumped().count`: the call's result is a
    /// temporary of the contract's substituted type, and the field has its
    /// declared type under that type's arguments in a template and a clone
    /// alike.
    fn result_field(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if self.call_result(object) && self.nominal(object))
    }

    /// Whether the recorded type of `expr` is a struct, whose fields a body
    /// may read. With no facts yet, the syntax alone never rules it out.
    fn nominal(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && matches!(ty, Ty::Struct(..)))
        })
    }
}

const fn grammar_scalar_or_literal(ty: &Ty) -> bool {
    grammar_scalar(ty) || matches!(ty, Ty::IntLiteral | Ty::FloatLiteral)
}
