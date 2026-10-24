//! Expression-lowering entry points: HIR wrappers, place lowering,
//! and adjustment application.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    /// Post-order: each subexpression emits one instruction and yields its result
    /// `Reg`, so `foo(bar(x))` → `t0 = bar(x); t1 = foo(t0)`. Total over `Expr`.
    pub(in crate::mir) fn expr_hir(&mut self, expression: &'a mojito_hir::hir::HirExpr) -> Reg {
        self.with_statement_loans(|this| this.expr(this.handle(expression)))
    }

    /// A branch condition: `expr_hir` plus the `Boolable` conversion of a
    /// checker-marked truthiness condition (`if collection:`).
    pub(in crate::mir) fn condition_hir(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
    ) -> Reg {
        self.with_statement_loans(|this| {
            let node = this.handle(expression);
            let value = this.expr(node);
            this.truthiness(node, value)
        })
    }

    /// A `comptime if` condition as the constraint its branch carries: the
    /// one the checker compiled and recorded on the expression, else its
    /// connectives over the leaves, each a recorded constraint or the thunk
    /// this function's lowering lifts for it.
    pub(in crate::mir) fn comptime_condition(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
    ) -> mojito_types::types::GenericConstraint {
        self.with_statement_loans(|this| this.comptime_condition_of(this.handle(expression)))
    }

    /// The handle of HIR expression `expression`.
    pub(in crate::mir) fn handle(&self, expression: &'a mojito_hir::hir::HirExpr) -> Node<'a> {
        Node::new(expression, self.checked)
    }

    /// The checked node of HIR expression `expression`.
    pub(in crate::mir) fn hir_node(
        &self,
        expression: &mojito_hir::hir::HirExpr,
    ) -> &mojito_checked::checked::CheckedExpr {
        self.checked
            .expression(expression.node)
            .expect("a HIR expression names a node of its tables")
    }

    /// The runtime spelling of name node `node`, written `name`, in this
    /// function's frame ([`name_spelling`]).
    pub(in crate::mir) fn spelling(
        &self,
        node: &mojito_checked::checked::CheckedExpr,
        name: &str,
    ) -> String {
        name_spelling(&self.slots, &self.vars, node).unwrap_or_else(|| name.to_owned())
    }

    pub(in crate::mir) fn comptime_condition_of(
        &mut self,
        expression: Node<'a>,
    ) -> mojito_types::types::GenericConstraint {
        use mojito_checked::checked::CheckedOp;
        use mojito_types::types::GenericConstraint;
        let recorded = expression
            .adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ComptimeCondition(constraint) => {
                    Some(constraint)
                }
                _ => None,
            })
            .cloned();
        if let Some(constraint) = recorded {
            // A binder the condition names is the enclosing declaration's
            // of that spelling: a fact carried from an earlier discovery
            // round may spell the declaration under its overload-qualified
            // symbol, and the body is lowered under the symbol it has now.
            return constraint.map(
                &|reference| {
                    self.enclosing_binders
                        .declarations
                        .iter()
                        .rev()
                        .find(|declaration| declaration.name() == reference.name.as_ref())
                        .map_or_else(|| reference.clone(), ParamDecl::binder)
                },
                &Clone::clone,
            );
        }
        let operands: Vec<_> = expression.operands().collect();
        match (expression.op(), operands.as_slice()) {
            (CheckedOp::Bool(value), _) => GenericConstraint::Bool(*value),
            (CheckedOp::Prefix(PrefixOp::Not, _), [inner]) => {
                GenericConstraint::Not(Box::new(self.comptime_condition_of(*inner)))
            }
            (CheckedOp::Infix(InfixOp::And, ..), [left, right]) => GenericConstraint::And(
                Box::new(self.comptime_condition_of(*left)),
                Box::new(self.comptime_condition_of(*right)),
            ),
            (CheckedOp::Infix(InfixOp::Or, ..), [left, right]) => GenericConstraint::Or(
                Box::new(self.comptime_condition_of(*left)),
                Box::new(self.comptime_condition_of(*right)),
            ),
            _ => {
                let binders = &self.enclosing_binders;
                self.comptime_thunks.request(expression.id(), binders)
            }
        }
    }

    pub(in crate::mir) fn reference_handle_hir(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
    ) -> Reg {
        self.with_statement_loans(|this| this.reference_handle(this.handle(expression)))
    }

    pub(in crate::mir) fn projected_reference_place_hir(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
    ) -> Option<MirPlace> {
        self.with_statement_loans(|this| {
            this.lower_projected_reference_place(this.handle(expression))
        })
    }

    pub(in crate::mir) fn place_hir(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
    ) -> MirPlace {
        self.with_statement_loans(|this| this.place(this.handle(expression)))
    }

    pub(in crate::mir) fn expr(&mut self, e: Node<'a>) -> Reg {
        let result = self.expr_with_adjustments(e);
        let result = match e.rebind_target() {
            Some((operand, dest)) => self.rebind_value(e, result, operand, dest),
            None => result,
        };
        // An emit-site type (a conversion result, a closure value) is more
        // precise than the source expression's pre-adjustment checked type.
        if let Some(ty) = e.ty() {
            self.f.reg_types.entry(result.0).or_insert(ty);
        }
        result
    }

    /// `rebind[dest](e)` read as a value: `value` holds the operand at its
    /// own type, or already at `dest` when it was read through its rebound
    /// place ([`Self::rebound_place`]).
    fn rebind_value(&mut self, e: Node<'a>, value: Reg, operand: Ty, dest: Ty) -> Reg {
        let source = self.f.reg_types.entry(value.0).or_insert(operand).clone();
        if source == dest {
            return value;
        }
        let rebound = self.fresh_typed(e.location(), None, dest);
        self.emit(MirInstr::Rebind {
            dest: rebound,
            value,
        });
        rebound
    }

    pub(in crate::mir) fn expr_with_adjustments(&mut self, e: Node<'a>) -> Reg {
        if e.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowShared
                    | mojito_checked::checked::SemanticAdjustment::BorrowMutable
            )
        }) {
            return self.reference_handle(e);
        }
        // A conversion of an index-normalized source applies after the
        // normalization below; every other conversion reads its source here,
        // a reference result included.
        if e.index_normalization().is_none()
            && let Some(target) = e.implicit_conversion()
        {
            // A view-constructor conversion (`BorrowConversionSource`) binds
            // its `ref [origin]` parameter to the source's caller place, and
            // the result register keeps the source root's provenance so the
            // borrowed owner stays live across the consuming expression.
            // A temporary source (`f(String("abc"))` at a `StringSpan`
            // parameter) has no place: the checker minted an anonymous owner,
            // and the unconverted value is stored in that owner's hidden
            // `$mat_r` slot whose place the constructor borrows.
            let adjustments = e.adjustments();
            let (argument, source_place) = if let Some(owner) =
                mojito_checked::checked::materialized_borrow_owner(adjustments)
            {
                let value = self.expr_unconverted(e);
                // The slot holds the source at its own checked type (a
                // constructor call's register is typed at its emit site).
                if let Some(ty) = e.ty() {
                    self.f.reg_types.entry(value.0).or_insert(ty);
                }
                let value = self.nominal_string_view_source(e, value, &target);
                self.materialize_borrow_slot(e, owner, value)
            } else {
                let borrows_source = adjustments.iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::BorrowConversionSource { .. }
                    )
                });
                if borrows_source {
                    self.borrowed_conversion_source(e)
                } else {
                    let value = self
                        .reference_result_value(e)
                        .unwrap_or_else(|| self.expr_unconverted(e));
                    // The source keeps its own checked type: a generic call's
                    // register is otherwise typed by its declared result.
                    if let Some(ty) = e.ty() {
                        self.f.reg_types.entry(value.0).or_insert(ty);
                    }
                    (value, None)
                }
            };
            // The conversion result is the constructed type, not the source
            // expression's checked type; targets are concrete constructors.
            // The checker records the converted-to type (with its arguments,
            // `Optional[Int]`); the constructor's bare name is the fallback.
            let provenance = source_place.as_ref().map(|place| place.root);
            let recorded = e
                .adjustments()
                .iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ConversionResultType(ty) => {
                        Some(ty)
                    }
                    _ => None,
                })
                .cloned();
            let dest = match (recorded, target.split(".__init__").next()) {
                (Some(ty), _) => self.fresh_typed(e.location(), provenance, ty),
                (_, Some(constructed)) if !constructed.is_empty() => self.fresh_typed(
                    e.location(),
                    provenance,
                    TyKind::Struct(constructed.to_string(), Vec::new()).into(),
                ),
                _ => self.fresh(e.location(), provenance),
            };
            let raises = e
                .adjustments()
                .iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ConversionRaises(error) => {
                        Some(error)
                    }
                    _ => None,
                })
                .cloned();
            let raises = self.raises(raises, e.location());
            let instantiated_args = e
                .adjustments()
                .iter()
                .find_map(|adjustment| {
                    match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ConversionInstantiatedArguments(
                        arguments,
                    ) => Some(arguments.clone()),
                    _ => None,
                }
                })
                .unwrap_or_default();
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises,
                args: vec![argument],
                kwargs: Vec::new(),
                arg_places: vec![source_place.clone()],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                receiver: None,
                instantiated_args,
                spread: None,
            });
            if let Some(source_place) = source_place {
                // A view-constructor conversion result borrows its source: bind
                // the temporary into a hidden retained slot whose loan keeps
                // the source alive (and conflict-checked) until the consuming
                // expression's last use of the view — the same persistent
                // representation an explicit `var sp = Span(xs)` binding gets.
                let loans = self.conversion_source_loans(e, source_place);
                if !loans.is_empty() {
                    let view_ty = self.f.reg_types.get(&dest.0).cloned();
                    let variable = self.var(&format!("$conv_view_r{}", dest.0));
                    if let Some(ty) = view_ty.clone() {
                        self.var_types.insert(variable, ty);
                    }
                    self.emit(MirInstr::DefVar {
                        var: variable,
                        src: dest,
                        binding_ty: view_ty.clone(),
                        in_place: false,
                    });
                    let marker =
                        self.fresh_typed(e.location(), Some(loans[0].place.root), Ty::none());
                    self.emit(MirInstr::EstablishLoans {
                        reference: variable,
                        loans: loans.clone(),
                        marker,
                        dest_interior: None,
                    });
                    if self.simple_place(e).is_none() {
                        self.conversion_view_loans
                            .insert(e.address(), loans.clone());
                    }
                    self.aggregate_loans.insert(variable, loans);
                    let read = match view_ty {
                        Some(ty) => self.fresh_typed(e.location(), Some(variable), ty),
                        None => self.fresh(e.location(), Some(variable)),
                    };
                    self.emit(MirInstr::UseVar {
                        dest: read,
                        var: variable,
                        mode: UseMode::Copy,
                        constructor: None,
                    });
                    return read;
                }
            }
            return dest;
        }
        if let Some(value) = self.reference_result_value(e) {
            return value;
        }
        if let Some(target) = e.index_normalization() {
            // The source Indexer is evaluated exactly once. The checked target
            // may be concrete or an abstract trait-dispatch symbol; MethodCall
            // already retargets the latter from the runtime receiver while
            // preserving the selected signature.
            let recv = self.expr_unconverted(e);
            if let Some(source) = e.ty() {
                self.f.reg_types.entry(recv.0).or_insert(source);
            }
            let dest = self.fresh_typed(e.location(), None, Ty::int());
            self.emit(MirInstr::MethodCall {
                dest,
                recv,
                method: "__mlir_index__".to_string(),
                resolved: Some(target),
                raises: None,
                reference_result: None,
                result_adapter: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                recv_place: None,
                recv_writes: false,
                receiver_convention: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_args: Vec::new(),
                spread: None,
            });
            return dest;
        }
        if let Some(target) = e.literal_materialization() {
            let value = self.expr_unconverted(e);
            if let Some(source) = e.ty() {
                self.f.reg_types.entry(value.0).or_insert(source);
            }
            return self.materialize_literal(value, &target, e.location());
        }
        if let Some(target) = e.scalar_splat() {
            let value = self.expr_unconverted(e);
            if let Some(source) = e.ty() {
                self.f.reg_types.entry(value.0).or_insert(source);
            }
            return self.splat_scalar(value, &target, e.location());
        }
        self.expr_unconverted(e)
    }

    /// Whether a method call's reference result has an origin rooted at one
    /// of the call's own arguments — a place, or the hidden slot a
    /// temporary argument materialized into — rather than at its receiver.
    fn reference_result_borrows_argument(&self, expression: Node<'a>) -> bool {
        let CheckedOp::MethodCall { args, kwargs, .. } = expression.op() else {
            return false;
        };
        let Some(reference) = expression.reference_result() else {
            return false;
        };
        let mut roots = Vec::new();
        origin_place_roots(&reference.origin, &mut roots);
        args.iter()
            .chain(kwargs.iter().map(|keyword| &keyword.value))
            .filter_map(|argument| {
                self.place_root_binding(*argument).or_else(|| {
                    self.checked.expression(*argument).and_then(|node| {
                        mojito_checked::checked::materialized_borrow_owner(&node.adjustments)
                    })
                })
            })
            .any(|owner| roots.contains(&owner))
    }

    /// The binding place node `place` is rooted at: through every field
    /// and subscript to the root's own binding.
    fn place_root_binding(
        &self,
        place: mojito_checked::checked::CheckedNodeId,
    ) -> Option<mojito_types::origin::OwnerId> {
        let node = self.checked.expression(place)?;
        match &node.op {
            CheckedOp::Member { object, .. } | CheckedOp::Index { object, .. } => {
                self.place_root_binding(*object)
            }
            _ => node.binding,
        }
    }

    /// The value of a reference-returning expression, read out of its
    /// referent into independently owned storage; `None` when `e` returns no
    /// reference.
    fn reference_result_value(&mut self, e: Node<'a>) -> Option<Reg> {
        // A free call's reference result is read out of a hidden handle slot
        // that loans the storage its origin names, so the arguments it
        // borrows from stay alive until the referent is copied. A method
        // call's result takes the same path when its origin names an argument
        // (`pick[o: Origin](self, ref[o] x: T) -> ref[o] T`), and so does
        // every operand read at its call's setup, so the slot's setup loans
        // keep its owners live until the read.
        if (matches!(e.op(), CheckedOp::Call { .. })
            || self.reference_result_borrows_argument(e)
            || self.awaits_setup_read(e))
            && e.reference_result().is_some()
            && let Some(place) = self.materialize_reference_result_place(e)
        {
            let value_ty = place
                .ty
                .clone()
                .or_else(|| e.ty())
                .unwrap_or_else(Ty::error);
            let read = self.fresh_typed(e.location(), Some(place.root), value_ty.clone());
            self.emit(MirInstr::LoadPlace { dest: read, place });
            let dest = self.fresh_typed(e.location(), None, value_ty);
            self.emit(MirInstr::CopyValue {
                dest,
                value: read,
                constructor: None,
            });
            return Some(dest);
        }
        if let Some(reference) = e.reference_result() {
            let handle = self.reference_handle(e);
            let value_ty = match self.f.reg_types.get(&handle.0).map(Ty::kind) {
                Some(TyKind::Ref(reference)) => reference.referent.clone(),
                _ => reference.referent,
            };
            let read = self.fresh_typed(e.location(), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            // A reference-returning expression has two checked uses. `ref x =
            // expression` is intercepted by the Borrow adjustment above and
            // retains its handle. Every ordinary value use reads the referent
            // into independently owned storage, so lifecycle types must run
            // their copy initializer rather than alias backing storage.
            let dest = self.fresh_typed(e.location(), None, value_ty);
            self.emit(MirInstr::CopyValue {
                dest,
                value: read,
                constructor: None,
            });
            return Some(dest);
        }
        None
    }

    /// The source of a view-constructor conversion (`BorrowConversionSource`)
    /// with the caller place its `ref [origin]` parameter binds: a named place
    /// as written, or the element place an accessor's reference result or an
    /// intrinsic subscript (`parts[i]` over a collector) names, evaluated once.
    fn borrowed_conversion_source(&mut self, e: Node<'a>) -> (Reg, Option<MirPlace>) {
        if let Some(place) = self.simple_place(e) {
            return (self.expr_unconverted(e), Some(place));
        }
        let Some(place) = self
            .materialize_reference_result_place(e)
            .or_else(|| self.lower_projected_reference_place(e))
            .or_else(|| self.try_place(e))
        else {
            return (self.expr_unconverted(e), None);
        };
        let value = self.fresh_typed(
            e.location(),
            Some(place.root),
            place
                .ty
                .clone()
                .or_else(|| e.ty())
                .unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        (value, Some(place))
    }

    /// The loans a view-constructor conversion result holds on its borrowed
    /// source: the checked whole-place loan of a named place, the loans of
    /// the hidden handle an accessor's reference result lives in, or the
    /// element place an intrinsic subscript names.
    fn conversion_source_loans(&mut self, e: Node<'a>, source: MirPlace) -> Vec<MirLoan> {
        let loans = self.aggregate_borrows(e);
        if !loans.is_empty() {
            return loans;
        }
        // A setup handle's loans become the view's ordinary loans: the view
        // is built at setup and lives as a borrow.
        if let Some(handle) = source.through {
            return self
                .aggregate_loans
                .get(&handle)
                .into_iter()
                .flatten()
                .map(|loan| MirLoan {
                    setup: false,
                    ..loan.clone()
                })
                .collect();
        }
        let mutable = e.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowConversionSource {
                    mutable: true
                }
            )
        });
        vec![MirLoan {
            place: source,
            mutable,
            interior: None,
            shared: false,
            setup: false,
        }]
    }

    /// A builtin string producer (`String("abc")`, `repr(x)`, `input(...)`)
    /// is typed as the compile-time string plus a nominal-String wrap
    /// conversion at its own span; a view conversion of that temporary
    /// (`f(String("abc"))` at a `StringSpan` parameter) shares the span and
    /// supersedes the wrap in the recorded conversions. The view constructor
    /// borrows a nominal `String`, so the wrap is re-emitted here: the
    /// materialized slot must hold the struct the constructor lends.
    fn nominal_string_view_source(&mut self, e: Node<'a>, value: Reg, target: &str) -> Reg {
        if !matches!(
            self.f.reg_types.get(&value.0).map(Ty::kind),
            Some(TyKind::StringLiteral)
        ) || !mojito_symbol::symbol::single_nominal_string_ctor(target)
        {
            return value;
        }
        let nominal = self.fresh_typed(
            e.location(),
            None,
            TyKind::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                Vec::new(),
            )
            .into(),
        );
        self.emit(MirInstr::Call {
            dest: nominal,
            func: FuncRef::named(&mojito_symbol::symbol::nominal_string_literal_ctor_symbol()),
            raises: None,
            args: vec![value],
            kwargs: Vec::new(),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        nominal
    }
}

fn origin_place_roots(
    origin: &mojito_types::origin::Origin,
    roots: &mut Vec<mojito_types::origin::OwnerId>,
) {
    match origin {
        mojito_types::origin::Origin::Place(place) => roots.push(place.root),
        mojito_types::origin::Origin::Union(members) => members
            .iter()
            .for_each(|member| origin_place_roots(member, roots)),
        _ => {}
    }
}
