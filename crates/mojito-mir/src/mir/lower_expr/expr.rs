//! The `expr_unconverted` expression dispatcher.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Flatten<'_> {
    pub(in crate::mir) fn expr_unconverted(&mut self, e: &Expr) -> Reg {
        match &e.kind {
            // --- Literals ------------------------------------------------------
            ExprKind::Int(n) => self.constant(e, Const::IntLiteral(n.clone())),
            ExprKind::Float(x) => self.constant(e, Const::FloatLiteral(x.clone())),
            ExprKind::Bool(b) => self.constant(e, Const::Bool(*b)),
            ExprKind::Str(s) => self.constant(e, Const::Str(s.clone())),
            ExprKind::None => self.constant(e, Const::None),
            ExprKind::Uninitialized => self.constant(e, Const::None),
            // The `p[]` dereference marker: the checker validated the pointer
            // receiver, and offset 0 is the whole lowering.
            ExprKind::EmptySubscript => self.constant(e, Const::Int(0)),
            ExprKind::Spread(_) => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(
                    "unexpanded call spread reached MIR lowering".to_string(),
                ));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }

            // --- Variable reads ------------------------------------------------
            // A bare read defaults to `Copy` — the lifecycle copy for owned
            // storage. Consuming conventions keep it; a read-convention call
            // argument is instead bound as a shallow place read where the
            // checker marked `BorrowReadArgument` (`lower_call_argument`), and
            // `x^` lowers as `Move` below.
            ExprKind::Identifier(name) => self.identifier_read(e, name),
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            ExprKind::Transfer(inner) => self.transfer(e, inner),

            // --- Operators -----------------------------------------------------
            ExprKind::Prefix(op, a) => {
                let ra = self.expr(a);
                // `not x` on a struct or a numeric scalar is
                // `not x.__bool__()`: convert through the explicit `Bool(x)`
                // call so the negation is scalar on both backends.
                let ra = if *op == PrefixOp::Not
                    && self.checked_ty(a).is_some_and(|ty| {
                        matches!(ty, Ty::Struct(..))
                            || mojito_types::conformance::is_numeric(&ty)
                            || mojito_types::types::is_scalar_simd(&ty)
                    }) {
                    self.bool_conversion(a.source_span(), ra)
                } else {
                    ra
                };
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::UnOp {
                    op: *op,
                    dest: d,
                    a: ra,
                });
                d
            }
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), a, b) => {
                self.short_circuit(*op, a, b, span(e))
            }
            // A checked nominal membership operation is an ordinary borrowed
            // `container.__contains__(value)` call.  Keeping it as a value-only
            // `BinOp` loses the receiver place; the VM would then install a
            // shallow struct value in the callee's `self` slot and destroy its
            // owned fields on return.  For pointer-backed collections that can
            // free the caller's storage.  Preserve source evaluation order
            // (value before container), the selected overload, and the normal
            // method-call place/capture contract.
            ExprKind::Infix(op @ (InfixOp::In | InfixOp::NotIn), value, container)
                if matches!(self.checked_ty(container), Some(Ty::Struct(..))) =>
            {
                self.contains_call(e, *op, value, container)
            }
            ExprKind::Infix(op, a, b) => self.infix_op(e, *op, a, b),

            // --- Calls / access ------------------------------------------------
            // A pack or reflection query read as a runtime value is a
            // parameter constant; its operand is a type, never lowered.
            ExprKind::Call { .. }
            | ExprKind::Invoke { .. }
            | ExprKind::Member { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Index { .. }
                if let Some(value) = self.param_value(e) =>
            {
                self.param_value_register(e, value)
            }
            // NOTE: keyword args + default-slot matching (`call::match_call_slots`)
            // are a follow-up; the checker has already validated them, so only the
            // positional `args` are flattened here.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => self.call_expr(e, name, param_args, args, kwargs),
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => self.invoke_expr(e, callee, param_args, args, kwargs),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => self.method_call_expr(e, object, method, args, kwargs),
            ExprKind::Member { object, field } => self.member_expr(e, object, field),
            // A variant projection spelled with a struct-name index
            // (`v[String]`) carries the same checked adjustment as the
            // type-token spelling below and lowers identically.
            ExprKind::Index { object, .. }
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
                    )
                }) =>
            {
                self.variant_project_index(e, object)
            }
            ExprKind::Index { object, index } => self.index_expr(e, object, index),

            // --- Aggregates ----------------------------------------------------
            ExprKind::ListLit(elems) => self.list_literal(e, elems),
            ExprKind::BraceLit(entries) => self.brace_literal(e, entries),
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => self.comprehension(e, *kind, key.as_deref(), value, clauses),
            ExprKind::TupleLit(elems) => self.tuple_literal(e, elems),

            // Walrus `:=` reaches MIR after type checking. Preserve an explicit
            // unsupported boundary rather than assigning accidental semantics.
            ExprKind::Named { name, value } => {
                let value = self.expr(value);
                let var = self.var(name);
                self.emit(MirInstr::DefVar {
                    var,
                    src: value,
                    binding_ty: None,
                });
                value
            }
            // Ternary `a if cond else b` — a value-producing branch (like the
            // short-circuit lowering, but both arms assign the result).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => self.ternary(cond, then_branch, else_branch, span(e)),
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            ExprKind::Compare { first, rest } => self.compare_chain(first, rest, span(e)),
            // Slice `object[lower:upper:step]` → a new List/String.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => self.slice_expr(
                e,
                object,
                lower.as_deref(),
                upper.as_deref(),
                step.as_deref(),
            ),
            ExprKind::MultiIndex { object, args } => self.multi_index_expr(e, object, args),
            // The production discovery path rewrites every concrete `t"…"`
            // occurrence into its lazy `TString` specialization's construction
            // before MIR, so this arm is the output-identical eager fallback:
            // the stage-composed seam (which skips discovery by design) and
            // t-strings inside retained abstract bound-generic bodies lower to
            // `"" + String(part) + …` concatenation here.
            ExprKind::TString { parts, .. } => self.tstring_concat(e, parts),
            ExprKind::TypeApply { name, .. }
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
                    )
                }) =>
            {
                self.variant_project_type_apply(e, name)
            }
            ExprKind::TypeApply { name, .. } if self.nested_info(e).is_some() => {
                self.nested_type_apply(e, name)
            }
            ExprKind::TypeApply { .. } if self.resolved_callable(e).is_some() => self.constant(
                e,
                Const::Function(
                    self.resolved_callable(e)
                        .expect("checked callable TypeApply has a lowered target"),
                ),
            ),
            // A lambda expression materializes its hidden definition's
            // closure at the expression's evaluation point — copy/move
            // captures snapshot here, once per evaluation.
            ExprKind::Lambda { .. } => {
                if let Some(info) = self.nested_info(e) {
                    self.emit_nested_closure(&info, span(e), false)
                } else {
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::Unsupported(
                        "lambda expression lost its checked nested declaration".to_string(),
                    ));
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    dest
                }
            }
            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. } => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked expression reached MIR lowering: {:?}",
                    e.kind
                )));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }
        }
    }

    pub(super) fn identifier_read(&mut self, e: &Expr, name: &str) -> Reg {
        if let Some(target) = self.resolved_callable(e) {
            return self.constant(e, Const::Function(target));
        }
        if let Some(info) = self.nested_info(e) {
            return self.load_nested_closure(name, &info, span(e));
        }
        if !self.vars.iter().any(|candidate| candidate == name) && self.overloads.is_function(name)
        {
            return self.constant(e, Const::Function(name.to_owned()));
        }
        let var = self.expression_var(name, e);
        let d = self.fresh(span(e), Some(var));
        if self.is_origin_bearing_pointer(e) {
            // A pointer variable's read is its pointer value: both
            // backends read a pointer-typed slot as the handle it
            // stores (the VM keys `UseVar` on the variable's pointer
            // type), so a copy into another local, a binding, or a
            // call argument carries the pointee's address rather than
            // a handle to this slot.
            self.emit(MirInstr::UseVar {
                dest: d,
                var,
                mode: UseMode::Copy,
            });
            return d;
        }
        if let Some(loan) = self.aliases.get(&var).cloned() {
            let mut place = loan.place;
            place.through = Some(var);
            self.emit(MirInstr::LoadPlace { dest: d, place });
        } else if self.runtime_aliases.contains(&var) {
            let handle = self.fresh(e.source_span(), Some(var));
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place: {
                    let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                    place.through = Some(var);
                    place
                },
            });
            self.emit(MirInstr::ReadRef {
                dest: d,
                reference: handle,
            });
        } else {
            self.emit(MirInstr::UseVar {
                dest: d,
                var,
                mode: UseMode::Copy,
            });
            return d;
        }
        // A checked value-copy of an alias-bound variable (a borrowed
        // loop binding consumed by a `var` argument) must run the
        // referent's `__copyinit__` rather than alias its owning
        // storage — the alias slot's referent stays live in its
        // collection. Ordinary variables keep the `UseVar` path above:
        // their copy/move lifecycle is drop-elaborated.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) && !matches!(self.checked_ty(e), Some(Ty::Ref(_)))
        {
            // The copy has the read register's exact type: the
            // variable's recorded type can keep a literal element
            // (`var t = 4, "four"` binds `Tuple[IntLiteral, ...]`)
            // where the identifier's checked type defaulted it, and
            // the verifier compares the two for equality.
            let ty = self
                .f
                .reg_types
                .get(&d.0)
                .cloned()
                .or_else(|| self.checked_ty(e))
                .unwrap_or(Ty::Error);
            let copied = self.fresh_typed(span(e), Some(var), ty);
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: d,
            });
            return copied;
        }
        d
    }

    pub(in crate::mir) fn transfer(&mut self, e: &Expr, inner: &Expr) -> Reg {
        if let ExprKind::Identifier(name) = &inner.kind {
            let var = self.expression_var(name, inner);
            // A `mut` parameter's slot holds the handle to the caller's
            // storage, so the transfer takes the referent out through it. A
            // value that owns no storage transfers as a copy and leaves the
            // caller's value in place.
            if (var as usize) < self.f.n_params
                && self.runtime_aliases.contains(&var)
                && !self.pointer_valued_slot(var)
            {
                let ty = self.var_types.get(&var).cloned();
                if !ty
                    .as_ref()
                    .is_some_and(crate::mir::calls::owns_droppable_storage)
                {
                    return self.expr(inner);
                }
                let d = self.fresh(span(e), Some(var));
                let mut place = MirPlace::root(var, ty);
                place.through = Some(var);
                self.emit(MirInstr::MovePlace { dest: d, place });
                return d;
            }
            let d = self.fresh(span(e), Some(var));
            self.emit(MirInstr::UseVar {
                dest: d,
                var,
                mode: UseMode::Move,
            });
            d
        } else if let Some(place) = self.pure_field_place(inner) {
            let d = self.fresh(span(e), Some(place.root));
            self.emit(MirInstr::MovePlace { dest: d, place });
            d
        } else if let ExprKind::Index { object, .. } = &inner.kind
            && matches!(self.checked_ty(object), Some(Ty::Tuple(_)))
            && let Some(place) = self.try_place(inner)
        {
            let d = self.fresh(span(e), Some(place.root));
            self.emit(MirInstr::MovePlace { dest: d, place });
            d
        } else {
            self.expr(inner)
        }
    }

    pub(super) fn contains_call(
        &mut self,
        e: &Expr,
        op: InfixOp,
        value: &Expr,
        container: &Expr,
    ) -> Reg {
        let (argument, arg_place) = self.lower_call_argument(value);
        let (recv, recv_place) = self.lower_call_receiver(container);
        let contains = self.fresh_typed(span(e), None, Ty::Bool);
        self.emit_interior_invalidations(container, None);
        self.emit_call_invalidations(e, std::slice::from_ref(value), &[]);
        self.emit(MirInstr::MethodCall {
            dest: contains,
            recv,
            method: "__contains__".to_string(),
            resolved: self.resolved_callable(e),
            raises: self.checked_raises(e),
            reference_result: None,
            result_adapter: None,
            args: vec![argument],
            kwargs: Vec::new(),
            recv_place,
            recv_writes: self.receiver_writes(e),
            arg_places: vec![arg_place],
            kwarg_places: Vec::new(),
            capture_accesses: self.checked_call_capture_accesses(e),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: self.instantiated_args(e),
            spread: None,
        });
        self.emit_nested_closure_argument_keepalives(std::slice::from_ref(value), &[]);
        if matches!(op, InfixOp::NotIn) {
            let dest = self.fresh_typed(span(e), None, Ty::Bool);
            self.emit(MirInstr::UnOp {
                op: PrefixOp::Not,
                dest,
                a: contains,
            });
            dest
        } else {
            contains
        }
    }

    pub(super) fn infix_op(&mut self, e: &Expr, op: InfixOp, a: &Expr, b: &Expr) -> Reg {
        // Operands left-to-right (evaluation order is explicit).
        let ra = self.lower_operator_operand(a);
        let rb = self.lower_operator_operand(b);
        let resolved = self.resolved_callable(e);
        // A reflected operator (checker-marked) runs on the RIGHT
        // operand: `1 + m` calls `m.__radd__(1)`.
        let reflected = self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ReflectedOperator
            )
        });
        let (ra, rb) = if reflected { (rb, ra) } else { (ra, rb) };
        // Equatable's default `__ne__` (checker-marked): dispatch
        // `__eq__` and negate its result.
        if op == InfixOp::Ne
            && self.checked_adjustments(e).iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::NegatedEquality
                )
            })
        {
            let equal = self.fresh_typed(span(e), None, Ty::Bool);
            self.emit(MirInstr::BinOp {
                op: InfixOp::Eq,
                dest: equal,
                a: ra,
                b: rb,
                resolved,
            });
            let dest = self.fresh_typed(span(e), None, Ty::Bool);
            self.emit(MirInstr::UnOp {
                op: PrefixOp::Not,
                dest,
                a: equal,
            });
            return dest;
        }
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::BinOp {
            op,
            dest: d,
            a: ra,
            b: rb,
            resolved,
        });
        d
    }

    pub(super) fn list_literal(&mut self, e: &Expr, elems: &[Expr]) -> Reg {
        if let Some((target, constructor)) = self.array_literal_plan(e) {
            // One nominal constructor call: the element registers are
            // the variadic arguments, `__list_literal__` selects the
            // literal overload, and the `length` parameter argument
            // reifies the constructed value's `value_params`.
            let regs = self.args(elems);
            let none = self.fresh_typed(span(e), None, Ty::None);
            self.emit(MirInstr::Const {
                dest: none,
                k: Const::None,
            });
            let length = mojito_types::types::array_parts(&target)
                .map_or(elems.len() as i64, |(_, length)| length);
            let length_reg = self.fresh_typed(span(e), None, Ty::Int);
            self.emit(MirInstr::Const {
                dest: length_reg,
                k: Const::Int(length),
            });
            let d = self.fresh_typed(span(e), None, target);
            self.emit(MirInstr::Call {
                dest: d,
                func: FuncRef::named(&constructor),
                raises: None,
                args: regs.clone(),
                kwargs: vec![("__list_literal__".to_string(), none)],
                arg_places: vec![None; regs.len()],
                kwarg_places: vec![None],
                capture_accesses: Vec::new(),
                param_arg_regs: vec![
                    MirParamArg {
                        name: None,
                        value: None,
                        binder: None,
                        expr: None,
                    },
                    MirParamArg {
                        name: None,
                        value: Some(length_reg),
                        binder: None,
                        expr: None,
                    },
                ],
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
            });
            return d;
        }
        if let Some((target, Some(insert))) = self.collection_plan(e) {
            let collection = self.begin_nominal_collection(e, &target);
            for element in elems {
                let value = self.expr(element);
                self.insert_nominal_collection(element, collection, &target, &insert, vec![value]);
            }
            return self.finish_nominal_collection(e, collection, &target);
        }
        // The unchecked CFG helper has no semantic facts. Keep it
        // syntax-total by emitting an ordinary constructor call; the
        // production checked path above always carries an exact target
        // and insertion method.
        let regs = self.args(elems);
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::Call {
            dest: d,
            func: FuncRef::named("List"),
            raises: None,
            args: regs.clone(),
            kwargs: Vec::new(),
            arg_places: vec![None; regs.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        d
    }

    pub(super) fn brace_literal(&mut self, e: &Expr, entries: &[(Expr, Option<Expr>)]) -> Reg {
        if let Some((target, Some(insert))) = self.collection_plan(e) {
            let collection = self.begin_nominal_collection(e, &target);
            let dictionary = dict_elements(&target).is_some();
            for (key, value) in entries {
                let key = self.expr(key);
                let mut arguments = vec![key];
                if dictionary {
                    arguments.push(
                        self.expr(
                            value
                                .as_ref()
                                .expect("checked dictionary display has paired values"),
                        ),
                    );
                }
                self.insert_nominal_collection(e, collection, &target, &insert, arguments);
            }
            return self.finish_nominal_collection(e, collection, &target);
        }
        // As above, this is only the syntax-only CFG compatibility
        // path. A verified program never guesses its collection kind.
        let dictionary = entries.first().is_none_or(|(_, value)| value.is_some());
        let d = self.fresh(span(e), None);
        let regs = if dictionary {
            entries
                .iter()
                .flat_map(|(key, value)| {
                    let key = self.expr(key);
                    let value = value.as_ref().map(|value| self.expr(value));
                    std::iter::once(key).chain(value)
                })
                .collect::<Vec<_>>()
        } else {
            entries
                .iter()
                .map(|(key, _)| self.expr(key))
                .collect::<Vec<_>>()
        };
        self.emit(MirInstr::Call {
            dest: d,
            func: FuncRef::named(if dictionary { "Dict" } else { "Set" }),
            raises: None,
            args: regs.clone(),
            kwargs: Vec::new(),
            arg_places: vec![None; regs.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        d
    }

    pub(super) fn tuple_literal(&mut self, e: &Expr, elems: &[Expr]) -> Reg {
        if let Some((target, None)) = self.collection_plan(e)
            && let Ty::Struct(name, _) = &target
        {
            let regs = self.args(elems);
            let dest = self.fresh_typed(span(e), None, target.clone());
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(name),
                raises: None,
                args: regs.clone(),
                kwargs: Vec::new(),
                arg_places: vec![None; regs.len()],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
            });
            return dest;
        }
        // Syntax-only lowering cannot select a variadic specialization,
        // but it still emits an ordinary public constructor call. The
        // private `MakeTuple` opcode is reserved for `__RuntimeTuple`.
        let regs = self.args(elems);
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::Call {
            dest: d,
            func: FuncRef::named("Tuple"),
            raises: None,
            args: regs.clone(),
            kwargs: Vec::new(),
            arg_places: vec![None; regs.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        d
    }

    pub(super) fn tstring_concat(&mut self, e: &Expr, parts: &[TStringPart]) -> Reg {
        let mut result = self.fresh(span(e), None);
        self.emit(MirInstr::Const {
            dest: result,
            k: Const::Str(String::new()),
        });
        for part in parts {
            let piece = match part {
                TStringPart::Literal(text) => {
                    let register = self.fresh(span(e), None);
                    self.emit(MirInstr::Const {
                        dest: register,
                        k: Const::Str(text.clone()),
                    });
                    register
                }
                TStringPart::Expr(value) => {
                    let argument = self.expr(value);
                    // Interpolation's implicit `String(value)` call has
                    // no source expression of its own.  Give the
                    // synthetic result its checked intrinsic type here
                    // instead of asking declaration-based MIR closure to
                    // rediscover the return type of the builtin.
                    let register = self.fresh_typed(span(value), None, Ty::StringLiteral);
                    self.emit(MirInstr::Call {
                        dest: register,
                        func: FuncRef::named("String"),
                        raises: None,
                        args: vec![argument],
                        kwargs: Vec::new(),
                        arg_places: vec![None],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        receiver: None,
                        instantiated_args: Vec::new(),
                        spread: None,
                    });
                    register
                }
            };
            let joined = self.fresh(span(e), None);
            self.emit(MirInstr::BinOp {
                op: InfixOp::Add,
                dest: joined,
                a: result,
                b: piece,
                resolved: None,
            });
            result = joined;
        }
        result
    }

    pub(super) fn nested_type_apply(&mut self, e: &Expr, name: &str) -> Reg {
        let info = self
            .nested_info(e)
            .expect("guard established a checked nested declaration");
        let dest = self.load_nested_closure(name, &info, span(e));
        // The closure slot carries the declaration's generic callable
        // type; this expression carries the checker's concrete Origin
        // substitution and must win at the MIR value boundary.
        if let Some(specialized) = self.checked_ty(e) {
            self.f.reg_types.insert(dest.0, specialized);
        }
        dest
    }
}
