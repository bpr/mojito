//! The method-call arm of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedExpr, CheckedOp};

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
        let (object, args): (Node<'a>, &[Node<'a>]) = if receives_first_argument(node) {
            let Some((receiver, rest)) = args.split_first() else {
                unreachable!("checked type-receiver call carries a receiver argument")
            };
            (*receiver, rest)
        } else {
            (object, args.as_slice())
        };
        if let Some(op) = Self::checked_mlir_op(call) {
            return self.lower_mlir_op(call, op, args);
        }
        if let Some(called) = Self::checked_llvm_intrinsic(call) {
            return self.lower_llvm_intrinsic(call, called, args);
        }
        if let Some(op) = Self::checked_pop_array(call) {
            return self.lower_pop_array_op(call, op, args);
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
    /// a name the check bound to nothing (`Point`), the
    /// single-argument application that parses as a value subscript over
    /// such a name (`Box[String]`), or `type_of(operand)`, the instance
    /// the checker recorded it denotes. The bracket content is a
    /// compile-time argument the checker already resolved into its symbol
    /// selection — never lowered.
    pub(in crate::mir) fn type_receiver_name(&self, object: Node<'a>) -> Option<String> {
        let node = object.checked;
        match &node.op {
            CheckedOp::TypeApply { name, .. } => Some(name.clone()),
            CheckedOp::Call { name, .. } if name == "type_of" => node
                .ty
                .as_ref()
                .and_then(mojito_types::types::nominal_parts)
                .map(|(name, _)| name.to_string()),
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
        object.ty().filter(|ty| {
            ty.kind()
                .as_non_simd_struct()
                .is_some_and(|(_, arguments)| !arguments.is_empty())
        })
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
        if args.is_empty()
            && kwargs.is_empty()
            && let Some(converted) = self.mlir_conversion(call, object, method)
        {
            return Some(converted);
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
        None
    }

    /// An argument-free conversion the checker resolved to upstream's MLIR
    /// storage or builtin form of `object`: an integer literal's
    /// `__mlir_index__`, or a collector's `get_loaded_kgen_pack`. `None` leaves the call to the later forms.
    fn mlir_conversion(&mut self, call: Node<'a>, object: Node<'a>, method: &str) -> Option<Reg> {
        // `n.__mlir_index__()` on an integer literal is upstream's
        // conversion of the literal, materialized as an `Int`, to the
        // builtin `index`: `Int.__mlir_index__`'s `pop.cast_to_builtin` of
        // the `Int`'s storage. A `SIMD`'s is its declared member.
        if method == "__mlir_index__" && object.ty().is_some_and(|ty| ty.is_int_literal()) {
            let value = self.expr(object);
            let value = self.materialize_literal(value, &Ty::int(), call.location());
            let storage = self.simd_storage_operand(value, call.location());
            return Some(self.builtin_cast(call.location(), storage, Ty::index()));
        }
        // A heterogeneous collector's `get_loaded_kgen_pack()` loads a
        // `VariadicPack`'s elements through its references
        // (`kgen.struct.load_indirect`), and is an owned collector's storage
        // itself.
        let collector = object
            .ty()
            .filter(mojito_types::types::is_heterogeneous_collector);
        if method == "get_loaded_kgen_pack"
            && let Some(collector) = collector
        {
            let Some(loaded) = mojito_types::types::loaded_pack_type(&collector) else {
                return Some(self.expr(object));
            };
            let TyKind::Tuple(elements) = loaded.kind() else {
                return Some(self.expr(object));
            };
            let references_ty: Ty = TyKind::LitRefPack {
                elements: elements.clone(),
                origin: mojito_types::origin::PointerOrigin::Untracked { mutable: false },
            }
            .into();
            // The pack's references are read where the collector lies,
            // without copying the pack.
            let references = self.fresh_typed(call.location(), None, references_ty.clone());
            if let Some(mut place) = self.simple_place(object) {
                place.project(Proj::Field("_value".to_string()), references_ty);
                self.emit(MirInstr::LoadPlace {
                    dest: references,
                    place,
                });
            } else {
                let pack = self.expr(object);
                self.emit(MirInstr::GetField {
                    dest: references,
                    base: pack,
                    field: "_value".to_string(),
                });
            }
            let dest = self.fresh_typed(call.location(), None, loaded);
            self.emit(MirInstr::LoadIndirect {
                dest,
                pack: references,
            });
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
                Some(index) => self.index_operand(*index),
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
        // `p._get_kgen_pointer()` retypes the receiver's address.
        if call.is_pointer_convert() {
            return Some(self.pointer_convert(call, object));
        }
        // `pointer.unsafe_offset(i)` is provenance-preserving element
        // arithmetic: the ordinary pointer `+` operation, its literal
        // offset converted at `Int` as upstream's `index(idx)` does.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerOffset { .. }
            )
        }) {
            let pointer = self.expr(object);
            let offset = self.index_operand(
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
                refined_bound: None,
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
            self.write_pointee(call, object, element, src);
            let dest = self.fresh_typed(call.location(), None, Ty::none());
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return Some(dest);
        }
        // `dst.unsafe_write_move_from(src)`: upstream's
        // `__get_address_as_owned_value(src)` is the constructor-free take;
        // the owned value is then moved out of its binding, which names the
        // element's move initializer, into the receiver's slot.
        let move_from = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::PointerWriteMoveFrom { element } => {
                    Some(element.clone())
                }
                _ => None,
            });
        if let Some(element) = move_from {
            let source = *args
                .first()
                .expect("checked unsafe_write_move_from has one source");
            let pointer = self.expr(source);
            let index = self.constant(call, Const::Int(0));
            let taken = self.fresh_typed(call.location(), None, element.clone());
            self.emit(MirInstr::PointerStorageTake {
                dest: taken,
                pointer,
                index,
                element: element.clone(),
            });
            let var = self.fresh_var();
            self.var_types.insert(var, element.clone());
            self.emit(MirInstr::DefVar {
                var,
                src: taken,
                binding_ty: Some(element.clone()),
                in_place: false,
            });
            let src = self.fresh_typed(call.location(), Some(var), element.clone());
            self.emit(MirInstr::UseVar {
                dest: src,
                var,
                mode: UseMode::Move,
                constructor: None,
            });
            self.write_pointee(call, object, element, src);
            let dest = self.fresh_typed(call.location(), None, Ty::none());
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return Some(dest);
        }
        None
    }

    /// Write `src` into the pointee at offset 0 of pointer `object`, an
    /// `element`: through its statically known place, through the runtime
    /// handle of an origin-bearing pointer, or through a synthetic binding
    /// of a heap pointer, so chained receivers stay expressible.
    fn write_pointee(&mut self, call: Node<'a>, object: Node<'a>, element: Ty, src: Reg) {
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
    }

    /// The ordinary method call on a value receiver, including an explicit
    /// destructor call and an implicitly copied consuming receiver.
    pub(super) fn ordinary_method_call(
        &mut self,
        call: Node<'a>,
        object: Node<'a>,
        method: &str,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        // If the receiver is a place, load it through that place (indices
        // evaluated once) and keep the place for write-back; otherwise it is
        // a temporary evaluated for its value only.
        let (recv, recv_place) = self.lower_setup_receiver(
            call,
            call.destroyed_receiver(object),
            call.implicitly_copies_consuming_receiver(),
        );
        let (positional, keywords) = self.lower_method_arguments(call, args, kwargs);
        self.close_call_setup(Some(call));
        self.emit_method_call(
            call,
            object,
            method,
            MethodCallOperands {
                recv,
                recv_place,
                positional,
                keywords,
            },
            (args, kwargs),
        )
    }

    /// Reflected operator `call`, `left op right` answered by the right
    /// operand's `right.__rop__(left)`: its argument, the left operand, is
    /// lowered before its receiver, as upstream evaluates both operands
    /// before it resolves the call.
    pub(super) fn reflected_operator_call(
        &mut self,
        call: Node<'a>,
        method: &str,
        left: Node<'a>,
        right: Node<'a>,
    ) -> Reg {
        self.open_call_setup();
        let (positional, keywords) = self.lower_method_arguments(call, &[left], &[]);
        let (recv, recv_place) = self.lower_open_setup_receiver(
            call,
            right,
            call.implicitly_copies_consuming_receiver(),
        );
        self.close_call_setup(Some(call));
        self.emit_method_call(
            call,
            right,
            method,
            MethodCallOperands {
                recv,
                recv_place,
                positional,
                keywords,
            },
            (&[left], &[]),
        )
    }

    /// Lower method call `call`'s arguments into its open setup frame.
    /// Retain checker-selected `mut`/`ref` ordinary-argument places,
    /// mirroring a free-function `Call` — and the shared-read places a
    /// borrowing-view or reference result lends to. Loan-carrying temporary
    /// arguments anchor exactly as in a free call unless the callee's
    /// `ref`-argument borrows or transfer effects already carry their loans
    /// (see `call_anchors_arguments`).
    fn lower_method_arguments(
        &mut self,
        call: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> (
        (Vec<Reg>, Vec<Option<MirPlace>>),
        crate::mir::calls::LoweredKeywords,
    ) {
        let lends_reads = call.lends_read_arguments();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let positional = self.lower_call_arguments(args, lends_reads);
        self.allow_argument_anchors = saved_anchor_permission;
        let keywords = self.lower_call_keywords(kwargs, lends_reads);
        (positional, keywords)
    }

    /// Emit method call `call` of `method` on receiver `object` over its
    /// lowered `operands`, `sources` being the argument expressions they
    /// were lowered from, with its transfers and error region.
    fn emit_method_call(
        &mut self,
        call: Node<'a>,
        object: Node<'a>,
        method: &str,
        operands: MethodCallOperands,
        sources: (&[Node<'a>], &[(&'a str, Node<'a>)]),
    ) -> Reg {
        let node = call.checked;
        let MethodCallOperands {
            recv,
            recv_place,
            positional: (regs, arg_places),
            keywords: (kw, kwarg_places),
        } = operands;
        let (args, kwargs) = sources;
        let d = self.fresh(call.location(), None);
        self.emit_interior_invalidations(call.destroyed_receiver(object), None);
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
            // receiver from then on and destroys its fields itself. An
            // implicitly copied receiver's place is its copy's slot.
            recv_place: recv_place.map(Box::new),
            recv_writes: call.receiver_writes(),
            receiver_convention: call.receiver_convention(object),
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread: Self::spread_position(args),
            refined_bound: call
                .call_contract()
                .and_then(|contract| contract.refined_bound)
                .map(MirRefinedBound::boxed),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, transfer_recv_place.as_ref(), &transfer_arg_places);
        self.error_region(raises);
        d
    }
}

/// Whether the checker typed `call`'s first argument as its receiver: an
/// instance method called through its type (`List[Int].__len__(xs)`).
pub(super) fn receives_first_argument(call: &CheckedExpr) -> bool {
    call.adjustments.iter().any(|adjustment| {
        matches!(
            adjustment,
            mojito_checked::checked::SemanticAdjustment::ReceiverFromFirstArgument { .. }
        )
    })
}

/// One method call's lowered receiver, positional arguments, and keyword
/// arguments, each with the caller place it retains.
struct MethodCallOperands {
    recv: Reg,
    recv_place: Option<MirPlace>,
    positional: (Vec<Reg>, Vec<Option<MirPlace>>),
    keywords: crate::mir::calls::LoweredKeywords,
}
