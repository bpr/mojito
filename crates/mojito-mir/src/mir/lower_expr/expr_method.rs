//! The method-call arm of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedExpr, CheckedOp};
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    pub(super) fn method_call_expr(&mut self, call: Node<'a>, method: &str) -> Reg {
        let node = call.checked;
        let object = call
            .object()
            .expect("a method call's receiver is its operand");
        let args = call.args();
        let kwargs = call.kwargs();
        let kwargs = kwargs.as_slice();
        // An instance method called through its type
        // (`List[Int].__len__(xs)`): the checker typed the first
        // argument as the receiver, so lower exactly that call.
        let receiver_argument = node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ReceiverFromFirstArgument { .. }
            )
        });
        let (object, args): (Node<'a>, &[Node<'a>]) = if receiver_argument {
            let Some((receiver, rest)) = args.split_first() else {
                unreachable!("checked type-receiver call carries a receiver argument")
            };
            (*receiver, rest)
        } else {
            (object, args.as_slice())
        };
        // A string literal's `format`: the checker typed the static call of
        // the bundled stand-in with the literal as its first argument.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::LiteralFormat
            )
        }) {
            let target = callable_target(&node.adjustments).unwrap_or_else(|| {
                format!(
                    "{}.{}",
                    mojito_symbol::symbol::STDLIB_FORMAT_UTILS_STRUCT,
                    mojito_symbol::symbol::FORMAT_LITERAL_METHOD
                )
            });
            return self.static_method_call(call, &target, None, Some(object), args, kwargs);
        }
        if let Some(dest) = self.method_call_value_form(call, object, method, args, kwargs) {
            return dest;
        }
        if let Some(dest) = self.method_call_storage_form(call, object, method, args, kwargs) {
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
            let target = callable_target(&node.adjustments)
                .unwrap_or_else(|| format!("{type_name}.{method}"));
            let receiver = Self::static_receiver(object);
            return self.static_method_call(call, &target, receiver, None, args, kwargs);
        }
        if let Some(dest) = self.lower_elided_receiver_call(call, object, args, kwargs) {
            return dest;
        }
        self.ordinary_method_call(call, object, method, args, kwargs)
    }

    /// The struct a static call's receiver names, when the receiver's node
    /// is a type rather than a value: a type application (`Dict[Int, Int]`),
    /// a name the check bound to nothing (`Point`), or
    /// the single-argument application that parses as a value subscript over
    /// such a name (`Box[String]`). The bracket content is a compile-time
    /// argument the checker already resolved into its symbol selection —
    /// never lowered.
    pub(in crate::mir) fn type_receiver_name(&self, object: Node<'a>) -> Option<String> {
        let node = object.checked;
        match &node.op {
            CheckedOp::TypeApply { name, .. } => Some(name.clone()),
            CheckedOp::Index { object: base, .. } => {
                self.type_name(self.checked.expression(*base)?)
            }
            _ => self.type_name(node),
        }
    }

    /// The instance a static call's spelled receiver names (`Pair[Int]`),
    /// which the checker recorded as the receiver expression's type. Inside
    /// a static method of a value-parameterized struct, the receiver may
    /// spell that struct's binders (`W[Self.k]`), which the elaborator
    /// substitutes as it does any binder of the body.
    pub(super) fn static_receiver(object: Node<'a>) -> Option<Ty> {
        object
            .ty()
            .filter(|ty| matches!(ty.kind(), TyKind::Struct(_, arguments) if !arguments.is_empty()))
    }

    /// The type name node `node` reads: a name the check bound to nothing
    /// (`Self` in a static is typed as its instance, yet names no local). A
    /// value binder of the enclosing declaration
    /// (`dt.is_integral()` in a `DType`-keyed body) is a value, read through
    /// its slot.
    fn type_name(&self, node: &CheckedExpr) -> Option<String> {
        let CheckedOp::Identifier(name) = &node.op else {
            return None;
        };
        let name = self.spelling(node, name);
        (node.binding.is_none()
            && !self
                .enclosing_binders
                .values
                .iter()
                .any(|(binder, _)| binder.name.as_ref() == name.as_str()))
        .then_some(name)
    }

    /// A call of the checker-selected static `target` over `args`, after a
    /// `leading` argument the call spells as its receiver (a literal's
    /// `format`), the receiver instance a spelled type names (`Pair[Int]`)
    /// carried beside.
    fn static_method_call(
        &mut self,
        call: Node<'a>,
        target: &str,
        receiver: Option<Ty>,
        leading: Option<Node<'a>>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let lends_reads = call.lends_read_arguments();
        let (mut regs, mut arg_places) = (Vec::new(), Vec::new());
        self.open_call_setup();
        for slice in [leading.as_slice(), args] {
            let (slice_regs, slice_places) = self.lower_call_arguments(slice, lends_reads);
            regs.extend(slice_regs);
            arg_places.extend(slice_places);
        }
        self.allow_argument_anchors = saved_anchor_permission;
        let (kw, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        // A leading operand shifts the positional sources off the contract's.
        self.close_call_setup(leading.is_none().then_some(call));
        let dest = self.fresh(call.location(), None);
        if let Some(leading) = leading {
            self.emit_interior_invalidations(leading, None);
        }
        self.emit_call_invalidations(call, args, kwargs);
        // A generic static its template serves takes its inferred value
        // parameters and solved arguments as a generic `def` does; a
        // per-call clone (`Lanes.widen$y…`) has baked them.
        let served =
            mojito_symbol::symbol::split_method_symbol(target).is_some_and(|(_, method)| {
                mojito_symbol::symbol::specialization_template(method).is_none()
            });
        let (param_arg_regs, instantiated_args) = if served {
            (Vec::new(), call.instantiated_args())
        } else {
            (Vec::new(), Vec::new())
        };
        self.record_call(dest, Some(target.to_string()), 0);
        let raises = self.raises(call.raises(), call.location());
        self.emit_call(MirInstr::Call {
            dest,
            func: FuncRef::named(target),
            raises: raises.clone(),
            args: regs,
            kwargs: kw,
            arg_places,
            kwarg_places,
            capture_accesses: self.call_capture_accesses(call),
            param_arg_regs,
            receiver,
            instantiated_args,
            spread: Self::spread_position(args)
                .map(|position| position + usize::from(leading.is_some())),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.error_region(raises);
        dest
    }

    /// Method calls the checker resolved to a value operation with no callee
    /// body: SIMD gathers and reinterprets, identity conversions, the
    /// `write_to`/`write_repr_to` swaps, and the unparameterized Variant
    /// operations. `None` leaves the call to the later forms.
    fn method_call_value_form(
        &mut self,
        call: Node<'a>,
        object: Node<'a>,
        method: &str,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Option<Reg> {
        let node = call.checked;
        // `v.join(w)`: the checker resolved a two-operand lane gather.
        let simd_join = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask, joined: true } => {
                    Some(mask.clone())
                }
                _ => None,
            });
        if let Some(mask) = simd_join
            && let [argument] = args
        {
            let value = self.expr(object);
            let other = Some(self.expr(*argument));
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param }) =
            node.adjustments
                .iter()
                .find(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { .. }
                    )
                })
                .cloned()
        {
            let (kwargs, kwarg_places) = self
                .copy_construction_keywords(kwargs)
                .unwrap_or_else(|| self.lower_call_keywords(kwargs, false));
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::ConstructTypeParam {
                dest,
                param,
                kwargs,
                kwarg_places,
                element: None,
            });
            return Some(dest);
        }
        // `x.__mlir_index__()` on an integer is the identity
        // conversion to the index representation (`Int`).
        if method == "__mlir_index__"
            && args.is_empty()
            && kwargs.is_empty()
            && object.ty().is_some_and(|ty| {
                matches!(
                    ty.kind(),
                    mojito_types::types::TyKind::Int
                        | mojito_types::types::TyKind::IntLiteral
                        | mojito_types::types::TyKind::UInt
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
            && object
                .ty()
                .is_some_and(|ty| mojito_types::types::builtin_copy_is_value_read(&ty))
        {
            return Some(self.expr(object));
        }
        // `x.write_to(writer)` on a receiver without a `write_to`
        // body of its own (checker-marked): receiver and argument
        // swap into the `writer.write(x)` shape, the writer's place
        // retained for the accumulator write-back.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::InvertedWrite
            )
        }) {
            let writer = *args.first().expect("checked write_to has one writer");
            let (recv, recv_place) = self.lower_call_receiver(writer);
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = call.call_anchors_arguments();
            let (regs, arg_places) = self.lower_call_arguments(&[object], false);
            self.allow_argument_anchors = saved_anchor_permission;
            let d = self.fresh_typed(call.location(), None, Ty::none());
            self.emit_interior_invalidations(writer, None);
            self.emit(MirInstr::MethodCall {
                dest: d,
                recv,
                method: mojito_symbol::symbol::WRITE_FORMATTED.to_string(),
                resolved: None,
                raises: None,
                reference_result: None,
                result_adapter: None,
                args: regs,
                kwargs: Vec::new(),
                recv_place: recv_place.map(Box::new),
                recv_writes: true,
                receiver_convention: Some(mojito_ast::ast::ArgConvention::Mut),
                arg_places,
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_args: Vec::new(),
                spread: None,
            });
            return Some(d);
        }
        // `write_repr_to` on a bounded or intrinsic Writable has no
        // concrete receiver body. Produce `repr(receiver)` once, typed
        // as the checked `repr` result, and feed the text through the
        // supplied Writer's ordinary `write` path.
        if let Some(repr_ty) = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::InvertedReprWrite { repr } => {
                    Some(repr.clone())
                }
                _ => None,
            })
        {
            let writer = *args.first().expect("checked write_repr_to has one writer");
            let value = self.read_operand(object);
            let repr = self.fresh_typed(object.location(), None, repr_ty);
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
            let d = self.fresh_typed(call.location(), None, Ty::none());
            self.emit_interior_invalidations(writer, None);
            self.emit(MirInstr::MethodCall {
                dest: d,
                recv,
                method: mojito_symbol::symbol::WRITE_FORMATTED.to_string(),
                resolved: None,
                raises: None,
                reference_result: None,
                result_adapter: None,
                args: vec![repr],
                kwargs: Vec::new(),
                recv_place: recv_place.map(Box::new),
                recv_writes: true,
                receiver_convention: Some(mojito_ast::ast::ArgConvention::Mut),
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_args: Vec::new(),
                spread: None,
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
                node.adjustments
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::SimdToBits {
                            dtype,
                            width,
                        } => Some((dtype.clone(), width.clone())),
                        _ => None,
                    })
        {
            let value = self.expr(object);
            let dest = self.fresh(call.location(), None);
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
        if let Some(index) = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::VariantSetInitWith {
                    index, ..
                } => Some(index.clone()),
                _ => None,
            })
        {
            let place = self
                .try_place(object)
                .expect("checked Variant.set receiver is a writable place");
            let factory = self.expr(
                kwargs
                    .first()
                    .expect("checked Variant.set(init_with=) has one factory")
                    .1,
            );
            let dest = self.fresh(call.location(), None);
            self.emit_interior_invalidations(call, None);
            self.emit(MirInstr::VariantSetInitWith {
                dest,
                place,
                index,
                factory,
                destructors: Vec::new(),
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return Some(dest);
        }
        // An `enum` storage's discriminant (`storage.get_discriminant()`)
        // reads the tag, as `isa[T]()` tests it.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::VariantTag { .. }
            )
        }) {
            let variant = self.expr(object);
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::VariantTag { dest, variant });
            return Some(dest);
        }
        // The parameterless Variant owning operation
        // (`v^.deinit_with(handler)`) is spelled as an ordinary method
        // call rather than a parameterized invoke.
        if let Some(index) = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::VariantDeinitWith {
                    index, ..
                } => Some(index.clone()),
                _ => None,
            })
        {
            let place = self
                .try_place(object)
                .expect("checked Variant.deinit_with receiver is an owned place");
            let variant = self.fresh(object.location(), None);
            self.emit(MirInstr::MovePlace {
                dest: variant,
                place,
            });
            let handler = self.expr(
                *args
                    .first()
                    .expect("checked Variant.deinit_with has one handler"),
            );
            let dest = self.fresh(call.location(), None);
            self.emit_interior_invalidations(call, None);
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
        call: Node<'a>,
        object: Node<'a>,
        method: &str,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Option<Reg> {
        let node = call.checked;
        // A callable-typed FIELD invocation (`holder.callback(1)`)
        // loads the stored value and calls indirectly; the callee
        // place is the field's, so a closure environment stays
        // reachable through stable storage.
        if let Some(mojito_checked::checked::SemanticAdjustment::FieldInvocation { callable }) =
            node.adjustments
                .iter()
                .find(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::FieldInvocation { .. }
                    )
                })
                .cloned()
        {
            let (recv, recv_place) = self.lower_call_receiver(object);
            let callee = self.fresh_typed(call.location(), None, callable.clone());
            self.emit(MirInstr::GetField {
                dest: callee,
                base: recv,
                field: method.to_owned(),
            });
            let callee_place = recv_place.map(|mut place| {
                place.project(Proj::Field(method.to_owned()), callable.clone());
                place
            });
            let resolved = callable_target(&node.adjustments);
            let raises = call.raises();
            return Some(self.emit_indirect_invocation(
                call,
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
        let pointer_storage = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::PointerStorageTake { element } => {
                    Some((true, element.clone()))
                }
                mojito_checked::checked::SemanticAdjustment::PointerStorageDestroy { element } => {
                    Some((false, element.clone()))
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
                Some(index) => self.expr(*index),
                None => self.constant(call, Const::Int(0)),
            };
            debug_assert!(kwargs.is_empty());
            let dest = self.fresh(call.location(), None);
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
                    destructor: None,
                }
            });
            return Some(dest);
        }
        // `__mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(p))`:
        // the place is defined from here, with nothing written.
        // `lit.ownership.mark_destroyed` over the same operand ends the
        // place's value, with nothing destroyed.
        let marked = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::MarkInitialized => Some(true),
                mojito_checked::checked::SemanticAdjustment::MarkDestroyed => Some(false),
                _ => None,
            });
        if let Some(initialized) = marked {
            let operand = match args {
                [argument] if matches!(argument.op(), CheckedOp::Call { .. }) => {
                    argument.args().first().copied()
                }
                _ => None,
            }
            .expect("a checked ownership mark names a place");
            let place = self.place(operand);
            self.emit(if initialized {
                MirInstr::MarkInitialized { place }
            } else {
                MirInstr::MarkDestroyed { place }
            });
            let dest = self.fresh_typed(call.location(), None, Ty::none());
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return Some(dest);
        }
        // Compiler-private inline uninit storage (`MaybeUninit`'s
        // field). `unsafe_write` stores through the payload projection
        // — the place is opaque to drop elaboration, so a previously
        // written payload is overwritten raw (it leaks by design).
        // `take`/`destroy` consume the transferred storage value.
        let uninit_storage = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::UninitStorageWrite { element } => {
                    Some((UninitStorageOp::Write, element.clone()))
                }
                mojito_checked::checked::SemanticAdjustment::UninitStorageTake { element } => {
                    Some((UninitStorageOp::Take, element.clone()))
                }
                mojito_checked::checked::SemanticAdjustment::UninitStorageDestroy { element } => {
                    Some((UninitStorageOp::Destroy, element.clone()))
                }
                _ => None,
            });
        if let Some((op, element)) = uninit_storage {
            debug_assert!(kwargs.is_empty());
            match op {
                UninitStorageOp::Write => {
                    let src = self.expr(*args.first().expect("checked unsafe_write has one value"));
                    let mut place = self.place(object);
                    place.project(Proj::UninitPayload, element);
                    self.emit(MirInstr::Store { place, src });
                    let dest = self.fresh_typed(call.location(), None, Ty::none());
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    return Some(dest);
                }
                UninitStorageOp::Take | UninitStorageOp::Destroy => {
                    let storage = self.expr(object);
                    let dest = self.fresh(call.location(), None);
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
                            destructor: None,
                        }
                    });
                    return Some(dest);
                }
            }
        }
        // `pointer.unsafe_offset(i)` is provenance-preserving element
        // arithmetic: the ordinary pointer `+` operation.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerOffset { .. }
            )
        }) {
            let pointer = self.expr(object);
            let offset = self.expr(
                *args
                    .first()
                    .expect("checked unsafe_offset has one argument"),
            );
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::BinOp {
                op: mojito_ast::ast::InfixOp::Add,
                dest,
                a: pointer,
                b: offset,
                resolved: None,
                raises: None,
            });
            return Some(dest);
        }
        // `pointer.unsafe_write(value)` / `unsafe_write(copy=v)`
        // initializes the pointee at offset 0 — the same store family
        // as `pointer[] = value`. An origin-bearing pointer writes its
        // pointee (its address substituted when statically known, else
        // through the runtime handle); a heap pointer stores through a
        // synthetic binding so chained receivers stay expressible.
        let pointer_write = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::PointerWrite { element, copy } => {
                    Some((element.clone(), *copy))
                }
                _ => None,
            });
        if let Some((element, copy)) = pointer_write {
            let value_expr = args
                .first()
                .or_else(|| kwargs.first().map(|(_, value)| value))
                .expect("checked unsafe_write has one value");
            let src = if copy {
                let value = self.read_operand(*value_expr);
                let copied = self.fresh_typed(call.location(), None, element.clone());
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value,
                    constructor: None,
                });
                copied
            } else {
                self.expr(*value_expr)
            };
            if let Some(target) = self.pointer_deref_place(object) {
                self.emit(MirInstr::Store { place: target, src });
            } else if object.is_origin_bearing_pointer() {
                let reference = self.expr(object);
                self.emit(MirInstr::WriteRef {
                    reference,
                    value: src,
                });
            } else {
                let pointer = self.expr(object);
                let pointer_ty = object.ty();
                let var = self.fresh_var();
                if let Some(ty) = pointer_ty.clone() {
                    self.var_types.insert(var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var,
                    src: pointer,
                    binding_ty: pointer_ty.clone(),
                    in_place: false,
                });
                let index = self.constant(call, Const::Int(0));
                let mut place = MirPlace::root(var, pointer_ty);
                place.project(Proj::Index(index), element);
                self.emit(MirInstr::Store { place, src });
            }
            let dest = self.fresh_typed(call.location(), None, Ty::none());
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
        call: Node<'a>,
        object: Node<'a>,
        method: &str,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let node = call.checked;
        let explicit_destroy = node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ExplicitDestroy
            )
        });
        let implicitly_copied_receiver = call.implicitly_copies_consuming_receiver();
        // If the receiver is a place, load it through that place (indices
        // evaluated once) and keep the place for write-back; otherwise it is
        // a temporary evaluated for its value only.
        let receiver_expr = if explicit_destroy {
            object.transfer_operand().unwrap_or(object)
        } else {
            object
        };
        let (recv, recv_place) =
            self.lower_setup_receiver(call, receiver_expr, implicitly_copied_receiver);
        // Retain checker-selected `mut`/`ref` ordinary-argument places,
        // mirroring a free-function `Call` — and the shared-read places
        // a borrowing-view or reference result lends to. Loan-carrying temporary
        // arguments anchor exactly as in a free call unless the
        // callee's `ref`-argument borrows or transfer effects already
        // carry their loans (see `call_anchors_arguments`).
        let lends_reads = call.lends_read_arguments();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let (regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        self.allow_argument_anchors = saved_anchor_permission;
        let (kw, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        self.close_call_setup(Some(call));
        let d = self.fresh(call.location(), None);
        self.emit_interior_invalidations(receiver_expr, None);
        self.emit_call_invalidations(call, args, kwargs);
        let capture_accesses = self.call_capture_accesses(call);
        // An ordinary method call can still select a generic method and
        // infer all of its compile-time arguments from runtime actuals.
        // Preserve that declaration vocabulary even though there are no
        // explicit `method[...]` value arguments to lower.
        let param_decls = call
            .call_contract()
            .map(|contract| contract.param_decls)
            .unwrap_or_default();
        let transfer_recv_place = recv_place.clone();
        let transfer_arg_places = arg_places.clone();
        // A value parameter the call leaves to inference travels in its
        // solved arguments, as a generic `def`'s does; a per-call clone has
        // baked every parameter and takes none.
        let (param_arg_regs, instantiated_args) = if param_decls.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (Vec::new(), call.instantiated_args())
        };
        self.record_call(d, callable_target(&node.adjustments), 0);
        let raises = self.raises(call.raises(), call.location());
        self.emit_call(MirInstr::MethodCall {
            dest: d,
            recv,
            method: method.to_owned(),
            resolved: callable_target(&node.adjustments),
            raises: raises.clone(),
            reference_result: (call
                .call_contract()
                .and_then(|contract| contract.reference_result))
            .map(Box::new),
            result_adapter: call
                .call_contract()
                .and_then(|contract| contract.result_adapter)
                .map(MirResultAdapter::new),
            args: regs,
            kwargs: kw,
            // An explicit-destructor call keeps its receiver place, which
            // the call consumes (a `deinit` `receiver_convention`): the callee owns the
            // receiver from then on and destroys its fields itself.
            recv_place: if implicitly_copied_receiver {
                None
            } else {
                recv_place.map(Box::new)
            },
            recv_writes: call.receiver_writes(),
            receiver_convention: call.receiver_convention(object),
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread: Self::spread_position(args),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, transfer_recv_place.as_ref(), &transfer_arg_places);
        self.error_region(raises);
        d
    }
}
