//! Member, subscript, slice, and variant-projection arms of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CheckedExpr, CheckedNodeId, CheckedSliceLiteral, CheckedSubscriptArg, SemanticAdjustment,
};
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    /// The runtime value of a compile-time `operand` crossing explicitly
    /// (`materialize[X]()`, `comptime(e)`): a local display binding is built
    /// here, where it crosses; an `Int` or a `Bool` read off one is the
    /// application of its thunk; any other operand is lowered in place.
    pub(in crate::mir) fn crossing_operand(&mut self, operand: Node<'a>) -> Reg {
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
        e: Node<'a>,
        value: mojito_types::param_expr::ParamExpr,
    ) -> Reg {
        self.param_value_register_at(e.location(), e.ty(), value)
    }

    /// [`Self::param_value_register`] at `location`, typed `ty` when the
    /// value has a checked type.
    pub(in crate::mir) fn param_value_register_at(
        &mut self,
        location: SourceSpan,
        ty: Option<Ty>,
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
            mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Float(
                bits,
            )) => Const::Float(f64::from_bits(*bits)),
            mojito_types::param_expr::ParamKind::Constant(
                mojito_types::ct::CtValue::FloatLiteral(value),
            ) => Const::FloatLiteral(value.clone()),
            _ => Const::Param(value),
        };
        self.constant_at(location, ty, k)
    }

    /// Read member node `e`, `object.field`.
    pub(super) fn member_expr(&mut self, e: Node<'a>, object: Node<'a>, field: &str) -> Reg {
        let node = e.checked;
        // `v.length` and `v.dtype` on a SIMD value (or `DType.<name>`) are
        // the lane slots: a known one is a constant, and one a generator
        // names is the evaluation of its parameter expression.
        let folded = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                SemanticAdjustment::SimdLength { width } => Some(match width.clone() {
                    mojito_types::types::SimdWidth::Known(width) => Ok(Const::Int(width)),
                    mojito_types::types::SimdWidth::Expr(expr) => Err(expr),
                }),
                SemanticAdjustment::DtypeConstant { dtype } => Some(match dtype.clone() {
                    mojito_types::types::SimdDtype::Known(dtype) => Ok(Const::Dtype(dtype)),
                    mojito_types::types::SimdDtype::Expr(expr) => Err(expr),
                }),
                _ => None,
            });
        if let Some(folded) = folded {
            // A value receiver (`f().dtype`) still runs; a type operand
            // or `DType` itself was never typed as a value.
            if !object.is_name() && object.ty().is_some() {
                self.expr(object);
            }
            return match folded {
                Ok(constant) => self.constant(e, constant),
                Err(expr) => {
                    if let Some(value) = self.inferred_value_register(&expr, &e.location()) {
                        return value;
                    }
                    let dest = self.fresh(e.location(), None);
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
        if let Some(place) = self.pure_field_place(e) {
            let place_root = place.root;
            let place_ty = place.ty.clone();
            let loaded = self.fresh_typed(
                e.location(),
                Some(place_root),
                place_ty
                    .clone()
                    .or_else(|| node.ty.clone())
                    .unwrap_or_else(Ty::error),
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
            // A direct `ref` field's load reads through to its referent,
            // whose handle a method receiver or a place operation keeps. A
            // value use, a copy or a read argument, is the referent, as a
            // `ref` binding's is.
            if let Some(TyKind::Ref(reference)) = place_ty.as_ref().map(Ty::kind) {
                if (copies_place_value(node) || reads_argument(node))
                    && !matches!(node.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                {
                    self.f
                        .reg_types
                        .insert(loaded.0, reference.referent.clone());
                } else {
                    return loaded;
                }
            }
            if copies_place_value(node) {
                // The copy has the loaded register's exact type (a
                // public tuple field can keep a literal element where
                // the checked expression type defaulted it).
                let ty = self
                    .f
                    .reg_types
                    .get(&loaded.0)
                    .cloned()
                    .or_else(|| node.ty.clone())
                    .unwrap_or_else(Ty::error);
                let copied = self.fresh_typed(e.location(), Some(place_root), ty);
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: loaded,
                    constructor: None,
                });
                copied
            } else {
                loaded
            }
        } else {
            let d = if let Some(loaded) = self.load_temporary_field(e, object, field) {
                loaded
            } else {
                let base = if object.reference_result().is_some() {
                    self.lower_call_receiver(object).0
                } else {
                    self.expr(object)
                };
                let d = self.fresh(e.location(), None);
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
            if !matches!(node.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                && copies_place_value(node)
            {
                // Type the loaded register from the checked expression
                // rather than leaving it to `GetField` instruction
                // typing: in a generic body the declaration's raw field
                // parameter would disagree with the copy's checked
                // type.
                if let Some(ty) = node.ty.clone() {
                    self.f.reg_types.insert(d.0, ty);
                }
                let copied = self.fresh_typed(
                    e.location(),
                    None,
                    node.ty.clone().unwrap_or_else(Ty::error),
                );
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: d,
                    constructor: None,
                });
                copied
            } else {
                d
            }
        }
    }

    /// Read `Variant` projection node `e`, `object[T]`.
    pub(super) fn variant_project_index(&mut self, e: Node<'a>, object: Node<'a>) -> Reg {
        let place = self.place(object);
        self.variant_payload_read(e, place)
    }

    /// Read subscript node `e`, `object[index]`.
    pub(super) fn index_expr(&mut self, e: Node<'a>, object: Node<'a>, index: Node<'a>) -> Reg {
        // An indexed reference-bearing aggregate element is a storage
        // place whose checked type is `ref T`; load through the stored
        // handle exactly like a direct reference field.  Ordinary
        // indexing remains the register-based operation below. A
        // checker-selected nominal accessor must stay on that dispatch
        // path: projecting the nominal struct as raw indexed storage
        // would lose its concrete `__getitem__$N` target.
        if matches!(e.place_ty().as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
            && e.resolved_callable().is_none()
            && !matches!(object.ty().as_ref().map(Ty::kind), Some(TyKind::Struct(..)))
            && let Some(place) = self.try_place(e)
        {
            let d = self.fresh(e.location(), Some(place.root));
            self.emit_interior_invalidations(e, None);
            self.emit(MirInstr::LoadPlace { dest: d, place });
            return d;
        }
        if is_pointer(object) && e.call_contract().is_none() {
            return self.pointer_subscript_read(e, object, index);
        }
        // A field of a call result the checker materialized
        // (`make(b).t[0]`) is indexed through the temporary's hidden
        // slot, so the viewed storage outlives the read.
        if object.rooted_at_materialized_temporary()
            && let Some(place) = self.lower_projected_reference_place(e)
        {
            let d = self.fresh(e.location(), Some(place.root));
            self.emit(MirInstr::LoadPlace { dest: d, place });
            return d;
        }
        if let Some(contract) = self.type_keyed_accessor_call(e) {
            let (recv, recv_place) = self.lower_method_receiver(e, object);
            let dest = self.fresh(e.location(), None);
            self.emit_interior_invalidations(object, None);
            let raises = self.raises(e.raises(), e.location());
            self.emit(MirInstr::MethodCall {
                dest,
                recv,
                method: "__getitem_param__".to_string(),
                resolved: Some(contract.target),
                raises,
                reference_result: contract.reference_result.map(Box::new),
                result_adapter: contract.result_adapter.map(MirResultAdapter::new),
                args: Vec::new(),
                kwargs: Vec::new(),
                recv_place: recv_place.map(Box::new),
                recv_writes: e.receiver_writes(),
                receiver_convention: e.receiver_convention(object),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: self.call_capture_accesses(e),
                param_arg_regs: Vec::new(),
                param_decls: contract.param_decls,
                instantiated_args: e.instantiated_args(),
                spread: None,
            });
            return dest;
        }
        let has_call = e.call_contract().is_some();
        if !has_call && let Some(d) = self.storage_element_read(e, object) {
            return d;
        }
        let (base, base_place) = if has_call {
            self.lower_setup_receiver_with(e, object, |this| this.lower_method_receiver(e, object))
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let (idx, index_place) = self.lower_call_argument(index);
        if has_call {
            self.close_call_setup(None);
        }
        let call = self.subscript_call_contract(e, &[MirSubscriptArg::Index(idx)]);
        let intrinsic = call
            .is_none()
            .then(|| object.intrinsic_index_dispatch())
            .flatten();
        let d = self.fresh(e.location(), None);
        self.emit_interior_invalidations(index, None);
        self.emit_interior_invalidations(e, None);
        self.record_call(d, call.as_ref().map(|call| call.target.clone()), 0);
        self.emit(MirInstr::Index {
            dest: d,
            base,
            index: idx,
            base_place: base_place.map(Box::new),
            index_place: index_place.map(Box::new),
            call: call.map(Box::new),
            intrinsic,
        });
        d
    }

    /// Read slice node `e`, `object[lower:upper:step]`.
    pub(super) fn slice_expr(
        &mut self,
        e: Node<'a>,
        object: Node<'a>,
        lower: Option<Node<'a>>,
        upper: Option<Node<'a>>,
        step: Option<Node<'a>>,
    ) -> Reg {
        let has_call = e.call_contract().is_some();
        let (obj, object_place) = if has_call {
            self.lower_method_receiver(e, object)
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let lower = lower.map(|bound| self.expr(bound));
        let upper = upper.map(|bound| self.expr(bound));
        let step = step.map(|bound| self.expr(bound));
        let descriptor = slice_descriptors(e.checked)
            .and_then(|descriptors| descriptors.first().cloned().flatten())
            .expect("checked slice has a selected descriptor");
        let argument = self.slice_literal_argument(e, &descriptor, [lower, upper, step]);
        // No intrinsic slice channel remains: StringLiteral
        // positional slicing was removed at the audited head, so
        // every checked slice routes through a nominal call.
        let call = self.subscript_call_contract(e, std::slice::from_ref(&argument));
        let d = self.fresh(e.location(), None);
        self.emit_interior_invalidations(e, None);
        self.emit_slice_read(d, obj, argument, object_place, call);
        d
    }

    /// The subscript argument slice literal `construction` of subscript
    /// `node` passes, its written operands (lower, upper, stride) already
    /// lowered to `operands`: the value its `__slice_literal__` constructor
    /// call returns, upstream's `T(lower, upper, stride,
    /// __slice_literal__=None)` with `None` for an omitted operand.
    pub(in crate::mir) fn slice_literal_argument(
        &mut self,
        node: Node<'a>,
        construction: &CheckedSliceLiteral,
        operands: [Option<Reg>; 3],
    ) -> MirSubscriptArg {
        let args = operands
            .into_iter()
            .zip(&construction.omitted)
            .map(|(written, omitted)| {
                written.unwrap_or_else(|| {
                    let none = self.none_register(node);
                    match omitted
                        .as_ref()
                        .and_then(|omitted| omitted.conversion.as_ref())
                    {
                        Some((constructor, ty)) => self.constructor_call(
                            node,
                            constructor,
                            ty.clone(),
                            vec![none],
                            Vec::new(),
                        ),
                        None => none,
                    }
                })
            })
            .collect();
        let marker = self.none_register(node);
        MirSubscriptArg::Index(self.constructor_call(
            node,
            &construction.constructor,
            construction.target.clone(),
            args,
            vec![("__slice_literal__".to_string(), marker)],
        ))
    }

    /// Emit the read of `object` subscripted by the one constructed slice
    /// value `argument` into `dest`, passed like any index.
    pub(in crate::mir) fn emit_slice_read(
        &mut self,
        dest: Reg,
        object: Reg,
        argument: MirSubscriptArg,
        object_place: Option<MirPlace>,
        call: Option<MirSubscriptCall>,
    ) {
        self.record_call(dest, call.as_ref().map(|call| call.target.clone()), 0);
        self.emit(MirInstr::MultiIndex {
            dest,
            object,
            args: vec![argument],
            object_place: object_place.map(Box::new),
            arg_places: vec![None],
            kwargs: Vec::new(),
            kwarg_places: Vec::new(),
            call: call.map(Box::new),
        });
    }

    /// Read multi-subscript node `e`, whose arguments are `args`.
    pub(super) fn multi_index_expr(
        &mut self,
        e: Node<'a>,
        object: Node<'a>,
        args: &[CheckedSubscriptArg],
    ) -> Reg {
        let operand = |id: CheckedNodeId| {
            e.operand_of(id)
                .expect("a subscript argument's node is its operand")
        };
        let bound = |this: &mut Self, id: Option<CheckedNodeId>| {
            id.map(|id| this.outside_call_setup(|this| this.expr(operand(id))))
        };
        // `ptr[unsafe_offset=i]` — the keyword spelling of pointer
        // indexed dereference — reads exactly like `ptr[i]`.
        if is_pointer(object)
            && let [CheckedSubscriptArg::Keyword { name, value }] = args
            && name == "unsafe_offset"
        {
            return self.pointer_subscript_read(e, object, operand(*value));
        }
        let has_call = e.call_contract().is_some();
        let (object_reg, object_place) = if has_call {
            self.lower_setup_receiver_with(e, object, |this| this.lower_method_receiver(e, object))
        } else {
            (self.expr(object), self.simple_place(object))
        };
        let descriptors =
            slice_descriptors(e.checked).expect("checked multi-subscript has descriptor metadata");
        let mut arg_places = Vec::with_capacity(args.len());
        let mut kwargs = Vec::new();
        let mut kwarg_places = Vec::new();
        let lowered_args: Vec<_> = args
            .iter()
            .zip(descriptors.iter().cloned())
            .filter_map(|(argument, descriptor)| match argument {
                CheckedSubscriptArg::Keyword { name, value } => {
                    debug_assert!(descriptor.is_none());
                    let (register, place) = self.lower_call_argument(operand(*value));
                    kwarg_places.push(place);
                    kwargs.push((name.clone(), MirSubscriptArg::Index(register)));
                    None
                }
                CheckedSubscriptArg::KeywordSlice {
                    name,
                    lower,
                    upper,
                    step,
                    ..
                } => {
                    kwarg_places.push(None);
                    let descriptor =
                        descriptor.expect("keyword slice argument has a bound literal");
                    let operands = [bound(self, *lower), bound(self, *upper), bound(self, *step)];
                    let argument = self.outside_call_setup(|this| {
                        this.slice_literal_argument(e, &descriptor, operands)
                    });
                    kwargs.push((name.clone(), argument));
                    None
                }
                CheckedSubscriptArg::Index(value) => {
                    debug_assert!(descriptor.is_none());
                    let (register, place) = self.lower_call_argument(operand(*value));
                    arg_places.push(place);
                    Some(MirSubscriptArg::Index(register))
                }
                CheckedSubscriptArg::Slice {
                    lower, upper, step, ..
                } => {
                    arg_places.push(None);
                    let descriptor = descriptor.expect("slice argument has a bound literal");
                    let operands = [bound(self, *lower), bound(self, *upper), bound(self, *step)];
                    Some(self.outside_call_setup(|this| {
                        this.slice_literal_argument(e, &descriptor, operands)
                    }))
                }
            })
            .collect();
        if has_call {
            self.close_call_setup(None);
        }
        let call = self.subscript_call_contract(e, &lowered_args);
        let dest = self.fresh(e.location(), None);
        for argument in args {
            if let CheckedSubscriptArg::Index(value) = argument {
                self.emit_interior_invalidations(operand(*value), None);
            }
        }
        self.emit_interior_invalidations(e, None);
        self.record_call(dest, call.as_ref().map(|call| call.target.clone()), 0);
        self.emit(MirInstr::MultiIndex {
            dest,
            object: object_reg,
            args: lowered_args,
            object_place: object_place.map(Box::new),
            arg_places,
            kwargs,
            kwarg_places,
            call: call.map(Box::new),
        });
        dest
    }

    /// Read `Variant` projection node `e`, `name[T]`.
    pub(super) fn variant_project_type_apply(&mut self, e: Node<'a>, name: &str) -> Reg {
        let place = self.resolved_place(name);
        self.variant_payload_read(e, place)
    }

    /// The checked value-copy boundary of place read `e`, whose borrowed
    /// value is `loaded`: a read selected for a consuming value context
    /// runs its `__copyinit__`; any other read stays the borrow.
    fn copy_boundary(&mut self, e: Node<'a>, loaded: Reg, root: Option<VarId>) -> Reg {
        let node = e.checked;
        if !copies_place_value(node) {
            return loaded;
        }
        let ty = self
            .f
            .reg_types
            .get(&loaded.0)
            .cloned()
            .or_else(|| node.ty.clone())
            .unwrap_or_else(Ty::error);
        let copied = self.fresh_typed(e.location(), root, ty);
        self.emit(MirInstr::CopyValue {
            dest: copied,
            value: loaded,
            constructor: None,
        });
        copied
    }

    /// Read pointer subscript node `e`, `object[index]` or
    /// `object[unsafe_offset=index]`. Upstream's `__getitem__` returns
    /// `ref T`, so the subscript is a place: the read borrows the pointee,
    /// and only the checked value-copy boundary copies it.
    fn pointer_subscript_read(&mut self, e: Node<'a>, object: Node<'a>, index: Node<'a>) -> Reg {
        let (read, root) = self.pointee_read(e, object, index);
        self.copy_boundary(e, read, root)
    }

    /// The borrowed read of the pointee pointer subscript node `e` names.
    /// Dereferencing an origin-bearing pointer reads its address; the
    /// checker fixed the offset to 0. A stably bound pointer with a
    /// statically known address substitutes that place, keeping it touched (and so
    /// droppable) at each access; a pointer field of a call result the
    /// checker materialized (`make(b).src[]`) dereferences through the
    /// temporary's hidden slot; otherwise the access reads through the
    /// runtime handle, or the pointer-intrinsic `Index` read.
    fn pointee_read(
        &mut self,
        e: Node<'a>,
        object: Node<'a>,
        index: Node<'a>,
    ) -> (Reg, Option<VarId>) {
        if let Some(place) = self.pointer_deref_place(object) {
            return self.pointee_place_read(e, place);
        }
        if let Some(d) = self.temporary_pointer_deref(e, object) {
            return (d, None);
        }
        if object.rooted_at_materialized_temporary()
            && let Some(place) = self.lower_projected_reference_place(e)
        {
            return self.pointee_place_read(e, place);
        }
        if object.is_origin_bearing_pointer() {
            let reference = self.expr(object);
            let d = self.fresh(e.location(), None);
            self.emit(MirInstr::ReadRef { dest: d, reference });
            return (d, None);
        }
        let base = self.expr(object);
        let base_place = self.simple_place(object);
        let (idx, index_place) = self.lower_call_argument(index);
        let d = self.fresh(e.location(), None);
        self.emit_interior_invalidations(index, None);
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::Index {
            dest: d,
            base,
            index: idx,
            base_place: base_place.map(Box::new),
            index_place: index_place.map(Box::new),
            call: None,
            intrinsic: Some(MirIntrinsicSubscript::Pointer),
        });
        (d, None)
    }

    /// Load the pointee at `place`, which pointer subscript node `e` reads.
    fn pointee_place_read(&mut self, e: Node<'a>, place: MirPlace) -> (Reg, Option<VarId>) {
        let root = place.root;
        let d = self.fresh(e.location(), Some(root));
        self.emit(MirInstr::LoadPlace { dest: d, place });
        (d, Some(root))
    }

    /// A value read of a homogeneous collector's element (`a[i]` over
    /// `*a: T`) loads the element place, as a field read does, and runs the
    /// checked value-copy boundary on that element alone; reading the
    /// collector as a value first would copy its whole storage.
    fn storage_element_read(&mut self, e: Node<'a>, object: Node<'a>) -> Option<Reg> {
        let node = e.checked;
        if !matches!(
            object.intrinsic_index_dispatch(),
            Some(MirIntrinsicSubscript::VariadicStorage)
        ) {
            return None;
        }
        let place = self.try_place(e)?;
        let root = place.root;
        let loaded = self.fresh_typed(
            e.location(),
            Some(root),
            place
                .ty
                .clone()
                .or_else(|| node.ty.clone())
                .unwrap_or_else(Ty::error),
        );
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place,
        });
        Some(self.copy_boundary(e, loaded, Some(root)))
    }

    /// Load the payload `Variant` projection node `node` selects out of the
    /// variant at `place`.
    fn variant_payload_read(&mut self, e: Node<'a>, mut place: MirPlace) -> Reg {
        let node = e.checked;
        let (index, alternatives) =
            variant_project(node).expect("checked Variant projection carries a tag");
        if place.root_ty.is_none() {
            place.root_ty = Some(TyKind::Variant(alternatives.to_vec()).into());
        }
        let ty = e
            .place_ty()
            .or_else(|| node.ty.clone())
            .expect("checked Variant projection has a payload type");
        place.project(Proj::Variant(index), ty);
        let root = place.root;
        let dest = self.fresh(e.location(), Some(root));
        self.emit(MirInstr::LoadPlace { dest, place });
        // The checked value-copy boundary: a Copyable payload runs its
        // `__copyinit__` out of the variant's storage instead of
        // aliasing it past the owner's lifetime.
        if copies_place_value(node) {
            let copied = self.fresh_typed(
                e.location(),
                Some(root),
                node.ty.clone().unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: dest,
                constructor: None,
            });
            // Keep the owning variant alive through the copy: the
            // loaded register aliases its storage until `__copyinit__`
            // has produced the independent value.
            self.emit(MirInstr::KeepAlive { var: root });
            return copied;
        }
        dest
    }

    /// A fresh register holding `None`, at `node`.
    fn none_register(&mut self, node: Node<'a>) -> Reg {
        let none = self.fresh_typed(node.location(), None, Ty::none());
        self.emit(MirInstr::Const {
            dest: none,
            k: Const::None,
        });
        none
    }
}

/// Whether `object` is a pointer, whose subscript reads its pointee.
fn is_pointer(object: Node<'_>) -> bool {
    matches!(
        object.ty().as_ref().map(Ty::kind),
        Some(TyKind::Pointer { .. })
    )
}

/// Whether `node` carries the checked value-copy boundary.
fn copies_place_value(node: &CheckedExpr) -> bool {
    node.adjustments
        .iter()
        .any(|adjustment| matches!(adjustment, SemanticAdjustment::CopyPlaceValue))
}

/// Whether the checker bound `node` to a read parameter, read where it lies.
fn reads_argument(node: &CheckedExpr) -> bool {
    node.adjustments
        .iter()
        .any(|adjustment| matches!(adjustment, SemanticAdjustment::BorrowReadArgument))
}

/// The alternative index and alternatives of the `Variant` projection
/// `node` records, if any.
fn variant_project(node: &CheckedExpr) -> Option<(mojito_types::types::VariantIndex, &[Ty])> {
    node.adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::VariantProject {
                index,
                alternatives,
            } => Some((index.clone(), alternatives.as_slice())),
            _ => None,
        })
}

/// The slice literals multi-subscript or slice node `node` records.
fn slice_descriptors(node: &CheckedExpr) -> Option<&[Option<CheckedSliceLiteral>]> {
    node.adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                Some(descriptors.as_slice())
            }
            _ => None,
        })
}
