//! The `expr_unconverted` expression dispatcher: five families, each
//! chosen by the expression's checked op.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedExpr, CheckedOp};
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    /// Lower `node` without its adjustments, through the syntax rebuilt
    /// from it: the one crossing back into the families, which still read
    /// operands off syntax (R649 deletes it).
    pub(in crate::mir) fn expr_unconverted(&mut self, node: Node<'a>) -> Reg {
        let Some(syntax) = self.syntax(node) else {
            let dest = self.fresh(node.location(), None);
            self.emit(MirInstr::Unsupported(
                "an expression without its rebuilt syntax reached MIR".to_string(),
            ));
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return dest;
        };
        let e = syntax.expr();
        if let Some(value) = self
            .literal_or_name(e)
            .or_else(|| self.call_family(e))
            .or_else(|| self.access_family(e))
            .or_else(|| self.display_family(e))
            .or_else(|| self.operator_family(e))
        {
            return value;
        }
        self.unsupported_value(e, "an expression without its checked node reached MIR")
    }

    /// Read slot `var` at `location`; `node` is the name node read, whose
    /// type and adjustments decide a pointer read and a checked value copy.
    pub(in crate::mir) fn read_slot(
        &mut self,
        var: VarId,
        location: SourceSpan,
        node: Option<&CheckedExpr>,
    ) -> Reg {
        let d = self.fresh(location.clone(), Some(var));
        if origin_bearing_pointer(node.and_then(|node| node.ty.as_ref())) {
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
            let handle = self.fresh(location.clone(), Some(var));
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
        let Some(node) = node else {
            return d;
        };
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) && !matches!(node.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
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
                .or_else(|| node.ty.clone())
                .unwrap_or_else(Ty::error);
            let copied = self.fresh_typed(location, Some(var), ty);
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: d,
            });
            return copied;
        }
        d
    }

    pub(in crate::mir) fn transfer(&mut self, e: Node<'a>, inner: Node<'a>) -> Reg {
        if let Some(name) = self.name_spelled(inner) {
            let var = self.expression_var(&name, inner);
            // A `mut` parameter's slot holds the handle to the caller's
            // storage, so the transfer takes the referent out through it. A
            // value that owns no storage transfers as a copy and leaves the
            // caller's value in place; a captured pack owns its elements.
            if (var as usize) < self.f.n_params
                && self.runtime_aliases.contains(&var)
                && !self.pointer_valued_slot(var)
            {
                let ty = self.var_types.get(&var).cloned();
                if !ty.as_ref().is_some_and(|ty| {
                    crate::mir::calls::owns_droppable_storage(ty)
                        || matches!(ty.kind(), TyKind::VariadicPack(_))
                }) {
                    return self.expr(inner);
                }
                let d = self.fresh(e.location(), Some(var));
                let mut place = MirPlace::root(var, ty);
                place.through = Some(var);
                self.emit(MirInstr::MovePlace { dest: d, place });
                return d;
            }
            let d = self.fresh(e.location(), Some(var));
            self.emit(MirInstr::UseVar {
                dest: d,
                var,
                mode: UseMode::Move,
            });
            d
        } else if let Some(place) = self.pure_field_place(inner) {
            let d = self.fresh(e.location(), Some(place.root));
            self.emit(MirInstr::MovePlace { dest: d, place });
            d
        } else if let Some((object, _)) = inner.index_operands()
            && matches!(object.ty().as_ref().map(Ty::kind), Some(TyKind::Tuple(_)))
            && let Some(place) = self.try_place(inner)
        {
            let d = self.fresh(e.location(), Some(place.root));
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
        let (argument, arg_place) = self.lower_call_argument(self.node_of(value));
        let (recv, recv_place) = self.lower_call_receiver(self.node_of(container));
        let contains = self.fresh_typed(span(e), None, Ty::bool());
        // A generic `__contains__[T]` binds its own parameters from the
        // call's solved arguments, as a spelled method call's does.
        let param_decls = self
            .node(e)
            .and_then(Node::call_contract)
            .map(|contract| contract.param_decls)
            .unwrap_or_default();
        let instantiated_args = if param_decls.is_empty() {
            Vec::new()
        } else {
            self.node(e)
                .map(Node::instantiated_args)
                .unwrap_or_default()
        };
        self.emit_interior_invalidations(self.node_of(container), None);
        self.emit_call_invalidations(self.node_of(e), &[self.node_of(value)], &[]);
        self.emit(MirInstr::MethodCall {
            dest: contains,
            recv,
            method: "__contains__".to_string(),
            resolved: self.node_of(e).resolved_callable(),
            raises: self.node(e).and_then(Node::raises),
            reference_result: None,
            result_adapter: None,
            args: vec![argument],
            kwargs: Vec::new(),
            recv_place: recv_place.map(Box::new),
            recv_writes: self.node_of(e).receiver_writes(),
            arg_places: vec![arg_place],
            kwarg_places: Vec::new(),
            capture_accesses: self
                .node(e)
                .map(|node| self.call_capture_accesses(node))
                .unwrap_or_default(),
            param_arg_regs: Vec::new(),
            param_decls,
            instantiated_args,
            spread: None,
        });
        self.emit_nested_closure_argument_keepalives(&[self.node_of(value)], &[]);
        if matches!(op, InfixOp::NotIn) {
            let dest = self.fresh_typed(span(e), None, Ty::bool());
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
        let ra = self.lower_operator_operand(self.node_of(a));
        let rb = self.lower_operator_operand(self.node_of(b));
        let resolved = self.node_of(e).resolved_callable();
        // A reflected operator (checker-marked) runs on the RIGHT
        // operand: `1 + m` calls `m.__radd__(1)`.
        let reflected = self
            .node(e)
            .map_or(&[][..], Node::adjustments)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ReflectedOperator
                )
            });
        let (ra, rb) = if reflected { (rb, ra) } else { (ra, rb) };
        // Equatable's default `__ne__` (checker-marked): dispatch
        // `__eq__` and negate its result.
        if op == InfixOp::Ne
            && self
                .node(e)
                .map_or(&[][..], Node::adjustments)
                .iter()
                .any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::NegatedEquality
                    )
                })
        {
            let equal = self.fresh_typed(span(e), None, Ty::bool());
            self.emit(MirInstr::BinOp {
                op: InfixOp::Eq,
                dest: equal,
                a: ra,
                b: rb,
                resolved,
            });
            let dest = self.fresh_typed(span(e), None, Ty::bool());
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

    pub(super) fn list_literal(&mut self, e: &Expr, node: &CheckedExpr, elems: &[Expr]) -> Reg {
        if let Some((target, constructor)) = array_literal_plan(node) {
            // One nominal constructor call: the element registers are
            // the variadic arguments, `__list_literal__` selects the
            // literal overload, and the `length` parameter argument
            // reifies the constructed value's `value_params`.
            let regs = self.args(&self.nodes_of(elems));
            let none = self.fresh_typed(span(e), None, Ty::none());
            self.emit(MirInstr::Const {
                dest: none,
                k: Const::None,
            });
            let length = mojito_types::types::array_parts(&target)
                .map_or(elems.len() as i64, |(_, length)| length);
            let length_reg = self.fresh_typed(span(e), None, Ty::int());
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
                        expr: None,
                    },
                    MirParamArg {
                        name: None,
                        value: Some(length_reg),
                        expr: None,
                    },
                ],
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
            });
            return d;
        }
        if let Some((target, Some(insert))) = collection_plan(node) {
            let collection = self.begin_nominal_collection(e, &target);
            for element in elems {
                let value = self.expr(self.node_of(element));
                self.insert_nominal_collection(
                    self.node_of(element),
                    collection,
                    &target,
                    &insert,
                    vec![value],
                );
            }
            return self.finish_nominal_collection(e, collection, &target);
        }
        self.display_without_plan(e, "a list display")
    }

    pub(super) fn brace_literal(
        &mut self,
        e: &Expr,
        node: &CheckedExpr,
        entries: &[(Expr, Option<Expr>)],
    ) -> Reg {
        if let Some((target, Some(insert))) = collection_plan(node) {
            let collection = self.begin_nominal_collection(e, &target);
            let dictionary = dict_elements(&target).is_some();
            for (key, value) in entries {
                let key = self.expr(self.node_of(key));
                let mut arguments = vec![key];
                if dictionary {
                    arguments.push(
                        self.expr(
                            self.node_of(
                                value
                                    .as_ref()
                                    .expect("checked dictionary display has paired values"),
                            ),
                        ),
                    );
                }
                self.insert_nominal_collection(
                    self.node_of(e),
                    collection,
                    &target,
                    &insert,
                    arguments,
                );
            }
            return self.finish_nominal_collection(e, collection, &target);
        }
        self.display_without_plan(e, "a brace display")
    }

    pub(super) fn tuple_literal(&mut self, e: &Expr, node: &CheckedExpr, elems: &[Expr]) -> Reg {
        if let Some((target, None)) = collection_plan(node)
            && let TyKind::Struct(name, _) = target.kind()
        {
            let regs = self.args(&self.nodes_of(elems));
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
        self.display_without_plan(e, "a tuple display")
    }

    pub(super) fn nested_type_apply(&mut self, e: &Expr, name: &str) -> Reg {
        let info = self
            .node(e)
            .and_then(|node| self.nested_info(node))
            .expect("guard established a checked nested declaration");
        let dest = self.load_nested_closure(name, &info, span(e));
        // The closure slot carries the declaration's generic callable
        // type; this expression carries the checker's concrete Origin
        // substitution and must win at the MIR value boundary.
        if let Some(specialized) = self.node(e).and_then(Node::ty) {
            self.f.reg_types.insert(dest.0, specialized);
        }
        dest
    }

    /// The explicit boundary for `e`: `Unsupported(message)`, then `None`
    /// into a fresh register.
    fn unsupported_value(&mut self, e: &Expr, message: &str) -> Reg {
        let dest = self.fresh(span(e), None);
        self.emit(MirInstr::Unsupported(message.to_string()));
        self.emit(MirInstr::Const {
            dest,
            k: Const::None,
        });
        dest
    }

    /// Lower `e` by its node's op when that op is a literal or a name:
    /// `None` for any other op, which another family lowers.
    fn literal_or_name(&mut self, e: &Expr) -> Option<Reg> {
        let node = self.node(e)?.checked;
        Some(match &node.op {
            CheckedOp::Int(n) => self.literal(node, Const::IntLiteral(n.clone())),
            CheckedOp::Float(x) => self.literal(node, Const::FloatLiteral(x.clone())),
            CheckedOp::Bool(b) => self.literal(node, Const::Bool(*b)),
            CheckedOp::Str(s) => self.literal(node, Const::Str(s.clone())),
            CheckedOp::None | CheckedOp::Uninitialized => self.literal(node, Const::None),
            // The `p[]` dereference marker: the checker validated the pointer
            // receiver, and offset 0 is the whole lowering.
            CheckedOp::EmptySubscript => self.literal(node, Const::Int(0)),
            // A bare read defaults to `Copy` — the lifecycle copy for owned
            // storage. Consuming conventions keep it; a read-convention call
            // argument is instead bound as a shallow place read where the
            // checker marked `BorrowReadArgument` (`lower_call_argument`), and
            // `x^` lowers as `Move`.
            CheckedOp::Identifier(name) => self.identifier_read(e, node, name),
            CheckedOp::Named { name, .. } => match expression_children(e).as_slice() {
                [value] => self.walrus(value, node, name),
                _ => return None,
            },
            _ => return None,
        })
    }

    /// Lower `e` by its node's op when that op is a call, an invocation, a
    /// method call, or a callable type application: `None` for any other
    /// op, which another family lowers. The op decides; the syntax only
    /// binds the operands still lowered from it.
    fn call_family(&mut self, e: &Expr) -> Option<Reg> {
        let node = self.node(e)?.checked;
        // A pack or reflection query read as a runtime value is a
        // parameter constant; its operand is a type, never lowered.
        if matches!(
            node.op,
            CheckedOp::Call { .. }
                | CheckedOp::Invoke { .. }
                | CheckedOp::Member { .. }
                | CheckedOp::MethodCall { .. }
                | CheckedOp::Index { .. }
        ) && let Some(value) = recorded_param_value(&node.adjustments)
        {
            return Some(self.param_value_register(self.node_of(e), value));
        }
        Some(match (&node.op, &e.kind) {
            (
                CheckedOp::Call { name, .. },
                ExprKind::Call {
                    param_args,
                    args,
                    kwargs,
                    ..
                },
            ) => self.call_op(e, node, name, param_args, args, kwargs),
            (
                CheckedOp::Invoke { .. },
                ExprKind::Invoke {
                    callee,
                    param_args,
                    args,
                    kwargs,
                },
            ) => self.invoke_expr(e, node, callee, param_args, args, kwargs),
            (
                CheckedOp::MethodCall { method, .. },
                ExprKind::MethodCall {
                    object,
                    args,
                    kwargs,
                    ..
                },
            ) => self.method_call_expr(e, node, object, method, args, kwargs),
            (CheckedOp::TypeApply { name, .. }, _) if !variant_projection(node) => {
                if node
                    .binding
                    .is_some_and(|binding| self.nested.contains_key(&binding))
                {
                    self.nested_type_apply(e, name)
                } else {
                    self.literal(node, Const::Function(callable_target(&node.adjustments)?))
                }
            }
            _ => return None,
        })
    }

    /// Lower `e` by its node's op when that op is a member read, a
    /// subscript, a slice, a multi-subscript, or a `Variant` projection:
    /// `None` for any other op, which another family lowers. The op
    /// decides; the syntax only binds the operands still lowered from it.
    fn access_family(&mut self, e: &Expr) -> Option<Reg> {
        let node = self.node(e)?.checked;
        Some(match (&node.op, &e.kind) {
            (CheckedOp::Member { field, .. }, ExprKind::Member { object, .. }) => {
                self.member_expr(e, node, object, field)
            }
            // A variant projection spelled with a struct-name index
            // (`v[String]`) carries the same checked adjustment as the
            // type-token spelling below and lowers identically.
            (CheckedOp::Index { .. }, ExprKind::Index { object, .. })
                if variant_projection(node) =>
            {
                self.variant_project_index(e, node, object)
            }
            (CheckedOp::Index { .. }, ExprKind::Index { object, index }) => {
                self.index_expr(e, node, object, index)
            }
            // Slice `object[lower:upper:step]` → a new List/String.
            (
                CheckedOp::Slice { .. },
                ExprKind::Slice {
                    object,
                    lower,
                    upper,
                    step,
                    ..
                },
            ) => self.slice_expr(
                e,
                node,
                object,
                lower.as_deref(),
                upper.as_deref(),
                step.as_deref(),
            ),
            (
                CheckedOp::MultiIndex {
                    args: checked_args, ..
                },
                ExprKind::MultiIndex { object, args },
            ) => self.multi_index_expr(e, node, object, checked_args, args),
            (CheckedOp::TypeApply { name, .. }, _) if variant_projection(node) => {
                self.variant_project_type_apply(e, node, name)
            }
            _ => return None,
        })
    }

    /// Lower `e` by its node's op when that op is a list, brace, or tuple
    /// display or a comprehension: `None` for any other op, which another
    /// family lowers. The op decides; the syntax only binds the operands
    /// still lowered from it.
    fn display_family(&mut self, e: &Expr) -> Option<Reg> {
        let node = self.node(e)?.checked;
        Some(match (&node.op, &e.kind) {
            (CheckedOp::ListLit(_), ExprKind::ListLit(elems)) => self.list_literal(e, node, elems),
            (CheckedOp::BraceLit(_), ExprKind::BraceLit(entries)) => {
                self.brace_literal(e, node, entries)
            }
            (
                CheckedOp::Comprehension { clauses, .. },
                ExprKind::Comprehension {
                    key,
                    value,
                    clauses: clause_syntax,
                    ..
                },
            ) => self.comprehension(e, node, clauses, key.as_deref(), value, clause_syntax),
            (CheckedOp::TupleLit(_), ExprKind::TupleLit(elems)) => {
                self.tuple_literal(e, node, elems)
            }
            _ => return None,
        })
    }

    /// The boundary for display `e` (described as `what`) whose node carries
    /// no construction the checker selected: lowering never guesses the
    /// collection from its delimiters.
    fn display_without_plan(&mut self, e: &Expr, what: &str) -> Reg {
        self.unsupported_value(
            e,
            &format!("{what} without its checked construction reached MIR"),
        )
    }

    /// Lower `e` by its node's op when that op is an operator, a comparison
    /// chain, a ternary, a transfer, a spread, a lambda, a type value, or a
    /// t-string: `None` for any other op. The op decides and supplies the
    /// operator; each operand is the rebuilt child at its position among
    /// the op's operands.
    fn operator_family(&mut self, e: &Expr) -> Option<Reg> {
        let node = self.node(e)?.checked;
        Some(match (&node.op, expression_children(e).as_slice()) {
            (CheckedOp::Prefix(op, _), [value]) => self.prefix_op(e, *op, value),
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            (CheckedOp::Infix(op @ (InfixOp::And | InfixOp::Or), ..), [a, b]) => {
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
            (CheckedOp::Infix(op @ (InfixOp::In | InfixOp::NotIn), ..), [value, container])
                if matches!(
                    self.node(container)
                        .and_then(Node::ty)
                        .as_ref()
                        .map(Ty::kind),
                    Some(TyKind::Struct(..))
                ) =>
            {
                self.contains_call(e, *op, value, container)
            }
            (CheckedOp::Infix(op, ..), [a, b]) => self.infix_op(e, *op, a, b),
            // Ternary `a if cond else b` — a value-producing branch (like the
            // short-circuit lowering, but both arms assign the result).
            (CheckedOp::IfExpr { .. }, [cond, then_branch, else_branch]) => {
                self.ternary(cond, then_branch, else_branch, span(e))
            }
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            (CheckedOp::Compare { rest, .. }, [first, others @ ..])
                if rest.len() == others.len() =>
            {
                let links: Vec<(InfixOp, &Expr)> = rest
                    .iter()
                    .zip(others)
                    .map(|((op, _), value)| (*op, *value))
                    .collect();
                self.compare_chain(first, &links, span(e))
            }
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            (CheckedOp::Transfer(_), [inner]) => {
                self.transfer(self.node_of(e), self.node_of(inner))
            }
            // A lambda expression materializes its hidden definition's
            // closure at the expression's evaluation point — copy/move
            // captures snapshot here, once per evaluation.
            (CheckedOp::Lambda { .. }, []) => match self
                .node(e)
                .and_then(|node| self.nested_info(node))
            {
                Some(info) => self.emit_nested_closure(&info, span(e), false),
                None => self
                    .unsupported_value(e, "lambda expression lost its checked nested declaration"),
            },
            // A call argument's spread is expanded by the call that takes it.
            (CheckedOp::Spread(_), _) => {
                self.unsupported_value(e, "unexpanded call spread reached MIR lowering")
            }
            // HIR lowers the construction the checker spelled for a
            // t-string in its place, so a checked program never reaches one.
            (CheckedOp::TString { .. }, _) => self.unsupported_value(
                e,
                "a t-string reached MIR without the construction the checker built for it",
            ),
            (CheckedOp::TypeValue { .. }, _) => {
                self.unsupported_value(e, "a type value reached MIR lowering")
            }
            _ => return None,
        })
    }

    /// Prefix operator `op` applied to `value`, the operand of node `e`.
    fn prefix_op(&mut self, e: &Expr, op: PrefixOp, value: &Expr) -> Reg {
        let ra = self.expr(self.node_of(value));
        // `not x` on a struct or a numeric scalar is
        // `not x.__bool__()`: convert through the explicit `Bool(x)`
        // call so the negation is scalar on both backends.
        let ra = if op == PrefixOp::Not
            && self.node(value).and_then(Node::ty).is_some_and(|ty| {
                matches!(ty.kind(), TyKind::Struct(..))
                    || mojito_types::conformance::is_numeric(&ty)
                    || mojito_types::types::is_scalar_simd(&ty)
            }) {
            self.bool_conversion(value.source_span(), ra)
        } else {
            ra
        };
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::UnOp { op, dest: d, a: ra });
        d
    }

    /// Call node `node`, written `name`: a crossing the elaborator left
    /// (`materialize[X]()`, `comptime(e)`) lowers its operand, any other
    /// call is an ordinary call.
    fn call_op(
        &mut self,
        e: &Expr,
        node: &CheckedExpr,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Reg {
        let CheckedOp::Call {
            param_args: params,
            args: arguments,
            kwargs: keywords,
            ..
        } = &node.op
        else {
            unreachable!("call_op lowers a call node")
        };
        // `materialize[X]()` the elaborator above MIR left, over a
        // binder: the runtime value of its compile-time operand.
        if name == "materialize"
            && arguments.is_empty()
            && keywords.is_empty()
            && let [mojito_checked::checked::CheckedParamArg::Value(_)] = params.as_slice()
            && let [ParamArg::Value(operand)] = param_args
        {
            return self.crossing_operand(self.node_of(operand));
        }
        // `comptime(e)` left the same way.
        if name == "comptime"
            && params.is_empty()
            && keywords.is_empty()
            && let [operand] = arguments.as_slice()
            && self
                .checked
                .expression(*operand)
                .is_some_and(|operand| operand.ty.is_some())
            && let [operand] = args
        {
            return self.crossing_operand(self.node_of(operand));
        }
        self.call_expr(e, node, name, param_args, args, kwargs)
    }

    /// Walrus node `node`, `name := value`: bind the value's slot and yield
    /// the value.
    fn walrus(&mut self, value: &Expr, node: &CheckedExpr, name: &str) -> Reg {
        let value = self.expr(self.node_of(value));
        let var = self.var(&self.spelling(node, name));
        self.emit(MirInstr::DefVar {
            var,
            src: value,
            binding_ty: None,
        });
        value
    }

    /// Read name node `node`, written `name`, which `e` was rebuilt from.
    fn identifier_read(&mut self, e: &Expr, node: &CheckedExpr, name: &str) -> Reg {
        let name = self.spelling(node, name);
        if let Some(target) = callable_target(&node.adjustments) {
            return self.literal(node, Const::Function(target));
        }
        if let Some(info) = node
            .binding
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
        {
            return self.load_nested_closure(&name, &info, node.location());
        }
        let local = node
            .binding
            .and_then(|owner| self.local_binding_var(owner, &name));
        if local.is_none() && self.overloads.is_function(&name) {
            return self.literal(node, Const::Function(name));
        }
        // A module constant is the parameter value the check recorded.
        if let Some(value) = recorded_param_value(&node.adjustments) {
            return self.param_value_register(self.node_of(e), value);
        }
        // In a lifted thunk, an enclosing `comptime for` index or local
        // `comptime` binding is the parameter expression it denotes.
        let denoted = node.binding.and_then(|owner| {
            self.parameter_reads
                .get(&owner)
                .and_then(|binder| self.enclosing_binders.value_of(binder))
                .or_else(|| self.enclosing_binders.comptime_value(owner))
                .or_else(|| self.comptime_thunks.bound_value(owner))
        });
        if let Some(value) = denoted {
            return self.param_value_register(self.node_of(e), value);
        }
        let var = match (local, node.binding) {
            (Some(var), _) => var,
            (None, Some(owner)) => self.binding_var(owner, &name),
            (None, None) => self.var(&name),
        };
        self.read_slot(var, node.location(), Some(node))
    }
}

/// Whether type application node `node` projects a `Variant` alternative
/// (`v[T]`), a place rather than a callable value.
fn variant_projection(node: &CheckedExpr) -> bool {
    node.adjustments.iter().any(|adjustment| {
        matches!(
            adjustment,
            mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
        )
    })
}
