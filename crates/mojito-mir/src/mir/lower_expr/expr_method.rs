//! The method-call arm of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_ast::ast::KwArg;
impl Flatten<'_> {
    pub(super) fn method_call_expr(
        &mut self,
        e: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        // An instance method called through its type
        // (`List[Int].__len__(xs)`): the checker typed the first
        // argument as the receiver, so lower exactly that call.
        let receiver_argument = self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ReceiverFromFirstArgument { .. }
            )
        });
        let (object, args): (&Expr, &[Expr]) = if receiver_argument {
            let Some((receiver, rest)) = args.split_first() else {
                unreachable!("checked type-receiver call carries a receiver argument")
            };
            (receiver, rest)
        } else {
            (object, args)
        };
        if let Some(dest) = self.method_call_value_form(e, object, method, args, kwargs) {
            return dest;
        }
        if let Some(dest) = self.method_call_storage_form(e, object, method, args, kwargs) {
            return dest;
        }
        // A **static** method — the receiver is a type, not a value
        // (`Point.origin()`, `Dict[Int, Int].fromkeys(...)`, the
        // pointer family's `UnsafePointer[T].alloc(n)`). Lower to a
        // call on the checker-selected symbol (overloaded statics
        // carry their exact spelling). The receiver's compile-time
        // arguments are already resolved into that selection by the
        // checker and erase here — a static's frame declares only the
        // method's own parameters, so struct arguments must not
        // occupy its `param_arg_regs` slots.
        if let Some(type_name) = self.type_receiver_name(object) {
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = self.call_anchors_arguments(e);
            let (regs, arg_places) = self.lower_call_arguments(args, false);
            self.allow_argument_anchors = saved_anchor_permission;
            let (kw, kwarg_places) = self.lower_call_keywords(kwargs, false);
            let d = self.fresh(span(e), None);
            let target = self
                .resolved_callable(e)
                .unwrap_or_else(|| format!("{type_name}.{method}"));
            self.emit_call_invalidations(e, args, kwargs);
            let receiver = self.static_receiver(object);
            self.emit(MirInstr::Call {
                dest: d,
                func: FuncRef::named(&target),
                raises: self.checked_raises(e),
                args: regs,
                kwargs: kw,
                arg_places,
                kwarg_places,
                capture_accesses: self.checked_call_capture_accesses(e),
                param_arg_regs: Vec::new(),
                receiver,
                instantiated_args: Vec::new(),
                spread: None,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return d;
        }
        if let Some(dest) = self.lower_elided_receiver_call(e, object, &[], args, kwargs) {
            return dest;
        }
        self.ordinary_method_call(e, object, method, args, kwargs)
    }

    /// The struct a static call's receiver names, when the receiver is a
    /// type rather than a value: a type application (`Dict[Int, Int]`), a
    /// bare name that is no local (`Point`), or the single-argument
    /// application that parses as a value subscript over such a name
    /// (`Box[String]`). The bracket content is a compile-time argument the
    /// checker already resolved into its symbol selection — never lowered.
    pub(super) fn type_receiver_name<'e>(&self, object: &'e Expr) -> Option<&'e str> {
        // A value binder of the enclosing declaration (`dt.is_integral()` in
        // a `DType`-keyed body) is a value, read through its slot.
        let not_a_local = |name: &'e String| {
            (!self.vars.iter().any(|local| local == name)
                && !self
                    .enclosing_binders
                    .values
                    .iter()
                    .any(|(binder, _)| binder.name.as_ref() == name.as_str()))
            .then_some(name.as_str())
        };
        match &object.kind {
            ExprKind::TypeApply { name, .. } => Some(name.as_str()),
            ExprKind::Identifier(name) => not_a_local(name),
            ExprKind::Index { object: base, .. } => match &base.kind {
                ExprKind::Identifier(name) => not_a_local(name),
                _ => None,
            },
            _ => None,
        }
    }

    /// The instance a static call's spelled receiver names (`Pair[Int]`),
    /// which the checker recorded as the receiver expression's type. Inside
    /// a static method of a value-parameterized struct, the receiver may
    /// spell that struct's binders (`W[Self.k]`), which the VM reads off the
    /// method's receiver-less `self` slot, so the slot is interned.
    pub(super) fn static_receiver(&mut self, object: &Expr) -> Option<Ty> {
        let receiver = self
            .checked_ty(object)
            .filter(|ty| matches!(ty, Ty::Struct(_, arguments) if !arguments.is_empty()))?;
        self.intern_static_self();
        Some(receiver)
    }

    /// Method calls the checker resolved to a value operation with no callee
    /// body: SIMD gathers and reinterprets, identity conversions, the
    /// `write_to`/`write_repr_to` swaps, and the unparameterized Variant
    /// operations. `None` leaves the call to the later forms.
    fn method_call_value_form(
        &mut self,
        e: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Option<Reg> {
        // `v.join(w)`: the checker resolved a two-operand lane gather.
        let simd_join = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask, joined: true } => {
                    Some(mask)
                }
                _ => None,
            });
        if let Some(mask) = simd_join
            && let [argument] = args
        {
            let value = self.expr(object);
            let other = Some(self.expr(argument));
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param }) =
            self.checked_adjustments(e).into_iter().find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { .. }
                )
            })
        {
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::ConstructTypeParam { dest, param });
            return Some(dest);
        }
        // `x.__mlir_index__()` on an integer is the identity
        // conversion to the index representation (`Int`).
        if method == "__mlir_index__"
            && args.is_empty()
            && kwargs.is_empty()
            && self.checked_ty(object).is_some_and(|ty| {
                matches!(
                    ty,
                    mojito_types::types::Ty::Int
                        | mojito_types::types::Ty::IntLiteral
                        | mojito_types::types::Ty::UInt
                )
            })
        {
            return Some(self.expr(object));
        }
        // `x.copy()` on a built-in copyable value has no callee: the
        // checker resolved it to the value read itself.
        if method == "copy"
            && args.is_empty()
            && kwargs.is_empty()
            && self
                .checked_ty(object)
                .is_some_and(|ty| mojito_types::types::builtin_copy_is_value_read(&ty))
        {
            return Some(self.expr(object));
        }
        // `x.write_to(writer)` on a receiver without a `write_to`
        // body of its own (checker-marked): receiver and argument
        // swap into the `writer.write(x)` shape, the writer's place
        // retained for the accumulator write-back.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::InvertedWrite
            )
        }) {
            let writer = args.first().expect("checked write_to has one writer");
            let (recv, recv_place) = self.lower_call_receiver(writer);
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = self.call_anchors_arguments(e);
            let (regs, arg_places) = self.lower_call_arguments(std::slice::from_ref(object), false);
            self.allow_argument_anchors = saved_anchor_permission;
            let d = self.fresh_typed(span(e), None, Ty::None);
            self.emit_interior_invalidations(writer, None);
            self.emit(MirInstr::MethodCall {
                dest: d,
                recv,
                method: "write".to_string(),
                resolved: None,
                raises: None,
                reference_result: None,
                result_adapter: None,
                args: regs,
                kwargs: Vec::new(),
                recv_place,
                recv_writes: true,
                arg_places,
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            });
            return Some(d);
        }
        // `write_repr_to` on a bounded or intrinsic Writable has no
        // concrete receiver body. Produce `repr(receiver)` once and
        // feed the text through the supplied Writer's ordinary
        // `write` path.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::InvertedReprWrite
            )
        }) {
            let writer = args.first().expect("checked write_repr_to has one writer");
            let value = self.expr(object);
            let repr = self.fresh_typed(span(object), None, Ty::StringLiteral);
            let value_place = self.simple_place(object);
            self.emit(MirInstr::Call {
                dest: repr,
                func: FuncRef::named("repr"),
                raises: None,
                args: vec![value],
                kwargs: Vec::new(),
                arg_places: vec![value_place],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
            });
            let (recv, recv_place) = self.lower_call_receiver(writer);
            let d = self.fresh_typed(span(e), None, Ty::None);
            self.emit_interior_invalidations(writer, None);
            self.emit(MirInstr::MethodCall {
                dest: d,
                recv,
                method: "write".to_string(),
                resolved: None,
                raises: None,
                reference_result: None,
                result_adapter: None,
                args: vec![repr],
                kwargs: Vec::new(),
                recv_place,
                recv_writes: true,
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            });
            return Some(d);
        }
        // `v.to_bits()` with the default target arrives as an ordinary
        // method call; the parameterized spelling takes the invoke
        // path above. Both lower to the same lane-wise reinterpret.
        if method == "to_bits"
            && args.is_empty()
            && kwargs.is_empty()
            && let Some((dtype, width)) =
                self.checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::SimdToBits {
                            dtype,
                            width,
                        } => Some((dtype, width)),
                        _ => None,
                    })
        {
            let value = self.expr(object);
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::SimdBitcast {
                dest,
                value,
                dtype,
                width,
            });
            return Some(dest);
        }
        // `v.set(init_with=…)` infers its alternative from the factory
        // (no explicit type parameter), so it arrives as an ordinary
        // method call rather than a parameterized invoke.
        if let Some(index) =
            self.checked_adjustments(e)
                .iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::VariantSetInitWith {
                        index,
                        ..
                    } => Some(*index),
                    _ => None,
                })
        {
            let place = self
                .try_place(object)
                .expect("checked Variant.set receiver is a writable place");
            let factory = self.expr(
                &kwargs
                    .first()
                    .expect("checked Variant.set(init_with=) has one factory")
                    .value,
            );
            let dest = self.fresh(span(e), None);
            self.emit_interior_invalidations(e, None);
            self.emit(MirInstr::VariantSetInitWith {
                dest,
                place,
                index,
                factory,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return Some(dest);
        }
        // The parameterless Variant owning operation
        // (`v^.deinit_with(handler)`) is spelled as an ordinary method
        // call rather than a parameterized invoke.
        if let Some(index) =
            self.checked_adjustments(e)
                .iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::VariantDeinitWith {
                        index,
                        ..
                    } => Some(*index),
                    _ => None,
                })
        {
            let place = self
                .try_place(object)
                .expect("checked Variant.deinit_with receiver is an owned place");
            let variant = self.fresh(span(object), None);
            self.emit(MirInstr::MovePlace {
                dest: variant,
                place,
            });
            let handler = self.expr(
                args.first()
                    .expect("checked Variant.deinit_with has one handler"),
            );
            let dest = self.fresh(span(e), None);
            self.emit_interior_invalidations(e, None);
            self.emit(MirInstr::VariantDeinitWith {
                dest,
                variant,
                handler,
                index,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return Some(dest);
        }
        None
    }

    /// Method calls over a callable field or the pointer and inline
    /// uninit-storage intrinsics. `None` leaves the call to the later forms.
    fn method_call_storage_form(
        &mut self,
        e: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Option<Reg> {
        // A callable-typed FIELD invocation (`holder.callback(1)`)
        // loads the stored value and calls indirectly; the callee
        // place is the field's, so a closure environment stays
        // reachable through stable storage.
        if let Some(mojito_checked::checked::SemanticAdjustment::FieldInvocation { callable }) =
            self.checked_adjustments(e).into_iter().find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::FieldInvocation { .. }
                )
            })
        {
            let (recv, recv_place) = self.lower_call_receiver(object);
            let callee = self.fresh_typed(span(e), None, callable.clone());
            self.emit(MirInstr::GetField {
                dest: callee,
                base: recv,
                field: method.to_owned(),
            });
            let callee_place = recv_place.map(|mut place| {
                place.project(Proj::Field(method.to_owned()), callable.clone());
                place
            });
            let resolved = self.resolved_callable(e);
            let raises = self.checked_raises(e);
            return Some(self.emit_indirect_invocation(
                e,
                callee,
                callee_place,
                Some(&callable),
                resolved,
                raises,
                Vec::new(),
                args,
                kwargs,
                true,
            ));
        }
        let pointer_storage = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::PointerStorageTake { element } => {
                    Some((true, element))
                }
                mojito_checked::checked::SemanticAdjustment::PointerStorageDestroy { element } => {
                    Some((false, element))
                }
                _ => None,
            });
        if let Some((take, element)) = pointer_storage {
            let pointer = self.expr(object);
            // Compiler-private `take(i)`/`destroy(i)` pass the slot
            // index; the public zero-argument pointee operations
            // (`unsafe_take_pointee`/`unsafe_deinit_pointee`) fix it
            // to the dereference offset 0.
            let index = match args.first() {
                Some(index) => self.expr(index),
                None => self.constant(e, Const::Int(0)),
            };
            debug_assert!(kwargs.is_empty());
            let dest = self.fresh(span(e), None);
            self.emit(if take {
                MirInstr::PointerStorageTake {
                    dest,
                    pointer,
                    index,
                    element,
                }
            } else {
                MirInstr::PointerStorageDestroy {
                    dest,
                    pointer,
                    index,
                    element,
                }
            });
            return Some(dest);
        }
        // Compiler-private inline uninit storage (`MaybeUninit`'s
        // field). `unsafe_write` stores through the payload projection
        // — the place is opaque to drop elaboration, so a previously
        // written payload is overwritten raw (it leaks by design).
        // `take`/`destroy` consume the transferred storage value.
        let uninit_storage = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::UninitStorageWrite { element } => {
                    Some((UninitStorageOp::Write, element))
                }
                mojito_checked::checked::SemanticAdjustment::UninitStorageTake { element } => {
                    Some((UninitStorageOp::Take, element))
                }
                mojito_checked::checked::SemanticAdjustment::UninitStorageDestroy { element } => {
                    Some((UninitStorageOp::Destroy, element))
                }
                _ => None,
            });
        if let Some((op, element)) = uninit_storage {
            debug_assert!(kwargs.is_empty());
            match op {
                UninitStorageOp::Write => {
                    let src = self.expr(args.first().expect("checked unsafe_write has one value"));
                    let mut place = self.place(object);
                    place.project(Proj::UninitPayload, element);
                    self.emit(MirInstr::Store { place, src });
                    let dest = self.fresh_typed(span(e), None, Ty::None);
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    return Some(dest);
                }
                UninitStorageOp::Take | UninitStorageOp::Destroy => {
                    let storage = self.expr(object);
                    let dest = self.fresh(span(e), None);
                    self.emit(if matches!(op, UninitStorageOp::Take) {
                        MirInstr::UninitStorageTake {
                            dest,
                            storage,
                            element,
                        }
                    } else {
                        MirInstr::UninitStorageDestroy {
                            dest,
                            storage,
                            element,
                        }
                    });
                    return Some(dest);
                }
            }
        }
        // `pointer.unsafe_offset(i)` is provenance-preserving element
        // arithmetic: the ordinary pointer `+` operation.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerOffset
            )
        }) {
            let pointer = self.expr(object);
            let offset = self.expr(
                args.first()
                    .expect("checked unsafe_offset has one argument"),
            );
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::BinOp {
                op: mojito_ast::ast::InfixOp::Add,
                dest,
                a: pointer,
                b: offset,
                resolved: None,
            });
            return Some(dest);
        }
        // `pointer.unsafe_write(value)` / `unsafe_write(copy=v)`
        // initializes the pointee at offset 0 — the same store family
        // as `pointer[] = value`. An origin-bearing pointer writes its
        // source place (owner substitution when stably bound, else
        // through the runtime handle); a heap pointer stores through a
        // synthetic binding so chained receivers stay expressible.
        let pointer_write = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::PointerWrite { element, copy } => {
                    Some((element, copy))
                }
                _ => None,
            });
        if let Some((element, copy)) = pointer_write {
            let value_expr = args
                .first()
                .or_else(|| kwargs.first().map(|keyword| &keyword.value))
                .expect("checked unsafe_write has one value");
            let mut src = self.expr(value_expr);
            if copy {
                let copied = self.fresh_typed(span(e), None, element.clone());
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: src,
                });
                src = copied;
            }
            if let Some(target) = self.pointer_deref_place(object) {
                self.emit(MirInstr::Store { place: target, src });
            } else if self.is_origin_bearing_pointer(object) {
                let reference = self.expr(object);
                self.emit(MirInstr::WriteRef {
                    reference,
                    value: src,
                });
            } else {
                let pointer = self.expr(object);
                let pointer_ty = self.checked_ty(object);
                let var = self.fresh_var();
                if let Some(ty) = pointer_ty.clone() {
                    self.var_types.insert(var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var,
                    src: pointer,
                    binding_ty: pointer_ty.clone(),
                });
                let index = self.constant(e, Const::Int(0));
                let mut place = MirPlace::root(var, pointer_ty);
                place.project(Proj::Index(index), element);
                self.emit(MirInstr::Store { place, src });
            }
            let dest = self.fresh_typed(span(e), None, Ty::None);
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return Some(dest);
        }
        None
    }

    /// The ordinary method call on a value receiver, including an explicit
    /// destructor call and an implicitly copied consuming receiver.
    fn ordinary_method_call(
        &mut self,
        e: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        let explicit_destroy = self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ExplicitDestroy
            )
        });
        let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
        // If the receiver is a place, load it through that place (indices
        // evaluated once) and keep the place for write-back; otherwise it is
        // a temporary evaluated for its value only.
        let receiver_expr = if explicit_destroy {
            match &object.kind {
                ExprKind::Transfer(inner) => inner.as_ref(),
                _ => object,
            }
        } else {
            object
        };
        let (recv, recv_place) = self.lower_method_receiver(e, receiver_expr);
        let recv = if implicitly_copied_receiver {
            self.copy_consuming_receiver(receiver_expr, recv, recv_place.as_ref())
        } else {
            recv
        };
        // Retain checker-selected `mut`/`ref` ordinary-argument places,
        // mirroring a free-function `Call` — and the shared-read places
        // a borrowing-view result lends to. Loan-carrying temporary
        // arguments anchor exactly as in a free call unless the
        // callee's `ref`-argument borrows or transfer effects already
        // carry their loans (see `call_anchors_arguments`).
        let view_result = self.borrows_view_result(e);
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = self.call_anchors_arguments(e);
        let (regs, arg_places) = self.lower_call_arguments(args, view_result);
        self.allow_argument_anchors = saved_anchor_permission;
        let (kw, kwarg_places) = self.lower_call_keywords(kwargs, view_result);
        // A wrapped `.format(...)` keeps its own callee but types its
        // register as the compile-time string the nominal-String
        // conversion consumes (mirroring the free-call builtins).
        let d = if method == "format" && self.implicit_conversion(e).is_some() {
            self.fresh_typed(span(e), None, Ty::StringLiteral)
        } else {
            self.fresh(span(e), None)
        };
        self.emit_interior_invalidations(receiver_expr, None);
        self.emit_call_invalidations(e, args, kwargs);
        let capture_accesses = self.checked_call_capture_accesses(e);
        // An ordinary method call can still select a generic method and
        // infer all of its compile-time arguments from runtime actuals.
        // Preserve that declaration vocabulary even though there are no
        // explicit `method[...]` value arguments to lower.
        let param_decls = self
            .checked_call_contract(e)
            .map(|contract| contract.param_decls)
            .unwrap_or_default();
        let transfer_recv_place = recv_place.clone();
        let transfer_arg_places = arg_places.clone();
        self.emit(MirInstr::MethodCall {
            dest: d,
            recv,
            method: method.to_owned(),
            resolved: self.resolved_callable(e),
            raises: self.checked_raises(e),
            reference_result: self
                .checked_call_contract(e)
                .and_then(|contract| contract.reference_result),
            result_adapter: self
                .checked_call_contract(e)
                .and_then(|contract| contract.result_adapter),
            args: regs,
            kwargs: kw,
            // An explicit-destructor call keeps its receiver place:
            // the VM writes the callee's final `self` state back before
            // the trailing `ConsumeVar`/`ConsumePlace`, so residual
            // destruction sees what the named destructor left (moved
            // fields are tombstones, drained containers are empty).
            recv_place: if implicitly_copied_receiver {
                None
            } else {
                recv_place
            },
            recv_writes: self.receiver_writes(e),
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs: Vec::new(),
            param_decls,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(e, transfer_recv_place.as_ref(), &transfer_arg_places);
        if explicit_destroy
            && !implicitly_copied_receiver
            && let Some(place) = self.try_place(receiver_expr)
        {
            if place.proj.is_empty() {
                self.emit(MirInstr::ConsumeVar { var: place.root });
            } else {
                self.emit(MirInstr::ConsumePlace {
                    place,
                    marker: recv,
                });
            }
        }
        d
    }
}
