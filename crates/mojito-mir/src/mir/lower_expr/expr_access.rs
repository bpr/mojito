//! Member, subscript, slice, and variant-projection arms of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_ast::ast::SubscriptArg;
impl Flatten<'_> {
    /// The parameter expression the checker recorded for `e`, a compile-time
    /// query read as a runtime value.
    pub(in crate::mir) fn param_value(
        &self,
        e: &Expr,
    ) -> Option<mojito_types::param_expr::ParamExpr> {
        self.checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ParamValue { value } => Some(value),
                _ => None,
            })
    }

    /// The runtime value of a compile-time `operand` crossing explicitly
    /// (`materialize[X]()`, `comptime(e)`): a local display binding is built
    /// here, where it crosses; an `Int` or a `Bool` read off one is the
    /// application of its thunk; any other operand is lowered in place.
    pub(in crate::mir) fn crossing_operand(&mut self, operand: &Expr) -> Reg {
        if let Some(binding) = self.named_display(operand) {
            return self.crossing_display(&binding);
        }
        match self.display_read(operand) {
            Some((value, _)) => value,
            None => self.expr(operand),
        }
    }

    /// Lower `e`'s recorded parameter value: a closed one is its constant,
    /// and one a generator names is a parameter constant the elaborator
    /// folds per instance.
    pub(in crate::mir) fn param_value_register(
        &mut self,
        e: &Expr,
        value: mojito_types::param_expr::ParamExpr,
    ) -> Reg {
        let k = match value.kind() {
            mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Int(n)) => {
                Const::Int(*n)
            }
            // A literal a local `comptime` binding denotes (`comptime c =
            // 3`), read in a lifted thunk.
            mojito_types::param_expr::ParamKind::Constant(
                mojito_types::ct::CtValue::IntLiteral(n),
            ) => Const::IntLiteral(n.clone()),
            mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Bool(b)) => {
                Const::Bool(*b)
            }
            mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Str(s)) => {
                Const::Str(s.clone())
            }
            _ => Const::Param(value),
        };
        self.constant(e, k)
    }

    pub(super) fn member_expr(&mut self, e: &Expr, object: &Expr, field: &str) -> Reg {
        // `v.length` and `v.dtype` on a SIMD value (or `DType.<name>`) are
        // the lane slots: a known one is a constant, and one a generator
        // names is the evaluation of its parameter expression.
        let folded =
            self.checked_adjustments(e)
                .into_iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::SimdLength { width } => {
                        Some(match width {
                            mojito_types::types::SimdWidth::Known(width) => Ok(Const::Int(width)),
                            mojito_types::types::SimdWidth::Expr(expr) => Err(expr),
                        })
                    }
                    mojito_checked::checked::SemanticAdjustment::DtypeConstant { dtype } => {
                        Some(match dtype {
                            mojito_types::types::SimdDtype::Known(dtype) => Ok(Const::Dtype(dtype)),
                            mojito_types::types::SimdDtype::Expr(expr) => Err(expr),
                        })
                    }
                    _ => None,
                });
        if let Some(folded) = folded {
            // A value receiver (`f().dtype`) still runs; a type operand
            // or `DType` itself was never typed as a value.
            if !matches!(object.kind, ExprKind::Identifier(_)) && self.checked_ty(object).is_some()
            {
                self.expr(object);
            }
            return match folded {
                Ok(constant) => self.constant(e, constant),
                Err(expr) => {
                    if let Some(value) = self.inferred_value_register(&expr, &span(e)) {
                        return value;
                    }
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::Unsupported(format!(
                        "lane slot `{expr}` has no runtime form"
                    )));
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    dest
                }
            };
        }
        // A pure field chain rooted at a variable (`p.a`, `p.a.b`) lowers to
        // a `LoadPlace` (a place read) so the ownership analysis sees *which*
        // field is read — enabling field-sensitive partial-move checking
        // (reading `p.b` after `p.a^` stays legal). A member of a temporary
        // or an indexed base keeps the register-based `GetField`.
        if !self.is_slice_descriptor(object)
            && let Some(place) = self.pure_field_place(e)
        {
            let place_root = place.root;
            let place_ty = place.ty.clone();
            let loaded = self.fresh_typed(
                span(e),
                Some(place_root),
                place_ty
                    .clone()
                    .or_else(|| self.checked_ty(e))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: loaded,
                place,
            });
            // A field expression selected by the checker for a
            // consuming value context owns its result just like a
            // bare-variable `UseVar { Copy }`. Keep `LoadPlace` itself
            // handle-preserving for method receivers, borrowed call
            // arguments, iteration, and other explicit place
            // operations; make only the checked value-copy boundary
            // visible here so a nested lifecycle field runs its
            // `__copyinit__` instead of merely duplicating an owning
            // UnsafePointer.
            //
            // Reference-valued fields retain their existing handle/read
            // path.  Their ordinary referent copies are selected by the
            // checked `ReferenceResult` adjustment, not by this nominal
            // field rule.
            if !matches!(place_ty, Some(Ty::Ref(_)))
                && self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
                    )
                })
            {
                // The copy has the loaded register's exact type (a
                // public tuple field can keep a literal element where
                // the checked expression type defaulted it).
                let ty = self
                    .f
                    .reg_types
                    .get(&loaded.0)
                    .cloned()
                    .or_else(|| self.checked_ty(e))
                    .unwrap_or(Ty::Error);
                let copied = self.fresh_typed(span(e), Some(place_root), ty);
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: loaded,
                });
                copied
            } else {
                loaded
            }
        } else {
            let d = if let Some(loaded) = self.load_temporary_field(e, object, field) {
                loaded
            } else {
                let base = if self.reference_result(object).is_some() {
                    self.lower_call_receiver(object).0
                } else {
                    self.expr(object)
                };
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::GetField {
                    dest: d,
                    base,
                    field: field.to_owned(),
                });
                d
            };
            // The same checked value-copy boundary as the place-read
            // branch above: a field selected for a consuming value
            // context must run its `__copyinit__` even when the base is
            // a temporary or reference-projected call result, or the
            // register copy aliases the base's heap storage past its
            // lifetime.
            if !matches!(self.checked_ty(e), Some(Ty::Ref(_)))
                && self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
                    )
                })
            {
                // Type the loaded register from the checked expression
                // rather than leaving it to `GetField` instruction
                // typing: in a generic body the declaration's raw field
                // parameter would disagree with the copy's checked
                // type.
                if let Some(ty) = self.checked_ty(e) {
                    self.f.reg_types.insert(d.0, ty);
                }
                let copied =
                    self.fresh_typed(span(e), None, self.checked_ty(e).unwrap_or(Ty::Error));
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: d,
                });
                copied
            } else {
                d
            }
        }
    }

    pub(super) fn variant_project_index(&mut self, e: &Expr, object: &Expr) -> Reg {
        let index = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::VariantProject { index, .. } => {
                    Some(index)
                }
                _ => None,
            })
            .expect("checked Variant projection carries a tag");
        let mut place = self.place(object);
        if place.root_ty.is_none() {
            place.root_ty = Some(Ty::Variant(
                self.checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            ..
                        } => Some(alternatives),
                        _ => None,
                    })
                    .unwrap_or_default(),
            ));
        }
        let ty = self
            .checked_place_ty(e)
            .or_else(|| self.checked_ty(e))
            .expect("checked Variant projection has a payload type");
        place.project(Proj::Variant(index), ty);
        let root = place.root;
        let dest = self.fresh(span(e), Some(root));
        self.emit(MirInstr::LoadPlace { dest, place });
        // The checked value-copy boundary: a Copyable payload runs its
        // `__copyinit__` out of the variant's storage instead of
        // aliasing it past the owner's lifetime.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            let copied =
                self.fresh_typed(span(e), Some(root), self.checked_ty(e).unwrap_or(Ty::Error));
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: dest,
            });
            // Keep the owning variant alive through the copy: the
            // loaded register aliases its storage until `__copyinit__`
            // has produced the independent value.
            self.emit(MirInstr::KeepAlive { var: root });
            return copied;
        }
        dest
    }

    pub(super) fn index_expr(&mut self, e: &Expr, object: &Expr, index: &Expr) -> Reg {
        // An indexed reference-bearing aggregate element is a storage
        // place whose checked type is `ref T`; load through the stored
        // handle exactly like a direct reference field.  Ordinary
        // indexing remains the register-based operation below. A
        // checker-selected nominal accessor must stay on that dispatch
        // path: projecting the nominal struct as raw indexed storage
        // would lose its concrete `__getitem__$N` target.
        if matches!(self.checked_place_ty(e), Some(Ty::Ref(_)))
            && self.resolved_callable(e).is_none()
            && !matches!(self.checked_ty(object), Some(Ty::Struct(..)))
            && let Some(place) = self.try_place(e)
        {
            let d = self.fresh(span(e), Some(place.root));
            self.emit_interior_invalidations(e, None);
            self.emit(MirInstr::LoadPlace { dest: d, place });
            return d;
        }
        // Dereferencing an origin-bearing pointer reads its source
        // place; the checker fixed the offset to 0. A stably bound
        // pointer substitutes the owner place directly, keeping the
        // owner touched (and so droppable) at each access; otherwise
        // the access reads through the runtime handle.
        if let Some(place) = self.pointer_deref_place(object) {
            let d = self.fresh(span(e), Some(place.root));
            self.emit(MirInstr::LoadPlace { dest: d, place });
            return d;
        }
        if let Some(d) = self.temporary_pointer_deref(e, object) {
            return d;
        }
        // A pointer field of a call result the checker materialized
        // (`make(b).src[]`) dereferences through the temporary's
        // hidden slot, so the viewed storage outlives the read.
        if self.rooted_at_materialized_temporary(object)
            && let Some(place) = self.lower_projected_reference_place(e)
        {
            let d = self.fresh(span(e), Some(place.root));
            self.emit(MirInstr::LoadPlace { dest: d, place });
            return d;
        }
        if self.is_origin_bearing_pointer(object) {
            let reference = self.expr(object);
            let d = self.fresh(span(e), None);
            self.emit(MirInstr::ReadRef { dest: d, reference });
            return d;
        }
        if let Some(contract) = self.type_keyed_accessor_call(e) {
            let (recv, recv_place) = self.lower_call_receiver(object);
            let dest = self.fresh(span(e), None);
            self.emit_interior_invalidations(object, None);
            self.emit(MirInstr::MethodCall {
                dest,
                recv,
                method: "__getitem_param__".to_string(),
                resolved: Some(contract.target),
                raises: self.checked_raises(e),
                reference_result: contract.reference_result,
                result_adapter: contract.result_adapter,
                args: Vec::new(),
                kwargs: Vec::new(),
                recv_place,
                recv_writes: self.receiver_writes(e),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: self.checked_call_capture_accesses(e),
                param_arg_regs: Vec::new(),
                param_decls: contract.param_decls,
                instantiated_args: self.instantiated_args(e),
                spread: None,
            });
            return dest;
        }
        let has_call = self.checked_call_contract(e).is_some();
        if !has_call && let Some(d) = self.storage_element_read(e, object) {
            return d;
        }
        let (base, base_place) = if has_call {
            self.lower_call_receiver(object)
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let (idx, index_place) = self.lower_call_argument(index);
        let call = self.subscript_call_contract(e, &[(index.source_span(), idx)]);
        let intrinsic = call
            .is_none()
            .then(|| self.intrinsic_index_dispatch(object))
            .flatten();
        let d = self.fresh(span(e), None);
        self.emit_interior_invalidations(index, None);
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::Index {
            dest: d,
            base,
            index: idx,
            base_place,
            index_place,
            call,
            intrinsic,
        });
        d
    }

    pub(super) fn slice_expr(
        &mut self,
        e: &Expr,
        object: &Expr,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Reg {
        let has_call = self.checked_call_contract(e).is_some();
        let (obj, object_place) = if has_call {
            self.lower_call_receiver(object)
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let lower = lower.as_ref().map(|b| self.expr(b));
        let upper = upper.as_ref().map(|b| self.expr(b));
        let step = step.as_ref().map(|b| self.expr(b));
        let call = self.subscript_call_contract(e, &[]);
        // No intrinsic slice channel remains: StringLiteral
        // positional slicing was removed at the audited head, so
        // every checked slice routes through a nominal call.
        let intrinsic = None;
        let kind = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    ..
                } => descriptors.first().copied().flatten(),
                _ => None,
            })
            .expect("checked slice has a selected descriptor");
        let d = self.fresh(span(e), None);
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::Slice {
            dest: d,
            object: obj,
            kind,
            lower,
            upper,
            step,
            object_place,
            arg_places: vec![None],
            call,
            intrinsic,
        });
        d
    }

    pub(super) fn multi_index_expr(
        &mut self,
        e: &Expr,
        object: &Expr,
        args: &[SubscriptArg],
    ) -> Reg {
        // `ptr[unsafe_offset=i]` — the keyword spelling of pointer
        // indexed dereference — lowers exactly like `ptr[i]`: place
        // substitution or handle read for origin-bearing pointers,
        // else the pointer-intrinsic `Index` read.
        if matches!(self.checked_ty(object), Some(Ty::Pointer { .. }))
            && let [SubscriptArg::Keyword { name, value }] = args
            && name == "unsafe_offset"
        {
            if let Some(place) = self.pointer_deref_place(object) {
                let d = self.fresh(span(e), Some(place.root));
                self.emit(MirInstr::LoadPlace { dest: d, place });
                return d;
            }
            if let Some(d) = self.temporary_pointer_deref(e, object) {
                return d;
            }
            if self.is_origin_bearing_pointer(object) {
                let reference = self.expr(object);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::ReadRef { dest: d, reference });
                return d;
            }
            let base = self.expr(object);
            let base_place = self.simple_place(object);
            let (idx, index_place) = self.lower_call_argument(value);
            let intrinsic = self.intrinsic_index_dispatch(object);
            let d = self.fresh(span(e), None);
            self.emit(MirInstr::Index {
                dest: d,
                base,
                index: idx,
                base_place,
                index_place,
                call: None,
                intrinsic,
            });
            return d;
        }
        let has_call = self.checked_call_contract(e).is_some();
        let (object, object_place) = if has_call {
            self.lower_call_receiver(object)
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let descriptors = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    ..
                } => Some(descriptors),
                _ => None,
            })
            .expect("checked multi-subscript has descriptor metadata");
        let mut arg_places = Vec::with_capacity(args.len());
        let mut kwargs = Vec::new();
        let mut kwarg_places = Vec::new();
        let mut parameter_sources = Vec::new();
        let lowered_args: Vec<_> = args
            .iter()
            .zip(descriptors)
            .filter_map(|(argument, descriptor)| match argument {
                SubscriptArg::Keyword { name, value } => {
                    debug_assert!(descriptor.is_none());
                    let (register, place) = self.lower_call_argument(value);
                    kwarg_places.push(place);
                    parameter_sources.push((value.source_span(), register));
                    kwargs.push((name.clone(), MirSubscriptArg::Index(register)));
                    None
                }
                SubscriptArg::KeywordSlice {
                    name,
                    lower,
                    upper,
                    step,
                    ..
                } => {
                    kwarg_places.push(None);
                    kwargs.push((
                        name.clone(),
                        MirSubscriptArg::Slice {
                            kind: descriptor.expect("keyword slice argument has descriptor kind"),
                            lower: lower.as_ref().map(|value| self.expr(value)),
                            upper: upper.as_ref().map(|value| self.expr(value)),
                            step: step.as_ref().map(|value| self.expr(value)),
                        },
                    ));
                    None
                }
                SubscriptArg::Index(value) => {
                    debug_assert!(descriptor.is_none());
                    let (register, place) = self.lower_call_argument(value);
                    arg_places.push(place);
                    parameter_sources.push((value.source_span(), register));
                    Some(MirSubscriptArg::Index(register))
                }
                SubscriptArg::Slice {
                    lower, upper, step, ..
                } => {
                    arg_places.push(None);
                    Some(MirSubscriptArg::Slice {
                        kind: descriptor.expect("slice argument has descriptor kind"),
                        lower: lower.as_ref().map(|value| self.expr(value)),
                        upper: upper.as_ref().map(|value| self.expr(value)),
                        step: step.as_ref().map(|value| self.expr(value)),
                    })
                }
            })
            .collect();
        let call = self.subscript_call_contract(e, &parameter_sources);
        let dest = self.fresh(span(e), None);
        for argument in args {
            if let SubscriptArg::Index(argument) = argument {
                self.emit_interior_invalidations(argument, None);
            }
        }
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::MultiIndex {
            dest,
            object,
            args: lowered_args,
            object_place,
            arg_places,
            kwargs,
            kwarg_places,
            call,
        });
        dest
    }

    pub(super) fn variant_project_type_apply(&mut self, e: &Expr, name: &str) -> Reg {
        let index = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::VariantProject { index, .. } => {
                    Some(index)
                }
                _ => None,
            })
            .expect("checked Variant projection carries a tag");
        let mut place = self.resolved_place(name);
        if place.root_ty.is_none() {
            place.root_ty = Some(Ty::Variant(
                self.checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            ..
                        } => Some(alternatives),
                        _ => None,
                    })
                    .unwrap_or_default(),
            ));
        }
        let ty = self
            .checked_place_ty(e)
            .or_else(|| self.checked_ty(e))
            .expect("checked Variant projection has a payload type");
        place.project(Proj::Variant(index), ty);
        let root = place.root;
        let dest = self.fresh(span(e), Some(root));
        self.emit(MirInstr::LoadPlace { dest, place });
        // The checked value-copy boundary: a Copyable payload runs its
        // `__copyinit__` out of the variant's storage instead of
        // aliasing it past the owner's lifetime.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            let copied =
                self.fresh_typed(span(e), Some(root), self.checked_ty(e).unwrap_or(Ty::Error));
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: dest,
            });
            // Keep the owning variant alive through the copy: the
            // loaded register aliases its storage until `__copyinit__`
            // has produced the independent value.
            self.emit(MirInstr::KeepAlive { var: root });
            return copied;
        }
        dest
    }

    /// A value read of a homogeneous collector's element (`a[i]` over
    /// `*a: T`) loads the element place, as a field read does, and runs the
    /// checked value-copy boundary on that element alone; reading the
    /// collector as a value first would copy its whole storage.
    fn storage_element_read(&mut self, e: &Expr, object: &Expr) -> Option<Reg> {
        if !matches!(
            self.intrinsic_index_dispatch(object),
            Some(MirIntrinsicSubscript::VariadicStorage)
        ) {
            return None;
        }
        let place = self.try_place(e)?;
        let root = place.root;
        let loaded = self.fresh_typed(
            span(e),
            Some(root),
            place
                .ty
                .clone()
                .or_else(|| self.checked_ty(e))
                .unwrap_or(Ty::Error),
        );
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place,
        });
        if !self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            return Some(loaded);
        }
        let ty = self
            .f
            .reg_types
            .get(&loaded.0)
            .cloned()
            .or_else(|| self.checked_ty(e))
            .unwrap_or(Ty::Error);
        let copied = self.fresh_typed(span(e), Some(root), ty);
        self.emit(MirInstr::CopyValue {
            dest: copied,
            value: loaded,
        });
        Some(copied)
    }
}
