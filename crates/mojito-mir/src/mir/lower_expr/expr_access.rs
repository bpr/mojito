//! Member, subscript, slice, and variant-projection arms of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::call_results::{CValue, OperandBinding};
use mojito_checked::checked::{
    CheckedCallArgumentSource, CheckedExpr, CheckedNodeId, CheckedSliceLiteral,
    CheckedSubscriptArg, SemanticAdjustment,
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
    /// folds per instance. A literal value converts to the type its use
    /// takes, as upstream's implicit literal conversion does.
    pub(in crate::mir) fn param_value_register(
        &mut self,
        e: Node<'a>,
        value: mojito_types::param_expr::ParamExpr,
    ) -> Reg {
        let value = self.param_value_register_at(e.location(), e.ty(), value);
        let literal = self
            .f
            .reg_types
            .get(&value.0)
            .is_some_and(|ty| ty.kind().is_numeric_literal());
        match e.param_literal_target().filter(|_| literal) {
            Some(target) => self.materialize_literal(value, &target, e.location()),
            None => value,
        }
    }

    /// [`Self::param_value_register`] at `location`, typed `ty` when the
    /// value has a checked type. A literal a typed binding denotes
    /// (`comptime t: UInt32 = 0x2`) is the value of that type, as
    /// upstream's alias is its declared type's attribute.
    pub(in crate::mir) fn param_value_register_at(
        &mut self,
        location: SourceSpan,
        ty: Option<Ty>,
        value: mojito_types::param_expr::ParamExpr,
    ) -> Reg {
        let literal = self.param_constant_register(location.clone(), ty.clone(), value);
        match ty {
            Some(ty) => self.materialize_register(literal, &ty, location),
            None => literal,
        }
    }

    fn param_constant_register(
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
            // A string at the nominal `String` is a `String` value, which
            // the elaborator materializes as it does a `String` parameter.
            mojito_types::param_expr::ParamKind::Constant(mojito_types::ct::CtValue::Str(s))
                if !value
                    .meta()
                    .as_value()
                    .is_some_and(mojito_types::types::is_nominal_string) =>
            {
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

    /// `value`, the lowered `e`, as a value: a direct `ref` field's load,
    /// which [`Self::member_expr`] leaves holding its handle, reads
    /// through to the referent, as a value use of a `ref` binding does.
    pub(in crate::mir) fn field_referent(&mut self, e: Node<'a>, value: Reg) -> Reg {
        let loads_field = matches!(
            self.f.body.instrs.last(),
            Some(MirInstr::LoadPlace { dest, .. }) if *dest == value
        );
        if loads_field
            && !matches!(e.ty().as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
            && let Some(TyKind::Ref(reference)) = self.f.reg_types.get(&value.0).map(Ty::kind)
        {
            let referent = reference.referent.clone();
            self.f.reg_types.insert(value.0, referent);
        }
        value
    }

    /// Read member node `e`, `object.field`.
    pub(super) fn member_expr(&mut self, e: Node<'a>, object: Node<'a>, field: &str) -> Reg {
        let node = e.checked;
        if e.is_pointer_mlir_value_read() {
            return self.pointer_convert(e, object);
        }
        // `v.length` on a SIMD value is its `SIMDLength` parameter, after
        // a value receiver (`f().length`) runs.
        if let Some(length) = e.simd_length() {
            if !object.is_name() && object.ty().is_some() {
                self.expr(object);
            }
            return self.param_value_register(e, length);
        }
        // `v.dtype` on a SIMD value (or `DType.<name>`) is the lane slot: a
        // known one is a constant, and one a generator names is the
        // evaluation of its parameter expression.
        let folded = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                SemanticAdjustment::DtypeConstant { dtype } => Some(match dtype.clone() {
                    mojito_types::types::SimdDtype::Known(dtype) => {
                        Ok(Const::Value(mojito_types::types::dtype_value(dtype)))
                    }
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
                    if let Some(value) = self.dtype_slot_register(&expr, &e.location()) {
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
        // or of a reference result projects that storage below; only a base
        // no storage holds keeps the register-based `GetField`.
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
            // Pointer.
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
            } else if !matches!(node.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                && let Some(place) = self.lower_projected_reference_place(e)
            {
                // A field of a reference result, or of a materialized
                // temporary, is a projection of that reference, as
                // upstream's `!lit.ref` field of one is: the load and any
                // copy of it read from the place.
                let ty = place
                    .ty
                    .clone()
                    .or_else(|| node.ty.clone())
                    .unwrap_or_else(Ty::error);
                let loaded = self.fresh_typed(e.location(), Some(place.root), ty);
                self.emit(MirInstr::LoadPlace {
                    dest: loaded,
                    place,
                });
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

    /// Read type-keyed subscript node `e`, `object[T]`: the accessor call
    /// `contract` selected (`v.__getitem_param__[T]()`) on `object`, with no
    /// run-time arguments; the receiver is passed as the accessor binds it
    /// ([`Self::lower_receiver_value`]).
    pub(super) fn type_keyed_accessor_read(
        &mut self,
        e: Node<'a>,
        object: Node<'a>,
        contract: mojito_checked::checked::CheckedCallContract,
    ) -> Reg {
        let (recv, _) = self.lower_receiver_value(e, object, false);
        let dest = self.fresh(e.location(), None);
        self.emit_interior_invalidations(object, None);
        let raises = self.call_raises(e, e.raises());
        self.emit(MirInstr::MethodCall {
            conventions: crate::mir::MirArgConventions::default(),
            dest,
            recv,
            method: "__getitem_param__".to_string(),
            resolved: Some(contract.target),
            raises,
            reference_result: contract.reference_result.map(Box::new),
            result_adapter: contract.result_adapter.map(MirResultAdapter::new),
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_writes: e.receiver_writes(),
            receiver_convention: e.receiver_convention(),
            capture_accesses: self.call_capture_accesses(e),
            param_arg_regs: Vec::new(),
            param_decls: contract.param_decls,
            instantiated_args: e.instantiated_args(),
            spread: None,
            refined_bound: contract.refined_bound.map(MirRefinedBound::boxed),
        });
        dest
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
            && !object
                .ty()
                .as_ref()
                .is_some_and(|ty| ty.kind().is_non_simd_struct())
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
        let has_call = e.call_contract().is_some();
        if !has_call && let Some(d) = self.storage_element_read(e, object) {
            return d;
        }
        let (base, _) = if has_call {
            self.lower_setup_receiver_with(e, object, |this| {
                this.lower_receiver_value(e, object, false)
            })
        } else {
            (self.expr(object), self.simple_place(object))
        };
        self.bind_call_operands(e, 0, &[index], &[]);
        let (mut idx, _) = self.lower_call_argument(index);
        if has_call {
            self.close_call_setup(None);
        } else {
            idx = self.materialize_register(idx, &Ty::int(), index.location());
        }
        let call = self
            .subscript_call_contract(e, &[(Some(index.location()), MirSubscriptArg::Index(idx))]);
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
        let obj = if has_call {
            self.lower_receiver_value(e, object, false).0
        } else {
            self.expr(object)
        };
        let lower = lower.map(|bound| self.expr(bound));
        let upper = upper.map(|bound| self.expr(bound));
        let step = step.map(|bound| self.expr(bound));
        let descriptor = slice_descriptors(e.checked)
            .and_then(|descriptors| descriptors.first().cloned().flatten())
            .expect("checked slice has a selected descriptor");
        let sliced = self.slice_literal_argument(e, &descriptor, [lower, upper, step]);
        let (argument, _) =
            self.address_subscript_argument(e, CheckedCallArgumentSource::Positional(0), sliced);
        // No intrinsic slice channel remains: StringLiteral
        // positional slicing was removed at the audited head, so
        // every checked slice routes through a nominal call.
        let call = self.subscript_call_contract(e, &[(None, argument.clone())]);
        let d = self.fresh(e.location(), None);
        self.emit_interior_invalidations(e, None);
        self.emit_slice_read(d, obj, argument, call);
        d
    }

    /// The subscript argument slice literal `construction` of subscript
    /// `node` passes, its written operands (lower, upper, stride) already
    /// lowered to `operands`: the value its `__slice_literal__` constructor
    /// call returns, upstream's `T(lower, upper, stride,
    /// __slice_literal__=None)` with `None` for an omitted operand. A
    /// value returned through memory lives in its call-result slot, which
    /// the subscript reads in place: the slot's place comes with it.
    pub(in crate::mir) fn slice_literal_argument(
        &mut self,
        node: Node<'a>,
        construction: &CheckedSliceLiteral,
        operands: [Option<Reg>; 3],
    ) -> (MirSubscriptArg, Option<MirPlace>) {
        let args = operands
            .into_iter()
            .zip(&construction.omitted)
            .map(|(written, omitted)| {
                written.unwrap_or_else(|| {
                    self.passed_none(node, omitted.as_ref().and_then(|o| o.conversion.as_ref()))
                })
            })
            .collect();
        let marker = self.passed_none(node, construction.marker.conversion.as_ref());
        let value = self.constructor_call(
            node,
            &construction.constructor,
            construction.target.clone(),
            args,
            vec![("__slice_literal__".to_string(), marker)],
        );
        if !construction.indirect_result {
            return (MirSubscriptArg::Index(value), None);
        }
        let slot = self.call_result_slot(value, construction.target.clone(), true);
        let (read, place) = self.read_c(node, CValue::Slot(slot));
        (MirSubscriptArg::Index(read), place)
    }

    /// Slice literal argument `argument` of subscript `subscript`, its
    /// accessor's argument `source`, as the accessor binds it: the address
    /// of the slot it is read from when the check judged the parameter by
    /// address, as upstream passes the temporary's `!lit.ref`.
    pub(in crate::mir) fn address_subscript_argument(
        &mut self,
        subscript: Node<'a>,
        source: CheckedCallArgumentSource,
        (argument, place): (MirSubscriptArg, Option<MirPlace>),
    ) -> (MirSubscriptArg, Option<MirPlace>) {
        let MirSubscriptArg::Index(register) = argument;
        if subscript.argument_binding(source) != Some(OperandBinding::Read { by_address: true }) {
            return (argument, place);
        }
        let (register, place) = self.address_operand(
            subscript,
            (register, place),
            mojito_types::origin::Mutability::Immutable,
        );
        (MirSubscriptArg::Index(register), place)
    }

    /// A synthesized `None` argument, built into its parameter's type by
    /// `conversion` when one applies.
    fn passed_none(&mut self, node: Node<'a>, conversion: Option<&(String, Ty)>) -> Reg {
        let none = self.none_register(node);
        match conversion {
            Some((constructor, ty)) => {
                self.constructor_call(node, constructor, ty.clone(), vec![none], Vec::new())
            }
            None => none,
        }
    }

    /// Emit the read of `object` subscripted by the one constructed slice
    /// value `argument` into `dest`, passed like any index.
    pub(in crate::mir) fn emit_slice_read(
        &mut self,
        dest: Reg,
        object: Reg,
        argument: MirSubscriptArg,
        call: Option<MirSubscriptCall>,
    ) {
        self.record_call(dest, call.as_ref().map(|call| call.target.clone()), 0);
        self.emit(MirInstr::MultiIndex {
            dest,
            object,
            args: vec![argument],
            kwargs: Vec::new(),
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
        let object_reg = if has_call {
            self.lower_setup_receiver_with(e, object, |this| {
                this.lower_receiver_value(e, object, false)
            })
            .0
        } else {
            self.expr(object)
        };
        let descriptors =
            slice_descriptors(e.checked).expect("checked multi-subscript has descriptor metadata");
        self.bind_subscript_operands(e, args, operand);
        let mut positional = 0;
        let mut kwargs = Vec::new();
        let mut evaluated = Vec::new();
        let lowered_args: Vec<_> = args
            .iter()
            .zip(descriptors.iter().cloned())
            .filter_map(|(argument, descriptor)| match argument {
                CheckedSubscriptArg::Keyword { name, value } => {
                    debug_assert!(descriptor.is_none());
                    let (register, _) = self.lower_call_argument(operand(*value));
                    evaluated.push((
                        Some(operand(*value).location()),
                        MirSubscriptArg::Index(register),
                    ));
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
                    let descriptor =
                        descriptor.expect("keyword slice argument has a bound literal");
                    let operands = [bound(self, *lower), bound(self, *upper), bound(self, *step)];
                    let sliced = self.outside_call_setup(|this| {
                        this.slice_literal_argument(e, &descriptor, operands)
                    });
                    let source = CheckedCallArgumentSource::Keyword(kwargs.len());
                    let (argument, _) = self.address_subscript_argument(e, source, sliced);
                    kwargs.push((name.clone(), argument));
                    None
                }
                CheckedSubscriptArg::Index(value) => {
                    debug_assert!(descriptor.is_none());
                    let (register, _) = self.lower_call_argument(operand(*value));
                    positional += 1;
                    evaluated.push((
                        Some(operand(*value).location()),
                        MirSubscriptArg::Index(register),
                    ));
                    Some(MirSubscriptArg::Index(register))
                }
                CheckedSubscriptArg::Slice {
                    lower, upper, step, ..
                } => {
                    let descriptor = descriptor.expect("slice argument has a bound literal");
                    let operands = [bound(self, *lower), bound(self, *upper), bound(self, *step)];
                    let sliced = self.outside_call_setup(|this| {
                        this.slice_literal_argument(e, &descriptor, operands)
                    });
                    let source = CheckedCallArgumentSource::Positional(positional);
                    let (argument, _) = self.address_subscript_argument(e, source, sliced);
                    positional += 1;
                    Some(argument)
                }
            })
            .collect();
        if has_call {
            self.close_call_setup(None);
        }
        let call = self.subscript_call_contract(e, &evaluated);
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
            kwargs,
            call: call.map(Box::new),
        });
        dest
    }

    /// Rewrite `read`, the pointer subscript node `e` just read by value,
    /// to the shape upstream's `__getitem__` has: the pointee's reference
    /// (`lit.ref.from_pointer`, MIR's `pointer.convert`) of the offset
    /// pointer, and a `ref.read` through it, so a copy of the read takes
    /// that reference. `false`, leaving the read alone, unless the last
    /// instruction is the read through a pointer: a pointee read off a
    /// place already passes its `ref.make`.
    pub(super) fn pointee_reference_read(&mut self, e: Node<'a>, read: Reg) -> bool {
        let (pointer, offset) = match self.f.body.instrs.last() {
            Some(MirInstr::Index {
                dest,
                base,
                index,
                call: None,
                intrinsic: Some(MirIntrinsicSubscript::Pointer),
            }) if *dest == read => (*base, Some(*index)),
            Some(MirInstr::ReadRef { dest, reference }) if *dest == read => (*reference, None),
            _ => return false,
        };
        let Some(pointer_ty) = self
            .f
            .reg_types
            .get(&pointer.0)
            .filter(|ty| ty.is_pointer())
            .cloned()
        else {
            return false;
        };
        let Some(referent) = pointer_ty.kind().pointer_element().cloned() else {
            return false;
        };
        let address_space = pointer_ty
            .kind()
            .pointer_address_space()
            .cloned()
            .unwrap_or(mojito_types::types::AddressSpaceSlot::GENERIC);
        let origin = pointer_ty
            .kind()
            .pointer_origin()
            .and_then(|origin| origin.as_origin())
            .unwrap_or(mojito_types::origin::Origin::Untracked { mutable: false });
        self.f.body.instrs.pop();
        let pointer = match offset {
            Some(offset) => {
                let offset_pointer = self.fresh_typed(e.location(), None, pointer_ty);
                self.emit(MirInstr::BinOp {
                    op: mojito_ast::ast::InfixOp::Add,
                    dest: offset_pointer,
                    a: pointer,
                    b: offset,
                    resolved: None,
                    raises: None,
                    refined_bound: None,
                    fastmath: None,
                });
                offset_pointer
            }
            None => pointer,
        };
        let reference = self.fresh_typed(
            e.location(),
            None,
            TyKind::Ref(mojito_types::origin::RefTy {
                referent,
                origin,
                mutability: mojito_types::origin::Mutability::Immutable,
                address_space,
            })
            .into(),
        );
        self.emit(MirInstr::PointerConvert {
            dest: reference,
            value: pointer,
        });
        self.emit(MirInstr::ReadRef {
            dest: read,
            reference,
        });
        true
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
    /// and only the checked value-copy boundary copies it, through the
    /// pointee's reference ([`Self::pointee_reference_read`]).
    fn pointer_subscript_read(&mut self, e: Node<'a>, object: Node<'a>, index: Node<'a>) -> Reg {
        let (read, root) = self.pointee_read(e, object, index);
        if copies_place_value(e.checked) {
            self.pointee_reference_read(e, read);
        }
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
        // The pointer is read by value: the subscript accesses its
        // pointee, not the storage holding it.
        let base = self.expr(object);
        let (idx, _) = self.lower_call_argument(index);
        let idx = self.materialize_register(idx, &Ty::int(), index.location());
        let d = self.fresh(e.location(), None);
        self.emit_interior_invalidations(index, None);
        self.emit_interior_invalidations(e, None);
        self.emit(MirInstr::Index {
            dest: d,
            base,
            index: idx,
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
    object.ty().as_ref().is_some_and(Ty::is_pointer)
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
