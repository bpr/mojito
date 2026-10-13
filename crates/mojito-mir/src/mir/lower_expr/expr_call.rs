//! Direct-call and callable-value invocation arms of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedExpr;
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    pub(super) fn call_expr(&mut self, call: Node<'a>, name: &str) -> Reg {
        let node = call.checked;
        let args = call.args();
        let args = args.as_slice();
        let kwargs = call.kwargs();
        let kwargs = kwargs.as_slice();
        if name == "__mojito_fieldwise_copy" {
            return self.expr(
                *args
                    .first()
                    .expect("checked synthesized fieldwise copy has one argument"),
            );
        }
        if let Some(dest) = self.checked_construction(call, args, kwargs) {
            return dest;
        }
        // SIMD construction resolves its `[DType.<dt>, width]` parameters
        // here (the MIR is otherwise untyped about them).
        if let Some(r) = self.try_simd_call(call, args, kwargs) {
            return r;
        }
        // `objs[0](3)` / `grid[i, j](x)`: the checker re-dispatched
        // these value brackets as subscript-then-indirect-call, so the
        // brackets are runtime indices, never compile-time parameters
        // (and never the named-local callable path below).
        if let Some(plan) = call.element_invocation() {
            let indices = element_indices(call);
            // The receiver is the local the call names, read as its place.
            let receiver = self
                .local_callee(call, name)
                .unwrap_or_else(|| self.var(name));
            let place = self.var_place(receiver);
            let value = self.fresh(call.location(), Some(place.root));
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return self.lower_element_invocation(
                call,
                (value, Some(place)),
                plan,
                &indices,
                args,
                kwargs,
            );
        }
        // A call to a nested `def` (a closure, called by name in scope):
        // rewrite to its lifted function, prepending the captured enclosing
        // locals as leading arguments (passed as places, so the `mut`
        // capture parameters write back — reference-capture semantics).
        if let Some(info) = node
            .binding
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
        {
            return self.lower_nested_call(call, name, &info, args, kwargs);
        }
        // The callee is the slot of the local the call's binding names.
        if let Some(variable) = self.local_callee(call, name) {
            let callee = self.read_slot(variable, call.location(), None);
            let callable_ty = self
                .var_types
                .get(&variable)
                .cloned()
                .or_else(|| self.f.reg_types.get(&callee.0).cloned());
            let param_arg_regs = self.indirect_param_arg_regs(call);
            let param_decls = callable_ty
                .as_ref()
                .map(generic_callable_param_decls)
                .unwrap_or_default();
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = call.call_anchors_arguments();
            let lends_reads = call.lends_read_arguments();
            self.open_call_setup();
            let (regs, arg_places) = self.lower_call_arguments(args, lends_reads);
            let (kw, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
            self.close_call_setup(Some(call));
            self.allow_argument_anchors = saved_anchor_permission;
            let place = self.var_place(variable);
            let callee_place = place.is_typed().then_some(place);
            let dest = self.fresh(call.location(), None);
            self.emit_call_invalidations(call, args, kwargs);
            let capture_accesses = self.call_capture_accesses(call);
            let (instantiated_contract, instantiated_args) = call
                .instantiated_callable_contract()
                .map_or((None, Vec::new()), |(contract, arguments)| {
                    (Some(contract), arguments)
                });
            let transfer_arg_places = arg_places.clone();
            // A callable-struct value is the receiver of its own
            // `__call__` transfer effects; its place is the callee's.
            let transfer_recv_place = callee_place.clone();
            self.emit(MirInstr::CallIndirect {
                dest,
                callee,
                resolved: callable_target(&node.adjustments),
                raises: call.raises(),
                args: regs,
                kwargs: kw,
                callee_place: callee_place.map(Box::new),
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls,
                instantiated_contract,
                instantiated_args,
                spread: Self::spread_position(args),
            });
            self.install_call_transfers(call, transfer_recv_place.as_ref(), &transfer_arg_places);
            return dest;
        }
        // `__RuntimeTuple` is the compiler-private heterogeneous pack
        // storage primitive. Public `Tuple` is an ordinary nominal
        // variadic struct and follows the call path below.
        if name == "__RuntimeTuple" && kwargs.is_empty() && !self.overloads.is_function(name) {
            // Moving a whole pack into its storage (`__RuntimeTuple(*args^)`)
            // is one relocation: the collector already holds the elements
            // in storage shape.
            if let [argument] = args
                && let Some(spread) = argument.spread_operand()
                && spread.transfer_operand().is_some_and(Node::is_name)
            {
                return self.expr(spread);
            }
            let regs = self.args(args);
            let element_types = match node.ty.clone().as_ref().map(Ty::kind) {
                Some(TyKind::Tuple(elements)) => Some(elements.clone()),
                _ => None,
            };
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::MakeTuple {
                dest,
                elems: regs,
                element_types,
            });
            return dest;
        }
        self.direct_call(call, name, args, kwargs)
    }

    /// A call the check recorded as a construction or an intrinsic by an
    /// adjustment on its node (a copy, a scalar zero, a type parameter's or
    /// a pack element's construction, a size, a type name, a pointer to a
    /// place, inline storage, a `Variant`): `None` for any other call.
    fn checked_construction(
        &mut self,
        call: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Option<Reg> {
        let node = call.checked;
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BuiltinCopyConstruction
            )
        }) {
            return Some(
                self.expr(
                    kwargs
                        .first()
                        .expect("checked built-in copy construction has a 'copy=' argument")
                        .1,
                ),
            );
        }
        if let Some(zero) = self.default_scalar_construction(node) {
            return Some(zero);
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
        if let Some(dest) = self.pack_element_construction(call) {
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructType { ty }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ConstructType { .. }
                )
            })
            .cloned()
        {
            let dest = self.fresh_typed(call.location(), None, ty.clone());
            self.emit(MirInstr::ConstructType { dest, ty });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::SizeOf { ty }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::SizeOf { .. }
                )
            })
            .cloned()
        {
            // A symbolic type has no layout to take the size of; the
            // elaborator substitutes the instance's type, and only the
            // erased oracle reaches the template's own instruction.
            let dest = self.fresh_typed(call.location(), None, Ty::int());
            self.emit(MirInstr::SizeOf { dest, ty });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ParamListAddress { values }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ParamListAddress { .. }
                )
            })
            .cloned()
        {
            return Some(self.param_list_address(call.location(), values));
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::TypeName { text, ty }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::TypeName { .. }
                )
            })
            .cloned()
        {
            // A type naming a compile-time parameter is spelled per
            // instance, so the template carries the type to the elaborator.
            if mojito_types::types::is_symbolic(&ty) {
                let dest = self.fresh_typed(call.location(), None, Ty::string_literal());
                self.emit(MirInstr::TypeName { dest, ty });
                return Some(dest);
            }
            return Some(self.constant(call, Const::Str(text)));
        }
        // A checked pointer construction materializes the frame/slot
        // handle for its source place; the checked pointer type keeps
        // the origin while the runtime value erases it.
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { .. }
            )
        }) {
            let value = kwargs
                .first()
                .expect("checked pointer construction has a 'to=' argument")
                .1;
            let place = self.place(value);
            let dest = self.fresh(call.location(), Some(place.root));
            self.emit(MirInstr::MakeRef { dest, place });
            return Some(dest);
        }
        // Compiler-private inline uninit-storage construction:
        // uninitialized, or holding a moved initial payload.
        if let Some(init) = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::UninitStorageMake { init, .. } => {
                    Some(*init)
                }
                _ => None,
            })
        {
            let init = init
                .then(|| self.expr(*args.first().expect("checked storage construction payload")));
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::UninitStorage { dest, init });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructVariant {
            alternatives,
            index,
        }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ConstructVariant { .. }
                )
            })
            .cloned()
        {
            let value = self.expr(
                *args
                    .first()
                    .expect("checked Variant construction has one payload"),
            );
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value,
            });
            return Some(dest);
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructVariantInitWith {
            alternatives,
            index,
            payload,
        }) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ConstructVariantInitWith { .. }
                )
            })
            .cloned()
        {
            // Invoke the zero-parameter factory, then wrap its result:
            // the payload temporary is consumed by the construction.
            let factory = self.expr(
                kwargs
                    .first()
                    .expect("checked Variant(init_with=) has one factory")
                    .1,
            );
            let payload = self.fresh_typed(call.location(), None, payload);
            self.emit(MirInstr::CallIndirect {
                dest: payload,
                callee: factory,
                resolved: None,
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                callee_place: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
                instantiated_contract: None,
                instantiated_args: Vec::new(),
                spread: None,
            });
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value: payload,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return Some(dest);
        }
        None
    }

    pub(super) fn invoke_expr(&mut self, call: Node<'a>) -> Reg {
        let node = call.checked;
        let callee = call
            .callee()
            .expect("an invocation's callee is its operand");
        let args = call.args();
        let args = args.as_slice();
        let kwargs = call.kwargs();
        let kwargs = kwargs.as_slice();
        if let Some(dest) = self.pack_element_construction(call) {
            return dest;
        }
        // `pointer.unsafe_origin_cast[...]()` retypes provenance only: the
        // runtime value is the receiver, unchanged, and the origin
        // parameter argument never lowers (origins erase).
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerOriginCast { .. }
            )
        }) && let Some(object) = callee.member_object()
        {
            return self.expr(object);
        }
        // Parameterized SIMD methods (`v.cast[DType.<dt>]()`) carry
        // their checker-resolved payload in the adjustment; the
        // receiver is the member callee's object.
        let simd_cast = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdCast { dtype, width } => {
                    Some((dtype.clone(), width.clone()))
                }
                _ => None,
            });
        if let Some((dtype, width)) = simd_cast
            && let Some(object) = callee.member_object()
        {
            let value = self.expr(object);
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
            });
            return dest;
        }
        // `v.to_bits[DType.<dt>]()` lowers like `cast`, as a lane-wise
        // bit reinterpretation of the member callee's object.
        let simd_to_bits = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdToBits { dtype, width } => {
                    Some((dtype.clone(), width.clone()))
                }
                _ => None,
            });
        if let Some((dtype, width)) = simd_to_bits
            && let Some(object) = callee.member_object()
        {
            let value = self.expr(object);
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::SimdBitcast {
                dest,
                value,
                dtype,
                width,
            });
            return dest;
        }
        let simd_shuffle = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask, joined } => {
                    Some((mask.clone(), *joined))
                }
                _ => None,
            });
        if let Some((mask, joined)) = simd_shuffle
            && let Some(object) = callee.member_object()
        {
            let value = self.expr(object);
            let other = joined.then(|| self.expr(args[0]));
            let dest = self.fresh(call.location(), None);
            self.emit(MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            });
            return dest;
        }
        if let Some(operation) = node
            .adjustments
            .iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::VariantIs { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantSet { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantSetInitWith { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantTake { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantDeinitWith { .. }
                        | mojito_checked::checked::SemanticAdjustment::VariantReplace { .. }
                )
            })
            .cloned()
        {
            return self.variant_operation(call, callee, &operation, args, kwargs);
        }
        // `a.b[i](x)`: the member-base element call re-dispatched by
        // the checker — the callee expression is the subscripted
        // receiver, and the brackets are runtime indices.
        if let Some(plan) = call.element_invocation() {
            let indices = element_indices(call);
            let receiver = self.lower_call_receiver(callee);
            return self.lower_element_invocation(call, receiver, plan, &indices, args, kwargs);
        }
        if parameterized_method_decls(node).is_some() {
            return self.parameterized_method_call(call, callee, args, kwargs);
        }
        let mut callee_place = self.callable_receiver_place(callee);
        let callable_ty = callee.ty();
        let lambda_callee = matches!(callee.op(), CheckedOp::Lambda { .. });
        let callee = self.expr(callee);
        let callable_ty = callable_ty.or_else(|| self.f.reg_types.get(&callee.0).cloned());
        if lambda_callee && callee_place.is_none() {
            // An immediately invoked lambda's closure is a temporary;
            // bind it to a synthetic slot so owned capture slots are
            // called from stable storage like a declaration-owned
            // closure.
            let slot = self.fresh_var();
            if let Some(ty) = &callable_ty {
                self.var_types.insert(slot, ty.clone());
            }
            self.emit(MirInstr::DefVar {
                var: slot,
                src: callee,
                binding_ty: callable_ty.clone(),
                in_place: false,
            });
            let place = MirPlace::root(slot, callable_ty.clone());
            callee_place = place.is_typed().then_some(place);
        }
        let param_arg_regs = self.indirect_param_arg_regs(call);
        let resolved = callable_target(&node.adjustments);
        let raises = call.raises();
        self.emit_indirect_invocation(
            call,
            callee,
            callee_place,
            callable_ty.as_ref(),
            resolved,
            raises,
            param_arg_regs,
            args,
            kwargs,
            true,
        )
    }

    /// The ordinary direct call: compile-time arguments, runtime arguments
    /// and keywords, then the `Call` on the checker-selected target.
    fn direct_call(
        &mut self,
        call: Node<'a>,
        name: &str,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let node = call.checked;
        // Compile-time parameter arguments (`Name[param_args](...)`),
        // evaluated before ordinary call arguments: a
        // **value** parameter is a comptime `Int` expression flattened to a
        // register; a **type** parameter is erased (`None`).
        let param_arg_regs = self.param_arg_regs(call);
        // Retain only checker-selected `mut`/`ref` caller places. A
        // syntactically simple copied argument remains eligible for
        // ASAP destruction after its value has been evaluated.
        // The prelude rewrite renames every use of `String`, including
        // the builtin Writable conversion the checker typed as the
        // compile-time string; route those back to the VM's
        // conversion builtin instead of the nominal constructor.
        // A retargeted `String(x)` stringify carries a
        // `ResolveCallable("String")` adjustment (production path) or
        // keeps the compile-time string checked type (the unlinked
        // seam): both route to the VM's `"String"` conversion builtin
        // with an explicitly literal-typed result — the surrounding
        // implicit-conversion wrap materializes the nominal struct.
        let stringify = mojito_symbol::symbol::is_stdlib_string_struct(name)
            && (callable_target(&node.adjustments).as_deref() == Some("String")
                || node.ty.clone() == Some(Ty::string_literal()));
        // A call's argument list anchors nested loan-carrying
        // temporaries unless another channel already carries their
        // loans (see `call_anchors_arguments`). A construction's
        // aggregate result additionally carries its arguments' loans
        // forward instead (its binding — or its own anchor one call
        // level up — installs them). The stringify conversion is the
        // exception: its result is a fresh owned string that carries
        // no loans, so a view argument anchors as in a plain call.
        let lends_reads = call.lends_read_arguments();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = (stringify
            || !matches!(
                node.ty.clone().as_ref().map(Ty::kind),
                Some(TyKind::Struct(constructed, _)) if *constructed == *name
            ))
            && call.call_anchors_arguments();
        self.open_call_setup();
        let (regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        self.allow_argument_anchors = saved_anchor_permission;
        let copy_construction = args.is_empty()
            && matches!(
                node.ty.clone().as_ref().map(Ty::kind),
                Some(TyKind::Struct(constructed, _)) if *constructed == *name
            );
        let (kw, kwarg_places) = copy_construction
            .then(|| self.copy_construction_keywords(kwargs))
            .flatten()
            .unwrap_or_else(|| self.lower_call_keywords(kwargs, lends_reads));
        self.close_call_setup(Some(call));
        let (regs, arg_places, kw, kwarg_places) = match node.ty.clone().as_ref().map(Ty::kind) {
            Some(TyKind::Struct(constructed, _)) if *constructed == *name => self
                .fieldwise_keywords_positional(constructed, (regs, arg_places, kw, kwarg_places)),
            _ => (regs, arg_places, kw, kwarg_places),
        };
        // Builtin string producers wrapped by the nominal-String
        // conversion keep their own callee but type their register
        // as the compile-time string the wrap consumes.
        let literal_result =
            stringify || (matches!(name, "input" | "repr") && call.implicit_conversion().is_some());
        let target = if stringify {
            "String".to_string()
        } else {
            callable_target(&node.adjustments)
                .unwrap_or_else(|| self.overloaded_name(name, args.len()))
        };
        let d = if literal_result {
            self.fresh_typed(call.location(), None, Ty::string_literal())
        } else {
            self.fresh(call.location(), None)
        };
        self.emit_call_invalidations(call, args, kwargs);
        let capture_accesses = self.call_capture_accesses(call);
        let transfer_arg_places = arg_places.clone();
        let instantiated_args = call.instantiated_args();
        self.emit(MirInstr::Call {
            dest: d,
            func: FuncRef::named(&target),
            raises: call.raises(),
            args: regs,
            kwargs: kw,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            receiver: None,
            instantiated_args,
            spread: Self::spread_position(args),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, None, &transfer_arg_places);
        d
    }

    /// A checked `Variant` operation spelled as a parameterized member call.
    fn variant_operation(
        &mut self,
        call: Node<'a>,
        callee: Node<'a>,
        operation: &mojito_checked::checked::SemanticAdjustment,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let Some(object) = callee.member_object() else {
            unreachable!("checked Variant operation has a member callee")
        };
        match operation.clone() {
            mojito_checked::checked::SemanticAdjustment::VariantIs { index, .. } => {
                let variant = self.expr(object);
                let dest = self.fresh(call.location(), None);
                self.emit(MirInstr::VariantIs {
                    dest,
                    variant,
                    index,
                });
                dest
            }
            // `unsafe_get[T]()`: the method spelling of the checked
            // projection read (`v[T]`).
            mojito_checked::checked::SemanticAdjustment::VariantProject { index, .. } => {
                let variant = self.expr(object);
                let dest = self.fresh(call.location(), None);
                self.emit(MirInstr::VariantGet {
                    dest,
                    variant,
                    index,
                });
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantSet { index, .. } => {
                let place = self
                    .try_place(object)
                    .expect("checked Variant.set receiver is a writable place");
                let value = self.expr(*args.first().expect("checked Variant.set has one payload"));
                let dest = self.fresh(call.location(), None);
                self.emit_interior_invalidations(call, None);
                self.emit(MirInstr::VariantSet {
                    dest,
                    place,
                    index,
                    value,
                });
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantSetInitWith { index, .. } => {
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
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantDeinitWith { index, .. } => {
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
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantTake { index, checked, .. } => {
                let place = self
                    .try_place(object)
                    .expect("checked Variant.take receiver is an owned place");
                let variant = self.fresh(object.location(), None);
                self.emit(MirInstr::MovePlace {
                    dest: variant,
                    place,
                });
                let dest = self.fresh(call.location(), None);
                self.emit_interior_invalidations(call, None);
                self.emit(MirInstr::VariantTake {
                    dest,
                    variant,
                    index,
                    checked,
                });
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantReplace {
                input_index,
                output_index,
                checked,
                ..
            } => {
                let place = self
                    .try_place(object)
                    .expect("checked Variant.replace receiver is writable");
                let value = self.expr(
                    *args
                        .first()
                        .expect("checked Variant.replace has one payload"),
                );
                let dest = self.fresh(call.location(), None);
                self.emit_interior_invalidations(call, None);
                self.emit(MirInstr::VariantReplace {
                    dest,
                    place,
                    input_index,
                    output_index,
                    value,
                    checked,
                });
                dest
            }
            _ => unreachable!("filtered Variant operation"),
        }
    }

    /// A checked parameterized method call (`recv.method[params](args)`).
    fn parameterized_method_call(
        &mut self,
        call: Node<'a>,
        callee: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let node = call.checked;
        let (CheckedOp::Member { field, .. }, Some(object)) = (callee.op(), callee.member_object())
        else {
            unreachable!("checked parameterized method call has a member callee")
        };
        let field = field.clone();
        let param_decls = parameterized_method_decls(node).unwrap_or_default();
        // A parameterized **static** call (`Bag[Int, String].has[Bool]()`,
        // `Lanes.ident[Int](3)`): the receiver is a type, so this
        // is a call on the checker-selected symbol with the
        // method's own compile-time arguments, exactly like the
        // unparameterized static spelling.
        if let Some(name) = self.type_receiver_name(object) {
            // A per-call clone declares no compile-time
            // parameters: the source type arguments are baked
            // into its symbol and occupy no slots.
            let (param_arg_regs, instantiated_args) = if param_decls.is_empty() {
                (Vec::new(), Vec::new())
            } else {
                (self.param_arg_regs(call), call.instantiated_args())
            };
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = call.call_anchors_arguments();
            let lends_reads = call.lends_read_arguments();
            self.open_call_setup();
            let (regs, arg_places) = self.lower_call_arguments(args, lends_reads);
            self.allow_argument_anchors = saved_anchor_permission;
            let (kw, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
            self.close_call_setup(Some(call));
            let dest = self.fresh(call.location(), None);
            let target =
                callable_target(&node.adjustments).unwrap_or_else(|| format!("{name}.{field}"));
            self.emit_call_invalidations(call, args, kwargs);
            let receiver = self.static_receiver(object);
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises: call.raises(),
                args: regs,
                kwargs: kw,
                arg_places,
                kwarg_places,
                capture_accesses: self.call_capture_accesses(call),
                param_arg_regs,
                receiver,
                instantiated_args,
                spread: Self::spread_position(args),
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return dest;
        }
        // Keep this as a direct method invocation. In particular,
        // do not synthesize a bound-method value (which would make
        // its receiver/environment escapable).
        if let Some(dest) = self.lower_elided_receiver_call(call, object, args, kwargs) {
            return dest;
        }
        let implicitly_copied_receiver = call.implicitly_copies_consuming_receiver();
        let (recv, recv_place) =
            self.lower_setup_receiver(call, object, implicitly_copied_receiver);
        // A per-call clone has baked its method parameters into
        // the selected symbol. Source brackets remain in the AST,
        // but an empty declaration list means they occupy no MIR
        // ABI slots.
        let (param_arg_regs, instantiated_args) = if param_decls.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (
                self.outside_call_setup(|this| this.param_arg_regs(call)),
                call.instantiated_args(),
            )
        };
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let lends_reads = call.lends_read_arguments();
        let (argument_regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        self.allow_argument_anchors = saved_anchor_permission;
        let (keyword_regs, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        self.close_call_setup(Some(call));
        let dest = self.fresh(call.location(), None);
        self.emit_interior_invalidations(object, None);
        self.emit_call_invalidations(call, args, kwargs);
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method: field,
            resolved: callable_target(&node.adjustments),
            raises: call.raises(),
            reference_result: (call
                .call_contract()
                .and_then(|contract| contract.reference_result))
            .map(Box::new),
            result_adapter: call
                .call_contract()
                .and_then(|contract| contract.result_adapter),
            args: argument_regs,
            kwargs: keyword_regs,
            recv_place: if implicitly_copied_receiver {
                None
            } else {
                recv_place.map(Box::new)
            },
            recv_writes: call.receiver_writes(),
            arg_places,
            kwarg_places,
            capture_accesses: self.call_capture_accesses(call),
            param_arg_regs,
            param_decls: param_decls.to_vec(),
            instantiated_args,
            spread: Self::spread_position(args),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        dest
    }
}

impl<'a> Flatten<'a> {
    /// `Ts[i]()` or `Self.Ts[i]()` over a symbolic pack: the element
    /// construction the checker recorded, its index the call's one
    /// parameter argument.
    fn pack_element_construction(&mut self, call: Node<'a>) -> Option<Reg> {
        let node = call.checked;
        let pack = node
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructPackElement { pack } => {
                    Some(pack.clone())
                }
                _ => None,
            })?;
        // The elaborator selects the element by the index's expression,
        // a literal one included, which a forwarded argument leaves out.
        let element = self
            .param_arg_regs(call)
            .into_iter()
            .next()
            .map(|mut element| {
                if let (None, Some(CheckedParamArg::Value(index))) =
                    (&element.expr, call.param_args().first())
                {
                    element.expr = self.value_binder_node(*index);
                }
                element
            });
        let dest = self.fresh(call.location(), None);
        self.emit(MirInstr::ConstructTypeParam {
            dest,
            param: pack,
            kwargs: Vec::new(),
            kwarg_places: Vec::new(),
            element,
        });
        Some(dest)
    }

    /// `__param_list_address[*values]()`: the static address of the
    /// parameter list `values`' elements, a `Pointer[E, ImmStaticOrigin]`.
    fn param_list_address(
        &mut self,
        location: SourceSpan,
        values: mojito_types::param_expr::ParamExpr,
    ) -> Reg {
        let element = match values.meta() {
            mojito_types::param_expr::MetaTy::ParamList(element) => element.as_value().cloned(),
            _ => None,
        };
        let Some(element) = element else {
            let dest = self.fresh(location, None);
            self.emit(MirInstr::Unsupported(format!(
                "the parameter list `{values}` is no list of values"
            )));
            self.emit(MirInstr::Const {
                dest,
                k: Const::None,
            });
            return dest;
        };
        let dest = self.fresh_typed(
            location,
            None,
            TyKind::Pointer {
                element,
                origin: mojito_types::origin::PointerOrigin::Static,
            }
            .into(),
        );
        self.emit(MirInstr::ParamListAddress {
            dest,
            values: Const::Param(values),
        });
        dest
    }
}

/// The declared compile-time parameters of the parameterized method call
/// node `node` is (`recv.method[params](args)`), as the check recorded them.
fn parameterized_method_decls(node: &CheckedExpr) -> Option<&[ParamDecl]> {
    node.adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            mojito_checked::checked::SemanticAdjustment::ParameterizedMethodCall {
                param_decls,
            } => Some(&param_decls[..]),
            _ => None,
        })
}

/// The runtime indices of element invocation `call` (`objs[0](3)`): its
/// compile-time value arguments, which the checker re-dispatched as
/// subscript operands.
fn element_indices(call: Node<'_>) -> Vec<Node<'_>> {
    call.param_args()
        .iter()
        .filter_map(|argument| match argument {
            CheckedParamArg::Value(value) => call.operand_of(*value),
            _ => None,
        })
        .collect()
}
