//! Expression-lowering entry points: HIR wrappers, place lowering,
//! and adjustment application.

use super::*;

impl Flatten<'_> {
    /// Post-order: each subexpression emits one instruction and yields its result
    /// `Reg`, so `foo(bar(x))` → `t0 = bar(x); t1 = foo(t0)`. Total over `Expr`.
    pub(in crate::mir) fn expr_hir(&mut self, expression: &mojito_hir::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.expr(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    /// A branch condition: `expr_hir` plus the `Boolable` conversion of a
    /// checker-marked truthiness condition (`if collection:`).
    pub(in crate::mir) fn condition_hir(&mut self, expression: &mojito_hir::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let value = self.expr(&expression.syntax);
        let result = self.truthiness(&expression.syntax, value);
        self.active_semantics.pop();
        result
    }

    pub(in crate::mir) fn reference_handle_hir(
        &mut self,
        expression: &mojito_hir::hir::HirExpr,
    ) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.reference_handle(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(in crate::mir) fn projected_reference_place_hir(
        &mut self,
        expression: &mojito_hir::hir::HirExpr,
    ) -> Option<MirPlace> {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.lower_projected_reference_place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(in crate::mir) fn place_hir(&mut self, expression: &mojito_hir::hir::HirExpr) -> MirPlace {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(in crate::mir) fn expr(&mut self, e: &Expr) -> Reg {
        let result = self.expr_with_adjustments(e);
        // An emit-site type (a conversion result, a closure value) is more
        // precise than the source expression's pre-adjustment checked type.
        if let Some(ty) = self.checked_ty(e) {
            self.f.reg_types.entry(result.0).or_insert(ty);
        }
        result
    }

    pub(in crate::mir) fn expr_with_adjustments(&mut self, e: &Expr) -> Reg {
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowShared
                    | mojito_checked::checked::SemanticAdjustment::BorrowMutable
            )
        }) {
            return self.reference_handle(e);
        }
        if let Some(reference) = self.reference_result(e) {
            let handle = self.reference_handle(e);
            let value_ty = match self.f.reg_types.get(&handle.0) {
                Some(Ty::Ref(reference)) => (*reference.referent).clone(),
                _ => (*reference.referent).clone(),
            };
            let read = self.fresh_typed(span(e), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            // A reference-returning expression has two checked uses. `ref x =
            // expression` is intercepted by the Borrow adjustment above and
            // retains its handle. Every ordinary value use reads the referent
            // into independently owned storage, so lifecycle types must run
            // their copy initializer rather than alias backing storage.
            let dest = self.fresh_typed(span(e), None, value_ty);
            self.emit(MirInstr::CopyValue { dest, value: read });
            return dest;
        }
        if let Some(target) = self.index_normalization(e) {
            // The source Indexer is evaluated exactly once. The checked target
            // may be concrete or an abstract trait-dispatch symbol; MethodCall
            // already retargets the latter from the runtime receiver while
            // preserving the selected signature.
            let recv = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(recv.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, Ty::Int);
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
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            });
            return dest;
        }
        if let Some(target) = self.implicit_conversion(e) {
            // A view-constructor conversion (`BorrowConversionSource`) binds
            // its `ref [origin]` parameter to the source's caller place, and
            // the result register keeps the source root's provenance so the
            // borrowed owner stays live across the consuming expression.
            let source_place = self
                .checked_adjustments(e)
                .iter()
                .any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::BorrowConversionSource { .. }
                    )
                })
                .then(|| self.simple_place(e))
                .flatten();
            let argument = self.expr_unconverted(e);
            // The conversion result is the constructed type, not the source
            // expression's checked type; targets are concrete constructors.
            // The checker records the converted-to type (with its arguments,
            // `Optional[Int]`); the constructor's bare name is the fallback.
            let provenance = source_place.as_ref().map(|place| place.root);
            let recorded = self
                .checked_adjustments(e)
                .into_iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ConversionResultType(ty) => {
                        Some(ty)
                    }
                    _ => None,
                });
            let dest = match (recorded, target.split(".__init__").next()) {
                (Some(ty @ Ty::Struct(..)), _) => self.fresh_typed(span(e), provenance, ty),
                (_, Some(constructed)) if !constructed.is_empty() => self.fresh_typed(
                    span(e),
                    provenance,
                    Ty::Struct(constructed.to_string(), Vec::new()),
                ),
                _ => self.fresh(span(e), provenance),
            };
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises: None,
                args: vec![argument],
                kwargs: Vec::new(),
                arg_places: vec![source_place.clone()],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
            });
            if source_place.is_some() {
                // A view-constructor conversion result borrows its source: bind
                // the temporary into a hidden retained slot whose loan keeps
                // the source alive (and conflict-checked) until the consuming
                // expression's last use of the view — the same persistent
                // representation an explicit `var sp = Span(xs)` binding gets.
                let loans = self.aggregate_borrows(e);
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
                    });
                    let marker = self.fresh_typed(span(e), Some(loans[0].place.root), Ty::None);
                    self.emit(MirInstr::EstablishLoans {
                        reference: variable,
                        loans: loans.clone(),
                        marker,
                        dest_interior: None,
                    });
                    self.aggregate_loans.insert(variable, loans);
                    let read = match view_ty {
                        Some(ty) => self.fresh_typed(span(e), Some(variable), ty),
                        None => self.fresh(span(e), Some(variable)),
                    };
                    self.emit(MirInstr::UseVar {
                        dest: read,
                        var: variable,
                        mode: UseMode::Copy,
                    });
                    return read;
                }
            }
            return dest;
        }
        if let Some(target) = self.literal_materialization(e) {
            let value = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(value.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, target.clone());
            self.emit(MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            });
            return dest;
        }
        self.expr_unconverted(e)
    }

    pub(in crate::mir) fn reference_result(
        &self,
        expression: &Expr,
    ) -> Option<mojito_types::origin::RefTy> {
        // The selected-call contract is the canonical checked handoff.  In
        // particular, a free-function reference result may share its source
        // expression with another compatibility adjustment, so consulting only
        // the legacy single-operation slot can silently lose `ref[a, b] T` and
        // type a runtime handle local as ordinary `T` storage.
        self.checked_call_contract(expression)
            .and_then(|contract| contract.reference_result)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                            reference,
                        } => Some(reference),
                        _ => None,
                    })
            })
    }
}
