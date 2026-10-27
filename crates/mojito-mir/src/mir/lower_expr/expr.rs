//! The `expr_unconverted` expression dispatcher: five families, each
//! chosen by the expression's checked op.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CheckedExpr, CheckedNodeId, CheckedOp, CheckedParamArg, DictLiteral,
};
use mojito_types::types::TyKind;

/// What the checker dispatched one operator to: the selected dunder's
/// lowered symbol (`None` for a built-in operator), and the error type its
/// call raises.
pub(in crate::mir) struct OperatorDispatch {
    pub(in crate::mir) resolved: Option<String>,
    pub(in crate::mir) raises: Option<Ty>,
    /// The method a struct receiver answers the operator with, when the
    /// operator is that method's call ([`struct_operator_method`]).
    pub(in crate::mir) method: Option<&'static str>,
    /// The selected dunder's method-call contract, when the check recorded
    /// one: its receiver convention, its compile-time parameter
    /// declarations, and the arguments the call solved for them.
    pub(in crate::mir) contract: Option<OperatorContract>,
}

/// What a chained comparison's link records of its dunder's method call.
pub(in crate::mir) struct OperatorContract {
    pub(in crate::mir) receiver_convention: Option<mojito_ast::ast::ArgConvention>,
    pub(in crate::mir) param_decls: Vec<ParamDecl>,
    pub(in crate::mir) instantiated_args: Vec<TyArg>,
    /// See [`mojito_checked::checked::CheckedCallContract::refined_bound`].
    pub(in crate::mir) refined_bound: Option<Vec<String>>,
}

/// The method struct value `receiver` answers a chained comparison's link
/// `op` with, as upstream's parser spells the link: `a < b` is
/// `a.__lt__(b)`. `None` for any other receiver, whose link stays a
/// `binary`.
pub(in crate::mir) fn struct_operator_method(
    op: InfixOp,
    receiver: Node<'_>,
) -> Option<&'static str> {
    crate::mir::operator_method(op).filter(|_| {
        receiver
            .ty()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Struct(..)))
    })
}

impl<'a> Flatten<'a> {
    /// Lower `node` without its adjustments, by the family its op names:
    /// each reads its operands off the handle.
    pub(in crate::mir) fn expr_unconverted(&mut self, node: Node<'a>) -> Reg {
        if let Some(value) = self
            .literal_or_name(node)
            .or_else(|| self.call_family(node))
            .or_else(|| self.access_family(node))
            .or_else(|| self.display_family(node))
            .or_else(|| self.operator_family(node))
        {
            return value;
        }
        self.unsupported_value(
            node.location(),
            "an expression without its checked node reached MIR",
        )
    }

    /// The explicit boundary at `location`: `Unsupported(message)`, then
    /// `None` into a fresh register.
    pub(in crate::mir) fn unsupported_value(&mut self, location: SourceSpan, message: &str) -> Reg {
        let dest = self.fresh(location, None);
        self.emit(MirInstr::Unsupported(message.to_string()));
        self.emit(MirInstr::Const {
            dest,
            k: Const::None,
        });
        dest
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
                constructor: None,
            });
            return d;
        }
        if let Some(loan) = self.aliases.get(&var).cloned() {
            let mut place = loan.place;
            place.through = Some(var);
            self.emit(MirInstr::LoadPlace { dest: d, place });
        } else if self.runtime_aliases.contains(&var) {
            // A read borrows the referent immutably, whatever the handle
            // the slot holds lets a write do.
            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
            place.through = Some(var);
            let handle = match mir_place_handle_ty(
                &place,
                Some(mojito_types::origin::Mutability::Immutable),
            ) {
                Some(ty) => self.fresh_typed(location.clone(), Some(var), ty),
                None => self.fresh(location.clone(), Some(var)),
            };
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place,
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
                constructor: None,
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
                constructor: None,
            });
            return copied;
        }
        d
    }

    pub(in crate::mir) fn transfer(&mut self, e: Node<'a>, inner: Node<'a>) -> Reg {
        // A trivial register value is copied, and its source stays
        // initialized, as upstream's `emitTransfer` decides it.
        if e.transfer_verdict() == Some(mojito_checked::checked::TransferVerdict::TrivialCopy) {
            return self.expr(inner);
        }
        if let Some(name) = self.name_spelled(inner) {
            let var = self.expression_var(&name, inner);
            // A `mut` parameter's slot, or a capture's reference slot,
            // holds the handle to the caller's storage, so the transfer
            // takes the referent out through it. A value that owns no
            // storage, whose triviality the check's predicate does not
            // model (a pointer, a literal), transfers as a copy and leaves
            // the caller's value in place; a captured pack owns its
            // elements.
            let slot_ty = self.var_types.get(&var).cloned();
            let reference_slot = matches!(slot_ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)));
            if ((var as usize) < self.f.n_params || reference_slot)
                && self.runtime_aliases.contains(&var)
                && !self.pointer_valued_slot(var)
            {
                let ty = match slot_ty.as_ref().map(Ty::kind) {
                    Some(TyKind::Ref(reference)) => Some(reference.referent.clone()),
                    _ => slot_ty.clone(),
                };
                if !ty.as_ref().is_some_and(|ty| {
                    crate::mir::calls::owns_droppable_storage(ty)
                        || matches!(ty.kind(), TyKind::VariadicPack(_))
                }) {
                    return self.expr(inner);
                }
                let d = self.fresh(e.location(), Some(var));
                let mut place = MirPlace::root(var, slot_ty);
                place.ty = ty;
                place.through = Some(var);
                self.emit(MirInstr::MovePlace { dest: d, place });
                return d;
            }
            // An owned parameter takes the slot's storage itself.
            let forwarded = e.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ForwardedArgument
                )
            });
            let d = self.fresh(e.location(), Some(var));
            self.emit(MirInstr::UseVar {
                dest: d,
                var,
                mode: if forwarded {
                    UseMode::Forward
                } else {
                    UseMode::Move
                },
                constructor: None,
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
        node: Node<'a>,
        op: InfixOp,
        value: Node<'a>,
        container: Node<'a>,
    ) -> Reg {
        let (argument, arg_place) = self.lower_call_argument(value);
        let (recv, recv_place) = self.lower_method_receiver(node, container);
        let contains = self.fresh_typed(node.location(), None, Ty::bool());
        // A generic `__contains__[T]` binds its own parameters from the
        // call's solved arguments, as a spelled method call's does.
        let param_decls = node
            .call_contract()
            .map(|contract| contract.param_decls)
            .unwrap_or_default();
        let instantiated_args = if param_decls.is_empty() {
            Vec::new()
        } else {
            node.instantiated_args()
        };
        self.emit_interior_invalidations(container, None);
        self.emit_call_invalidations(node, &[value], &[]);
        let raises = self.raises(node.raises(), node.location());
        self.emit_call(MirInstr::MethodCall {
            dest: contains,
            recv,
            method: "__contains__".to_string(),
            resolved: node.resolved_callable(),
            raises: raises.clone(),
            reference_result: None,
            result_adapter: None,
            args: vec![argument],
            kwargs: Vec::new(),
            recv_place: recv_place.map(Box::new),
            recv_writes: node.receiver_writes(),
            receiver_convention: node.receiver_convention(container),
            arg_places: vec![arg_place],
            kwarg_places: Vec::new(),
            capture_accesses: self.call_capture_accesses(node),
            param_arg_regs: Vec::new(),
            param_decls,
            instantiated_args,
            spread: None,
            refined_bound: None,
        });
        self.emit_nested_closure_argument_keepalives(&[value], &[]);
        self.error_region(raises);
        if matches!(op, InfixOp::NotIn) {
            let dest = self.fresh_typed(node.location(), None, Ty::bool());
            self.emit(MirInstr::UnOp {
                op: PrefixOp::Not,
                dest,
                a: contains,
                raises: None,
            });
            dest
        } else {
            contains
        }
    }

    pub(super) fn infix_op(
        &mut self,
        node: Node<'a>,
        op: InfixOp,
        a: Node<'a>,
        b: Node<'a>,
    ) -> Reg {
        // A reflected operator (checker-marked) runs on the RIGHT
        // operand: `1 + m` calls `m.__radd__(1)`.
        let reflected = node.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ReflectedOperator
            )
        });
        // A struct operator is its dunder's method call, as upstream's
        // `emitBinOpCall` makes `a + b` the call `a.__add__(b)`.
        if node.call_contract().is_some()
            && let Some(method) = crate::mir::operator_method(op)
        {
            return match op.reflected_dunder().filter(|_| reflected) {
                Some(method) => self.reflected_operator_call(node, method, a, b),
                None => self.ordinary_method_call(node, a, method, &[b], &[]),
            };
        }
        // Operands left-to-right (evaluation order is explicit), each place
        // operand read at the operator call's setup.
        self.open_call_setup();
        let ra = self.lower_operator_operand(node, a);
        let rb = self.lower_operator_operand(node, b);
        self.close_call_setup(None);
        let dispatch = OperatorDispatch {
            resolved: node.resolved_callable(),
            raises: node.raises(),
            method: None,
            contract: None,
        };
        self.emit_comparison(node.location(), op, ra, rb, dispatch)
    }

    /// Emit one dispatched operator over `a` and `b`: the dunder the
    /// checker selected for a struct receiver is called as a method, as
    /// upstream's parser spells `a + b` as `a.__add__(b)`; any other
    /// operator is a `binary`, whose `resolved` names a bound's
    /// requirement the elaborator answers per instance.
    pub(in crate::mir) fn emit_comparison(
        &mut self,
        location: SourceSpan,
        op: InfixOp,
        a: Reg,
        b: Reg,
        dispatch: OperatorDispatch,
    ) -> Reg {
        let OperatorDispatch {
            resolved,
            raises,
            method,
            contract,
        } = dispatch;
        let raises = self.raises(raises, location.clone());
        let d = self.fresh(location, None);
        let (contract, refined_bound) = contract.map_or((None, None), |mut contract| {
            let refined_bound = contract.refined_bound.take().map(MirRefinedBound::boxed);
            (Some(contract), refined_bound)
        });
        self.emit(match (resolved, method) {
            (Some(resolved), Some(method)) => {
                let mut call =
                    crate::mir::dunder_method_call(d, a, method, Some(resolved), raises, vec![b]);
                if let Some(contract) = contract
                    && let MirInstr::MethodCall {
                        receiver_convention,
                        param_decls,
                        instantiated_args,
                        ..
                    } = &mut call
                {
                    *receiver_convention = contract.receiver_convention;
                    *param_decls = contract.param_decls;
                    *instantiated_args = contract.instantiated_args;
                }
                call
            }
            (resolved, _) => MirInstr::BinOp {
                op,
                dest: d,
                a,
                b,
                resolved,
                raises,
                refined_bound,
            },
        });
        d
    }

    /// One call of constructor `func` to a fresh register typed `ty`,
    /// instantiated at `ty`'s arguments, binding `args` positionally and
    /// then `kwargs`.
    pub(super) fn constructor_call(
        &mut self,
        node: Node<'a>,
        func: &str,
        ty: Ty,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
    ) -> Reg {
        let TyKind::Struct(_, instantiated_args) = ty.kind() else {
            unreachable!("a constructor builds a nominal value")
        };
        let instantiated_args = instantiated_args.clone();
        let dest = self.fresh_typed(node.location(), None, ty);
        self.record_call(dest, Some(func.to_string()), 0);
        self.emit(MirInstr::Call {
            dest,
            func: FuncRef::named(func),
            raises: None,
            arg_places: vec![None; args.len()],
            args,
            kwarg_places: vec![None; kwargs.len()],
            kwargs,
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args,
            spread: None,
        });
        dest
    }

    /// List display `node`, whose operands are its elements in order.
    pub(super) fn list_literal(&mut self, node: Node<'a>) -> Reg {
        let elements: Vec<Node<'a>> = node.operands().collect();
        if let Some((target, constructor)) = array_literal_plan(node.checked) {
            // One nominal constructor call: the element registers are
            // the variadic arguments, `__list_literal__` selects the
            // literal overload, and the `length` parameter argument
            // reifies the constructed value's `value_params`.
            let regs = self.display_elements(node, &elements);
            let none = self.fresh_typed(node.location(), None, Ty::none());
            self.emit(MirInstr::Const {
                dest: none,
                k: Const::None,
            });
            let length = mojito_types::types::array_parts(&target)
                .map_or(elements.len() as i64, |(_, length)| length);
            let length_reg = self.fresh_typed(node.location(), None, Ty::int());
            self.emit(MirInstr::Const {
                dest: length_reg,
                k: Const::Int(length),
            });
            let d = self.fresh_typed(node.location(), None, target);
            self.record_call(d, Some(constructor.clone()), 0);
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
        if let Some(CollectionPlan {
            target,
            constructor: Some(constructor),
            marker,
            insert: None,
        }) = collection_plan(node.checked)
        {
            let values = self.display_elements(node, &elements);
            return self.display_construction(node, target.clone(), constructor, marker, values);
        }
        self.display_without_plan(node, "a list display")
    }

    /// Brace display `node`, whose entries are `entries`.
    pub(super) fn brace_literal(
        &mut self,
        node: Node<'a>,
        entries: &[(CheckedNodeId, Option<CheckedNodeId>)],
    ) -> Option<Reg> {
        let Some(plan) = collection_plan(node.checked) else {
            return Some(self.display_without_plan(node, "a brace display"));
        };
        // A dictionary display evaluates every key before any value, as its
        // `keys` and `values` constructor arguments are.
        let mut operands = entries
            .iter()
            .map(|(key, _)| node.operand_of(*key))
            .collect::<Option<Vec<_>>>()?;
        if dict_elements(plan.target).is_some() {
            operands.extend(entries.iter().map(|(_, value)| {
                value
                    .and_then(|value| node.operand_of(value))
                    .expect("checked dictionary display has paired values")
            }));
        }
        let regs = self.display_elements(node, &operands);
        let (keys, values) = regs.split_at(entries.len());
        if let Some(literal) = dict_literal_plan(node.checked) {
            return Some(self.dict_literal(node, plan.target, literal, keys, values));
        }
        // A set display, or an empty `{}` built by the empty constructor.
        Some(match plan {
            CollectionPlan {
                target,
                constructor: Some(constructor),
                marker,
                insert: None,
            } if values.is_empty() => {
                self.display_construction(node, target.clone(), constructor, marker, keys.to_vec())
            }
            _ => self.display_without_plan(node, "a brace display"),
        })
    }

    /// Tuple display `node`, whose operands are its elements in order.
    pub(super) fn tuple_literal(&mut self, node: Node<'a>) -> Reg {
        if let Some(CollectionPlan {
            target,
            constructor: Some(constructor),
            insert: None,
            ..
        }) = collection_plan(node.checked)
        {
            let elements: Vec<Node<'a>> = node.operands().collect();
            let regs = self.display_elements(node, &elements);
            let dest = self.fresh_typed(node.location(), None, target.clone());
            self.record_call(dest, Some(constructor.to_string()), 0);
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(constructor),
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
        self.display_without_plan(node, "a tuple display")
    }

    pub(super) fn nested_type_apply(&mut self, node: Node<'a>, name: &str) -> Reg {
        let info = self
            .nested_info(node)
            .expect("guard established a checked nested declaration");
        let dest = self.load_nested_closure(name, &info, node.location());
        // The closure slot carries the declaration's generic callable
        // type; this expression carries the checker's concrete Origin
        // substitution and must win at the MIR value boundary.
        if let Some(specialized) = node.ty() {
            self.f.reg_types.insert(dest.0, specialized);
        }
        dest
    }

    /// Lower `node` by its op when that op is a literal or a name: `None`
    /// for any other op, which another family lowers.
    fn literal_or_name(&mut self, node: Node<'a>) -> Option<Reg> {
        let checked = node.checked;
        Some(match node.op() {
            CheckedOp::Int(n) => self.literal(checked, Const::IntLiteral(n.clone())),
            CheckedOp::Float(x) => self.literal(checked, Const::FloatLiteral(x.clone())),
            CheckedOp::Bool(b) => self.literal(checked, Const::Bool(*b)),
            CheckedOp::Str(s) => self.literal(checked, Const::Str(s.clone())),
            CheckedOp::None => self.literal(checked, Const::None),
            CheckedOp::Uninitialized => self.unsupported_value(
                node.location(),
                "an uninitialized declaration's marker has no value",
            ),
            // The `p[]` dereference marker: the checker validated the pointer
            // receiver, and offset 0 is the whole lowering.
            CheckedOp::EmptySubscript => self.literal(checked, Const::Int(0)),
            // A bare read defaults to `Copy` — the lifecycle copy for owned
            // storage. Consuming conventions keep it; a read-convention call
            // argument is instead bound as a shallow place read where the
            // checker marked `BorrowReadArgument` (`lower_call_argument`), and
            // `x^` lowers as `Move`.
            CheckedOp::Identifier(name) => self.identifier_read(node, name),
            CheckedOp::Named { target, value } => {
                let (target, value) = (node.operand_of(*target)?, node.operand_of(*value)?);
                match target.op() {
                    CheckedOp::Identifier(name) => self.walrus(value, target.checked, name),
                    _ => self.lower_walrus_store(node, target, value),
                }
            }
            _ => return None,
        })
    }

    /// Lower `node` by its op when that op is a call, an invocation, a
    /// method call, or a callable type application: `None` for any other
    /// op, which another family lowers.
    fn call_family(&mut self, node: Node<'a>) -> Option<Reg> {
        let checked = node.checked;
        // A pack or reflection query, a parametric value's application, or
        // a type comparison, read as a runtime value is a parameter
        // constant; its operands are compile-time, never lowered.
        if matches!(
            checked.op,
            CheckedOp::Call { .. }
                | CheckedOp::Infix(..)
                | CheckedOp::Invoke { .. }
                | CheckedOp::Member { .. }
                | CheckedOp::MethodCall { .. }
                | CheckedOp::Index { .. }
                | CheckedOp::MultiIndex { .. }
                | CheckedOp::TypeApply { .. }
        ) && let Some(value) = recorded_param_value(&checked.adjustments)
        {
            return Some(self.param_value_register(node, value));
        }
        Some(match node.op() {
            CheckedOp::Call { name, .. } => self.call_op(node, name),
            CheckedOp::Invoke { .. } => self.invoke_expr(node),
            CheckedOp::MethodCall { method, .. } => self.method_call_expr(node, method),
            CheckedOp::TypeApply { name, .. } if !variant_projection(checked) => {
                if checked
                    .binding
                    .is_some_and(|binding| self.nested.contains_key(&binding))
                {
                    self.nested_type_apply(node, name)
                } else {
                    let function = callable_target(&checked.adjustments)?;
                    let arguments = node.instantiated_args();
                    self.literal(
                        checked,
                        if arguments.is_empty() {
                            Const::Function(function)
                        } else {
                            Const::FunctionInstance {
                                function,
                                arguments,
                            }
                        },
                    )
                }
            }
            _ => return None,
        })
    }

    /// Lower `node` by its op when that op is a member read, a subscript, a
    /// slice, a multi-subscript, or a `Variant` projection: `None` for any
    /// other op, which another family lowers.
    fn access_family(&mut self, node: Node<'a>) -> Option<Reg> {
        let bound = |id: &Option<CheckedNodeId>| id.and_then(|id| node.operand_of(id));
        Some(match node.op() {
            CheckedOp::Member { field, .. } => self.member_expr(node, node.object()?, field),
            // A variant projection spelled with a struct-name index
            // (`v[String]`) carries the same checked adjustment as the
            // type-token spelling below and lowers identically.
            CheckedOp::Index { .. } if variant_projection(node.checked) => {
                self.variant_project_index(node, node.object()?)
            }
            CheckedOp::Index { .. } => self.index_expr(node, node.object()?, node.index()?),
            // Slice `object[lower:upper:step]` → the selected `__getitem__`'s
            // result.
            CheckedOp::Slice {
                lower, upper, step, ..
            } => self.slice_expr(
                node,
                node.object()?,
                bound(lower),
                bound(upper),
                bound(step),
            ),
            CheckedOp::MultiIndex { args, .. } => self.multi_index_expr(node, node.object()?, args),
            CheckedOp::TypeApply { name, .. } if variant_projection(node.checked) => {
                self.variant_project_type_apply(node, name)
            }
            _ => return None,
        })
    }

    /// Lower `node` by its op when that op is a list, brace, or tuple
    /// display or a comprehension: `None` for any other op, which another
    /// family lowers.
    fn display_family(&mut self, node: Node<'a>) -> Option<Reg> {
        Some(match node.op() {
            CheckedOp::ListLit(_) => self.list_literal(node),
            CheckedOp::BraceLit(entries) => self.brace_literal(node, entries)?,
            CheckedOp::Comprehension {
                clauses,
                key,
                value,
                ..
            } => {
                let key = match key {
                    Some(key) => Some(node.operand_of(*key)?),
                    None => None,
                };
                self.comprehension(node, clauses, key, node.operand_of(*value)?)
            }
            CheckedOp::TupleLit(_) => self.tuple_literal(node),
            _ => return None,
        })
    }

    /// The boundary for display `node` (described as `what`) that carries
    /// no construction the checker selected: lowering never guesses the
    /// collection from its delimiters.
    fn display_without_plan(&mut self, node: Node<'a>, what: &str) -> Reg {
        self.unsupported_value(
            node.location(),
            &format!("{what} without its checked construction reached MIR"),
        )
    }

    /// Lower `node` by its op when that op is an operator, a comparison
    /// chain, a ternary, a transfer, a spread, a lambda, a type value, or a
    /// t-string: `None` for any other op. The op decides and supplies the
    /// operator, and each operand is the handle of the id the op names.
    fn operator_family(&mut self, node: Node<'a>) -> Option<Reg> {
        let operand = |id: &CheckedNodeId| node.operand_of(*id);
        Some(match node.op() {
            CheckedOp::Prefix(op, value) => self.prefix_op(node, *op, operand(value)?),
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            CheckedOp::Infix(op @ (InfixOp::And | InfixOp::Or), a, b) => {
                self.short_circuit(*op, operand(a)?, operand(b)?, node.location())
            }
            // A checked nominal membership operation is an ordinary borrowed
            // `container.__contains__(value)` call.  Keeping it as a value-only
            // `BinOp` loses the receiver place; the VM would then install a
            // shallow struct value in the callee's `self` slot and destroy its
            // owned fields on return.  For pointer-backed collections that can
            // free the caller's storage.  Preserve source evaluation order
            // (value before container), the selected overload, and the normal
            // method-call place/capture contract.
            CheckedOp::Infix(op @ (InfixOp::In | InfixOp::NotIn), value, container)
                if matches!(
                    operand(container).and_then(Node::ty).as_ref().map(Ty::kind),
                    Some(TyKind::Struct(..))
                ) =>
            {
                self.contains_call(node, *op, operand(value)?, operand(container)?)
            }
            CheckedOp::Infix(op, a, b) => self.infix_op(node, *op, operand(a)?, operand(b)?),
            // Ternary `a if cond else b`: an `if` whose arms produce the result.
            CheckedOp::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => self.ternary(
                node,
                operand(cond)?,
                operand(then_branch)?,
                operand(else_branch)?,
            ),
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            CheckedOp::Compare { first, rest } => {
                let links = rest
                    .iter()
                    .map(|(op, value)| Some((*op, operand(value)?)))
                    .collect::<Option<Vec<_>>>()?;
                self.compare_chain(node, operand(first)?, &links)
            }
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            CheckedOp::Transfer(inner) => self.transfer(node, operand(inner)?),
            // A lambda expression builds its closure's storage at the
            // expression's evaluation point, once per evaluation, and moves
            // it into the closure.
            CheckedOp::Lambda { .. } => match self.nested_info(node) {
                Some(info) if info.storage.is_some() && !node.closure_callable() => {
                    self.lambda_storage_value(&info, node.location())
                }
                Some(info) => self.lambda_closure(&info, node.location()),
                None => self.unsupported_value(
                    node.location(),
                    "lambda expression lost its checked nested declaration",
                ),
            },
            // A call argument's spread is expanded by the call that takes it.
            CheckedOp::Spread(_) => self.unsupported_value(
                node.location(),
                "unexpanded call spread reached MIR lowering",
            ),
            // HIR lowers the construction the checker spelled for a
            // t-string in its place, so a checked program never reaches one.
            CheckedOp::TString { .. } => self.unsupported_value(
                node.location(),
                "a t-string reached MIR without the construction the checker built for it",
            ),
            CheckedOp::TypeValue { .. } => {
                self.unsupported_value(node.location(), "a type value reached MIR lowering")
            }
            _ => return None,
        })
    }

    /// Prefix operator `op` applied to `value`, the operand of node `node`.
    fn prefix_op(&mut self, node: Node<'a>, op: PrefixOp, value: Node<'a>) -> Reg {
        // `-x` and `~x` on a struct are its dunder's method call, as
        // upstream's `x.__neg__()` is.
        if op != PrefixOp::Not && node.call_contract().is_some() {
            return self.ordinary_method_call(node, value, op.dunder(), &[], &[]);
        }
        let ra = self.lower_operator_operand(node, value);
        // `not x` on a struct or a numeric scalar is
        // `not x.__bool__()`: convert through the explicit `Bool(x)`
        // call so the negation is scalar on both backends.
        let ra = if op == PrefixOp::Not
            && value.ty().is_some_and(|ty| {
                matches!(ty.kind(), TyKind::Struct(..))
                    || mojito_types::conformance::is_numeric(&ty)
                    || mojito_types::types::is_scalar_simd(&ty)
            }) {
            self.bool_conversion(value.location(), ra)
        } else {
            ra
        };
        let d = self.fresh(node.location(), None);
        let raises = self.raises(node.raises(), node.location());
        self.emit(MirInstr::UnOp {
            op,
            dest: d,
            a: ra,
            raises,
        });
        d
    }

    /// Call node `node`, written `name`: a crossing the elaborator left
    /// (`materialize[X]()`, `comptime(e)`) lowers its operand, any other
    /// call is an ordinary call.
    fn call_op(&mut self, node: Node<'a>, name: &str) -> Reg {
        let param_args = node.param_args();
        let args = node.args();
        let keywordless = node.kwargs().is_empty();
        // `materialize[X]()` the elaborator above MIR left, over a
        // binder: the runtime value of its compile-time operand.
        if name == "materialize"
            && args.is_empty()
            && keywordless
            && let [argument @ CheckedParamArg::Value(_)] = param_args
            && let Some(operand) = node.param_value(argument)
        {
            return self.crossing_operand(operand);
        }
        // `comptime(e)` left the same way.
        if name == "comptime"
            && param_args.is_empty()
            && keywordless
            && let [operand] = args.as_slice()
            && operand.ty().is_some()
        {
            return self.crossing_operand(*operand);
        }
        self.call_expr(node, name)
    }

    /// Walrus `name := value`, its name node `node`: bind the name's slot
    /// and yield the value.
    fn walrus(&mut self, value: Node<'a>, node: &CheckedExpr, name: &str) -> Reg {
        let value = self.expr(value);
        let var = self.var(&self.spelling(node, name));
        self.emit(MirInstr::DefVar {
            var,
            src: value,
            binding_ty: None,
            in_place: false,
        });
        value
    }

    /// Read name node `node`, written `name`.
    fn identifier_read(&mut self, node: Node<'a>, name: &str) -> Reg {
        let checked = node.checked;
        let name = self.spelling(checked, name);
        if let Some(target) = callable_target(&checked.adjustments) {
            return self.literal(checked, Const::Function(target));
        }
        if let Some(info) = checked
            .binding
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
        {
            if info.storage.is_some() && !node.closure_callable() {
                return self.closure_storage_read(node, &name, &info);
            }
            return self.load_nested_closure(&name, &info, checked.location());
        }
        let local = checked
            .binding
            .and_then(|owner| self.local_binding_var(owner, &name));
        if local.is_none() && self.overloads.is_function(&name) {
            return self.literal(checked, Const::Function(name));
        }
        // A module constant is the parameter value the check recorded.
        if let Some(value) = recorded_param_value(&checked.adjustments) {
            return self.param_value_register(node, value);
        }
        // In a lifted thunk, an enclosing `comptime for` index or local
        // `comptime` binding is the parameter expression it denotes.
        let denoted = checked.binding.and_then(|owner| {
            self.parameter_reads
                .get(&owner)
                .and_then(|binder| self.enclosing_binders.value_of(binder))
                .or_else(|| self.enclosing_binders.comptime_value(owner))
                .or_else(|| self.comptime_thunks.bound_value(owner))
        });
        if let Some(value) = denoted {
            return self.param_value_register(node, value);
        }
        let var = match (local, checked.binding) {
            (Some(var), _) => var,
            (None, Some(owner)) => self.binding_var(owner, &name),
            (None, None) => self.var(&name),
        };
        debug_assert!(
            self.slots.get(&checked.id).is_none_or(|slot| *slot == var),
            "HIR's slot for a name is its binding's"
        );
        self.read_slot(var, checked.location(), Some(checked))
    }

    /// Dictionary display `node` as upstream's
    /// `Dict(keys=[k…], values=[v…], __dict_literal__=None)`: the key and
    /// value registers become `List` literal constructions, which the `Dict`
    /// literal constructor consumes.
    fn dict_literal(
        &mut self,
        node: Node<'a>,
        target: &Ty,
        literal: &DictLiteral,
        keys: &[Reg],
        values: &[Reg],
    ) -> Reg {
        let (key_ty, value_ty) = dict_elements(target).expect("checked dictionary display target");
        let (keys_ty, values_ty) = (
            mojito_types::types::list_type(key_ty.clone()),
            mojito_types::types::list_type(value_ty.clone()),
        );
        let keys = self.literal_constructor_call(
            node,
            &literal.list_constructor,
            keys_ty,
            keys.to_vec(),
            Vec::new(),
            "__list_literal__",
        );
        let values = self.literal_constructor_call(
            node,
            &literal.list_constructor,
            values_ty,
            values.to_vec(),
            Vec::new(),
            "__list_literal__",
        );
        self.literal_constructor_call(
            node,
            &literal.constructor,
            target.clone(),
            Vec::new(),
            vec![("keys".to_string(), keys), ("values".to_string(), values)],
            "__dict_literal__",
        )
    }

    /// Display `node` as one call of the constructor its check selected:
    /// its `elements` positionally, then the literal `marker` keyword
    /// bound to `None` when it has one.
    fn display_construction(
        &mut self,
        node: Node<'a>,
        target: Ty,
        constructor: &str,
        marker: Option<&str>,
        elements: Vec<Reg>,
    ) -> Reg {
        match marker {
            Some(marker) => self.literal_constructor_call(
                node,
                constructor,
                target,
                elements,
                Vec::new(),
                marker,
            ),
            None => self.constructor_call(node, constructor, target, elements, Vec::new()),
        }
    }

    /// One call of a literal constructor `func` to a fresh register typed
    /// `ty`, instantiated at `ty`'s arguments: `args` positionally, then
    /// `kwargs`, then the `marker` keyword bound to `None`.
    fn literal_constructor_call(
        &mut self,
        node: Node<'a>,
        func: &str,
        ty: Ty,
        args: Vec<Reg>,
        mut kwargs: Vec<(String, Reg)>,
        marker: &str,
    ) -> Reg {
        let none = self.fresh_typed(node.location(), None, Ty::none());
        self.emit(MirInstr::Const {
            dest: none,
            k: Const::None,
        });
        kwargs.push((marker.to_string(), none));
        self.constructor_call(node, func, ty, args, kwargs)
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
