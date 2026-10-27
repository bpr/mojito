//! Indirect/element invocations, nested closures, keep-alives, and
//! register materialization.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedOp;
use mojito_types::types::TyKind;

impl<'a> Flatten<'a> {
    /// The argument position of a call's pack spread (`f(*a)`), the slot the
    /// elaborator expands into the collector's elements.
    pub(super) fn spread_position(args: &[Node<'a>]) -> Option<usize> {
        args.iter()
            .position(|argument| argument.spread_operand().is_some())
    }

    /// A `@staticmethod` reached through an instance (`value.static_method()`,
    /// `v.is_type_supported[Int]()`): the checker selected a target without a
    /// receiver, so evaluate the receiver for its effect only and call the
    /// static symbol with the arguments alone — exactly the type-receiver
    /// spelling's lowering, the instance's type naming the struct binders
    /// the static body reads. `None` when the call keeps its receiver.
    pub(super) fn lower_elided_receiver_call(
        &mut self,
        call: Node<'a>,
        object: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Option<Reg> {
        let contract = call.call_contract()?;
        if !contract.receiver_elided {
            return None;
        }
        let _ = self.lower_call_receiver(object);
        let receiver = Self::static_receiver(object);
        let (param_arg_regs, instantiated_args) = if contract.param_decls.is_empty() {
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
        self.emit_call_invalidations(call, args, kwargs);
        self.record_call(dest, Some(contract.target.clone()), 0);
        let raises = self.raises(call.raises(), call.location());
        self.emit_call(MirInstr::Call {
            dest,
            func: FuncRef::named(&contract.target),
            raises: raises.clone(),
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
        self.error_region(raises);
        Some(dest)
    }

    /// Emit the shared indirect-call tail: argument lowering, call-boundary
    /// invalidations, and the `CallIndirect` with its transfer replay. The
    /// callee register/place and the dispatch metadata are the caller's; when
    /// `call_site_invalidations` is false the call-span invalidation facts are
    /// the caller's responsibility (the element-call channel fires them before
    /// materializing its reference handle, so a generation-replacing getter
    /// retires the previous generation rather than the one it establishes).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_indirect_invocation(
        &mut self,
        call: Node<'a>,
        callee: Reg,
        callee_place: Option<MirPlace>,
        callable_ty: Option<&Ty>,
        resolved: Option<String>,
        raises: Option<Ty>,
        param_arg_regs: Vec<MirParamArg>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
        call_site_invalidations: bool,
    ) -> Reg {
        let param_decls = callable_ty
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let lends_reads = call.lends_read_arguments();
        self.open_call_setup();
        let (arg_regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        self.close_call_setup(Some(call));
        self.allow_argument_anchors = saved_anchor_permission;
        let dest = self.fresh(call.location(), None);
        if call_site_invalidations {
            self.emit_call_invalidations(call, args, kwargs);
        } else {
            for argument in args {
                self.emit_interior_invalidations(*argument, None);
            }
            for argument in kwargs {
                self.emit_interior_invalidations(argument.1, None);
            }
        }
        let capture_accesses = self.call_capture_accesses(call);
        let (instantiated_contract, instantiated_args) = call
            .instantiated_callable_contract()
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        let transfer_arg_places = arg_places.clone();
        let transfer_recv_place = callee_place.clone();
        self.record_call(dest, resolved.clone(), 0);
        let raises = self.raises(raises, call.location());
        self.emit_call(MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            raises: raises.clone(),
            args: arg_regs,
            kwargs: kw_regs,
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
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, transfer_recv_place.as_ref(), &transfer_arg_places);
        self.error_region(raises);
        dest
    }

    /// Lower the bare element-call spelling (`objs[0](3)`, `a.b[i](x)`,
    /// `grid[i, j](x)`), given its lowered receiver and the receiver's
    /// place: read the element through the checker-selected
    /// `__getitem__` contract, then dispatch the element value through the
    /// shared indirect-call emission. The receiver's call-span invalidations
    /// fire before any reference materialization so a generation-replacing
    /// getter (a Dict lookup) retires the previous generation, not the one
    /// this call establishes.
    pub(super) fn lower_element_invocation(
        &mut self,
        call: Node<'a>,
        (base, base_place): (Reg, Option<MirPlace>),
        plan: mojito_checked::checked::CheckedElementInvocation,
        indices: &[Node<'a>],
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        // The getter's reference designates storage inside the receiver, so
        // the receiver's root owns a loan the checked origin cannot name.
        let receiver_root = base_place.as_ref().map(|place| place.root);
        let mut index_regs = Vec::with_capacity(indices.len());
        let mut index_places = Vec::with_capacity(indices.len());
        for index in indices {
            let (register, place) = self.lower_call_argument(*index);
            index_regs.push(register);
            index_places.push(place);
        }
        for index in indices {
            self.emit_interior_invalidations(*index, None);
        }
        self.emit_interior_invalidations(call, None);
        let reference_result = plan.getter.reference_result.clone();
        let operands = index_regs
            .iter()
            .map(|register| MirSubscriptArg::Index(*register))
            .collect::<Vec<_>>();
        let getter =
            Some(self.mir_subscript_call_contract(plan.getter, &operands, call.location()));
        let element_ty = getter
            .as_ref()
            .map(|contract| contract.result_ty.clone())
            .expect("element-call plan carries the getter contract");
        let element = self.fresh_typed(call.location(), None, element_ty);
        self.record_call(
            element,
            getter.as_ref().map(|contract| contract.target.clone()),
            0,
        );
        if let [index] = index_regs.as_slice() {
            self.emit(MirInstr::Index {
                dest: element,
                base,
                index: *index,
                base_place: base_place.map(Box::new),
                index_place: index_places.pop().flatten().map(Box::new),
                call: getter.map(Box::new),
                intrinsic: None,
            });
        } else {
            self.emit(MirInstr::MultiIndex {
                dest: element,
                object: base,
                args: index_regs.into_iter().map(MirSubscriptArg::Index).collect(),
                object_place: base_place.map(Box::new),
                arg_places: index_places,
                kwargs: Vec::new(),
                kwarg_places: Vec::new(),
                call: getter.map(Box::new),
            });
        }
        let (callee, callee_place) = match reference_result {
            Some(reference) => {
                let place = self.materialize_call_reference_place(
                    call,
                    element,
                    &reference,
                    receiver_root,
                    false,
                );
                let value = self.fresh_typed(
                    call.location(),
                    Some(place.root),
                    place.ty.clone().unwrap_or_else(Ty::error),
                );
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                (value, Some(place))
            }
            None => (element, None),
        };
        self.emit_indirect_invocation(
            call,
            callee,
            callee_place,
            Some(&plan.callable),
            plan.target,
            plan.raises,
            Vec::new(),
            args,
            kwargs,
            false,
        )
    }

    /// If `name(...)` is a SIMD construction — `SIMD[DType.<dt>, width](elems)` or
    /// a scalar alias (`Int32(x)`, `Float32(x)`, …) — resolve its dtype/width and
    /// emit a [`MirInstr::MakeSimd`], returning its result register. Otherwise
    /// `None`, and the caller lowers it as an ordinary call.
    pub(in crate::mir) fn try_simd_call(
        &mut self,
        call: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Option<Reg> {
        let (dtype, width) = call
            .adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructSimd { dtype, width } => {
                    Some((dtype.clone(), width.clone()))
                }
                _ => None,
            })?;
        // `SIMD[DType.bool, N](fill=b)` is the one-lane splat, by keyword.
        // A nullary construction splats a zero lane.
        let elems = match (kwargs, args) {
            ([(_, fill)], _) => vec![self.expr(*fill)],
            ([], []) if dtype.known() == Some(Dtype::Bool) => {
                vec![self.constant(call, Const::Bool(false))]
            }
            ([], []) => vec![self.constant(call, Const::IntLiteral(0.into()))],
            _ => args.iter().map(|arg| self.read_operand(*arg)).collect(),
        };
        let d = self.fresh(call.location(), None);
        self.emit(MirInstr::MakeSimd {
            dest: d,
            dtype,
            width,
            elems,
        });
        Some(d)
    }

    /// Lower a call of a nested `def` by name. A body whose storage this
    /// frame holds, or whose captures are all places this frame holds, is
    /// called directly, as the pin calls the lifted closure method: a
    /// `call` of the lifted body passing the storage, or the captured
    /// places as its leading reference arguments. Any other body is called
    /// through its closure value.
    pub(in crate::mir) fn lower_nested_call(
        &mut self,
        call: Node<'a>,
        name: &str,
        info: &NestedInfo,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        if self.calls_nested_directly(info) {
            return self.lower_direct_nested_call(call, info, args, kwargs);
        }
        let callee = self.load_nested_closure(name, info, call.location());
        let callable_ty = info
            .callable_ty
            .clone()
            .or_else(|| self.f.reg_types.get(&callee.0).cloned());
        let param_arg_regs = self.param_arg_regs(call);
        let param_decls = callable_ty
            .as_ref()
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let lends_reads = call.lends_read_arguments();
        self.open_call_setup();
        let (arg_regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        self.close_call_setup(Some(call));
        self.allow_argument_anchors = saved_anchor_permission;
        let d = self.fresh(call.location(), None);
        self.emit_call_invalidations(call, args, kwargs);
        let transfer_arg_places = arg_places.clone();
        let callee_place = self
            .owner_vars
            .contains_key(&info.binding)
            .then(|| self.binding_place(info.binding, name));
        let capture_accesses = self.call_capture_accesses(call);
        // A generic nested `def` called by name binds its own parameters
        // from the call's solved arguments, as a module `def` does: the call
        // carries them with the contract they instantiate.
        let (instantiated_contract, instantiated_args) = call
            .instantiated_callable_contract()
            .or_else(|| {
                let arguments = call.instantiated_args();
                let contract = callable_ty.as_ref()?;
                (!arguments.is_empty() && matches!(contract.kind(), TyKind::GenericFunc(_)))
                    .then(|| {
                        crate::mir::verify::instantiate_generic_callable_contract(
                            contract, &arguments,
                        )
                        .ok()
                    })
                    .flatten()
                    .map(|contract| (contract, arguments))
            })
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        self.record_call(d, Some(info.mangled.clone()), info.captures.len());
        let raises = self.raises(call.raises(), call.location());
        self.emit_call(MirInstr::CallIndirect {
            dest: d,
            callee,
            // The checked owner already selects this exact lifted closure.
            // `resolved` is reserved for nominal/trait `__call__` dispatch;
            // attaching that abstract target here can disagree with an erased
            // variadic closure ABI even though execution never consults it.
            resolved: None,
            raises: raises.clone(),
            args: arg_regs,
            kwargs: kw_regs,
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
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, None, &transfer_arg_places);
        self.emit_capture_keepalives(info);
        self.error_region(raises);
        d
    }

    /// Whether a call of `info` by name is a direct call of its lifted
    /// body: the frame holds its storage ([`NestedInfo::storage`]), its
    /// declaration's slot or a sibling's capture of it, or the body has no
    /// storage and the frame holds each captured place, its own local or
    /// a forwarded reference parameter.
    pub(in crate::mir) fn calls_nested_directly(&self, info: &NestedInfo) -> bool {
        match &info.storage {
            Some(_) => self.owner_vars.contains_key(&info.binding),
            None => info.captures.iter().all(|capture| match capture.kind {
                mojito_ast::ast::CaptureKind::Imm
                | mojito_ast::ast::CaptureKind::Mut
                | mojito_ast::ast::CaptureKind::Ref => {
                    info.materialized_here || self.owner_vars.contains_key(&capture.binding)
                }
                mojito_ast::ast::CaptureKind::Copy | mojito_ast::ast::CaptureKind::Move => false,
            }),
        }
    }

    /// The place of the storage of `info` this frame holds: the
    /// declaration's slot, or a capture of it holding a reference to it;
    /// `None` when the frame holds no storage of it.
    pub(in crate::mir) fn storage_place(&mut self, info: &NestedInfo) -> Option<MirPlace> {
        let storage = info.storage.as_ref()?;
        self.owner_vars.contains_key(&info.binding).then(|| {
            self.capture_place(&NestedCapture {
                name: info.source_name.clone(),
                binding: info.binding,
                ty: storage.ty.clone(),
                kind: mojito_ast::ast::CaptureKind::Imm,
            })
        })
    }

    /// The direct call of the lifted body of `info`, called by name or
    /// invoked where its lambda stands: the storage this frame holds, as
    /// upstream passes a closure's `self`, retained as the call's place
    /// when it is memory-only (by address), else its captured places,
    /// loaded in capture order and retained as the call's places for the
    /// body's leading reference parameters, lead the explicit arguments.
    pub(in crate::mir) fn lower_direct_nested_call(
        &mut self,
        call: Node<'a>,
        info: &NestedInfo,
        args: &[Node<'a>],
        kwargs: &[(&'a str, Node<'a>)],
    ) -> Reg {
        let param_arg_regs = self.param_arg_regs(call);
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = call.call_anchors_arguments();
        let lends_reads = call.lends_read_arguments();
        self.open_call_setup();
        let (arg_regs, arg_places) = self.lower_call_arguments(args, lends_reads);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs, lends_reads);
        self.close_call_setup(Some(call));
        self.allow_argument_anchors = saved_anchor_permission;
        let captured: Vec<(Reg, Option<MirPlace>)> = match self.storage_place(info) {
            Some(place) => {
                let storage = info
                    .storage
                    .as_ref()
                    .expect("a storage place has a storage");
                let dest = self.fresh_typed(call.location(), Some(place.root), storage.ty.clone());
                self.emit(MirInstr::LoadPlace {
                    dest,
                    place: place.clone(),
                });
                vec![(dest, (!storage.register_passable).then_some(place))]
            }
            None => info
                .captures
                .iter()
                .map(|capture| {
                    let place = self.capture_place(capture);
                    let ty = place.ty.clone().unwrap_or_else(|| capture.ty.clone());
                    let dest = self.fresh_typed(call.location(), Some(place.root), ty);
                    self.emit(MirInstr::LoadPlace {
                        dest,
                        place: place.clone(),
                    });
                    (dest, Some(place))
                })
                .collect(),
        };
        let d = self.fresh(call.location(), None);
        self.emit_call_invalidations(call, args, kwargs);
        let capture_accesses = self.call_capture_accesses(call);
        let transfer_arg_places = arg_places.clone();
        // A generic nested `def` called by name binds its own parameters
        // from the call's solved arguments, as a module `def` does.
        let instantiated_args = call
            .instantiated_callable_contract()
            .map_or_else(|| call.instantiated_args(), |(_, arguments)| arguments);
        let raises = self.raises(call.raises(), call.location());
        // A body that is its storage's method is called on the storage,
        // as upstream calls `fn::__storage::fn`, binding its own
        // parameters from the call's solved arguments.
        if info
            .storage
            .as_ref()
            .is_some_and(|storage| storage.method.is_some())
            && let [(recv, recv_place)] = captured.as_slice()
        {
            self.record_call(d, Some(info.mangled.clone()), 0);
            self.emit_call(MirInstr::MethodCall {
                dest: d,
                recv: *recv,
                method: info.source_name.clone(),
                resolved: Some(info.mangled.clone()),
                raises: raises.clone(),
                reference_result: call.reference_result().map(Box::new),
                result_adapter: None,
                args: arg_regs,
                kwargs: kw_regs,
                recv_place: recv_place.clone().map(Box::new),
                // Only a body owning a moved capture writes its storage.
                recv_writes: info
                    .captures
                    .iter()
                    .any(|capture| capture.kind == mojito_ast::ast::CaptureKind::Move),
                receiver_convention: None,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls: info
                    .callable_ty
                    .as_ref()
                    .map(generic_callable_param_decls)
                    .unwrap_or_default(),
                instantiated_args,
                spread: Self::spread_position(args),
                refined_bound: None,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            self.install_call_transfers(call, None, &transfer_arg_places);
            self.emit_capture_keepalives(info);
            self.error_region(raises);
            return d;
        }
        let spread = Self::spread_position(args).map(|position| position + captured.len());
        // The captured places fill the lifted body's leading parameters,
        // which its declaration lists.
        self.record_call(d, Some(info.mangled.clone()), 0);
        let mut all_args: Vec<Reg> = captured.iter().map(|(reg, _)| *reg).collect();
        all_args.extend(arg_regs);
        let mut all_places: Vec<Option<MirPlace>> =
            captured.into_iter().map(|(_, place)| place).collect();
        all_places.extend(arg_places);
        self.emit_call(MirInstr::Call {
            dest: d,
            func: FuncRef::named(&info.mangled),
            raises: raises.clone(),
            args: all_args,
            kwargs: kw_regs,
            arg_places: all_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            receiver: None,
            instantiated_args,
            spread,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(call, None, &transfer_arg_places);
        self.emit_capture_keepalives(info);
        self.error_region(raises);
        d
    }

    /// Keep the owners of the reference captures of `info`, and the
    /// storage slot holding its copied or moved ones, alive through a call
    /// of it.
    fn emit_capture_keepalives(&mut self, info: &NestedInfo) {
        let mut owners = Vec::new();
        let mut seen = HashSet::new();
        let storage = info
            .storage
            .as_ref()
            .and_then(|_| self.owner_vars.get(&info.binding).copied());
        owners.extend(storage);
        for capture in &info.captures {
            if !matches!(
                capture.kind,
                mojito_ast::ast::CaptureKind::Copy | mojito_ast::ast::CaptureKind::Move
            ) {
                self.collect_capture_keepalives(capture, &mut owners, &mut seen);
            }
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
    }

    pub(in crate::mir) fn collect_capture_keepalives(
        &self,
        capture: &NestedCapture,
        owners: &mut Vec<VarId>,
        seen: &mut HashSet<mojito_types::origin::OwnerId>,
    ) {
        if capture.kind == mojito_ast::ast::CaptureKind::Move || !seen.insert(capture.binding) {
            return;
        }
        if let Some(var) = self.owner_vars.get(&capture.binding).copied()
            && !owners.contains(&var)
        {
            owners.push(var);
        }
        // A captured closure slot can itself retain reference captures. Keep
        // those owners alive transitively; copy/move captures live in the
        // closure's storage and deliberately stop the walk.
        if let Some(callable) = self.nested.get(&capture.binding) {
            for nested in &callable.captures {
                if matches!(
                    nested.kind,
                    mojito_ast::ast::CaptureKind::Imm
                        | mojito_ast::ast::CaptureKind::Mut
                        | mojito_ast::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(nested, owners, seen);
                }
            }
        }
    }

    /// A capture-bearing nested callable passed to another non-escaping call
    /// can leave its environment handle in an SSA register. Keep the referenced
    /// owner storage alive through that consuming call without creating a
    /// persistent access loan (Mojo permits intervening owner mutation).
    pub(in crate::mir) fn emit_nested_closure_argument_keepalives(
        &mut self,
        args: &[Node<'a>],
        kwargs: &[(&str, Node<'a>)],
    ) {
        let mut owners = Vec::new();
        for &expression in args.iter().chain(kwargs.iter().map(|(_, value)| value)) {
            let expression = expression.named_value().unwrap_or(expression);
            // A closure argument is either a binding naming a nested def or a
            // lambda expression materialized in place.
            let (CheckedOp::Identifier(_) | CheckedOp::Lambda { .. }) = expression.op() else {
                continue;
            };
            let Some(info) = self.nested_info(expression) else {
                continue;
            };
            let mut seen = HashSet::new();
            let callable_capture = NestedCapture {
                name: info.source_name.clone(),
                binding: info.binding,
                ty: info.callable_ty.clone().unwrap_or_else(|| {
                    TyKind::Param {
                        binder: mojito_types::param_expr::ParamRef {
                            id: mojito_types::param_expr::ParamId::new("$capture", 0),
                            name: "$capture".into(),
                        },
                        bounds: Vec::new(),
                        callable_bound: None,
                    }
                    .into()
                }),
                kind: mojito_ast::ast::CaptureKind::Imm,
            };
            self.collect_capture_keepalives(&callable_capture, &mut owners, &mut seen);
            for capture in info.captures {
                if matches!(
                    capture.kind,
                    mojito_ast::ast::CaptureKind::Imm
                        | mojito_ast::ast::CaptureKind::Mut
                        | mojito_ast::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(&capture, &mut owners, &mut seen);
                }
            }
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
    }

    /// The value of the capture-list `def` `info`, read by name at `node`
    /// where it is not converted to a closure
    /// ([`Node::closure_callable`]): its storage struct, read from the
    /// declaration's slot as any local is, as upstream's `emitClosure`
    /// returns the storage. A body that is no method of its storage (one
    /// naming an enclosing callable binder) has no value (R92).
    pub(in crate::mir) fn closure_storage_read(
        &mut self,
        node: Node<'a>,
        name: &str,
        info: &NestedInfo,
    ) -> Reg {
        if info
            .storage
            .as_ref()
            .is_none_or(|storage| storage.method.is_none())
        {
            return self.unsupported_value(
                node.location(),
                "a closure naming an enclosing callable parameter is not a value (R92)",
            );
        }
        match self.owner_vars.get(&info.binding).copied() {
            Some(var) => self.read_slot(var, node.location(), Some(node.checked)),
            None => self.unsupported_value(
                node.location(),
                &format!("closure `{name}` is used where no frame holds its storage"),
            ),
        }
    }

    /// The value of the capturing lambda `info` at `at` where it is not
    /// converted to a closure: its storage struct, built here, once per
    /// evaluation, as upstream's `resolveAnonymousClosure` emits it where
    /// the lambda stands.
    pub(in crate::mir) fn lambda_storage_value(
        &mut self,
        info: &NestedInfo,
        at: SourceSpan,
    ) -> Reg {
        let storage = info
            .storage
            .as_ref()
            .expect("a lambda's storage value has a storage");
        if storage.method.is_none() {
            return self.unsupported_value(
                at,
                "a closure naming an enclosing callable parameter is not a value (R92)",
            );
        }
        self.build_closure_storage(info, storage, &at)
    }

    /// The closure value of the lambda `info` at `at`: a capturing
    /// lambda builds its storage here, once per evaluation, and moves it
    /// into a closure of its lifted body, whose `self` it is; a lambda
    /// without storage makes the closure of its captured places.
    pub(in crate::mir) fn lambda_closure(&mut self, info: &NestedInfo, at: SourceSpan) -> Reg {
        let Some(storage) = &info.storage else {
            return self.emit_nested_closure(info, at);
        };
        let slot = self.lambda_storage_slot(info, storage, &at);
        let dest = match Self::closure_value_ty(info) {
            Some(ty) => self.fresh_typed(at, None, ty),
            None => self.fresh(at, None),
        };
        self.emit(MirInstr::MakeClosure {
            dest,
            function: info.mangled.clone(),
            captures: vec![MirClosureCapture {
                place: MirPlace::root(slot, Some(storage.ty.clone())),
                mode: MirCaptureMode::Move,
                constructor: None,
                destructor: None,
            }],
        });
        dest
    }

    /// The closure of the body `info` without storage at `at` (a thin
    /// lambda, or a `@parameter` closure or capture-free `def` used as a
    /// value): each capture is a reference to its place, which a
    /// `@parameter` closure's body takes as a leading reference parameter.
    pub(in crate::mir) fn emit_nested_closure(&mut self, info: &NestedInfo, at: SourceSpan) -> Reg {
        let captures = info
            .captures
            .iter()
            .map(|capture| MirClosureCapture {
                place: self.capture_place(capture),
                mode: MirCaptureMode::Reference,
                constructor: None,
                destructor: None,
            })
            .collect();
        let dest = match Self::closure_value_ty(info) {
            Some(ty) => self.fresh_typed(at, None, ty),
            None => self.fresh(at, None),
        };
        self.emit(MirInstr::MakeClosure {
            dest,
            function: info.mangled.clone(),
            captures,
        });
        dest
    }

    pub(in crate::mir) fn load_nested_closure(
        &mut self,
        name: &str,
        info: &NestedInfo,
        at: SourceSpan,
    ) -> Reg {
        // A body with storage is the closure of its lifted body over that
        // storage, captured by reference. The closure is a temporary of the
        // statement that takes it, so the storage stays live to the
        // statement's end.
        if let Some(place) = self.storage_place(info) {
            if !self.pending_argument_anchors.contains(&place.root) {
                self.pending_argument_anchors.push(place.root);
            }
            let dest = match Self::closure_value_ty(info) {
                Some(ty) => self.fresh_typed(at, None, ty),
                None => self.fresh(at, None),
            };
            self.emit(MirInstr::MakeClosure {
                dest,
                function: info.mangled.clone(),
                captures: vec![MirClosureCapture {
                    place,
                    mode: MirCaptureMode::Reference,
                    constructor: None,
                    destructor: None,
                }],
            });
            return dest;
        }
        if !self.owner_vars.contains_key(&info.binding) {
            // A declaration without a slot makes its closure at each
            // first-class use. A lifted body has no direct access to an
            // outer frame's closure slot: its inherited callable is
            // reconstructed from the environment parameters forwarded into
            // this frame.
            return self.emit_nested_closure(info, at);
        }
        let var = self.binding_var(info.binding, name);
        if let Some(ty) = &info.callable_ty {
            self.var_types.entry(var).or_insert_with(|| ty.clone());
        }
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at.clone(), Some(var), ty.clone()),
            None => self.fresh(at.clone(), Some(var)),
        };
        if let Some(loan) = self.aliases.get(&var).cloned() {
            let mut place = loan.place;
            place.through = Some(var);
            self.emit(MirInstr::LoadPlace { dest, place });
        } else if self.runtime_aliases.contains(&var) {
            let handle = self.fresh(at, Some(var));
            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
            place.through = Some(var);
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place,
            });
            self.emit(MirInstr::ReadRef {
                dest,
                reference: handle,
            });
        } else {
            self.emit(MirInstr::UseVar {
                dest,
                var,
                // Calling a closure borrows its declaration-created environment;
                // neither loading it for a call nor a repeated call consumes or
                // duplicates that environment.
                mode: UseMode::BorrowShared,
                constructor: None,
            });
        }
        dest
    }

    /// A nullary scalar built-in construction (`Int()`, `UInt()`,
    /// `Float64()`, `Bool()`) is its zero; `None` for any other expression.
    /// The `UInt` zero is the literal `0` materialized, as upstream's
    /// `SIMD.__init__()` is `SIMD(0)`.
    pub(in crate::mir) fn default_scalar_construction(
        &mut self,
        node: &mojito_checked::checked::CheckedExpr,
    ) -> Option<Reg> {
        let mojito_checked::checked::CheckedOp::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &node.op
        else {
            return None;
        };
        if !(param_args.is_empty() && args.is_empty() && kwargs.is_empty()) {
            return None;
        }
        let zero = match (name.as_str(), node.ty.as_ref()?.kind()) {
            ("Int", TyKind::Int) => Const::Int(0),
            ("UInt", TyKind::UInt) => {
                let value = self.literal(node, Const::IntLiteral(0.into()));
                return Some(self.materialize_literal(value, &Ty::uint(), node.location()));
            }
            ("Float64", TyKind::Float64) => Const::Float(0.0),
            ("Bool", TyKind::Bool) => Const::Bool(false),
            _ => return None,
        };
        Some(self.literal(node, zero))
    }

    /// Emit a `Const` writing a fresh register.
    pub(in crate::mir) fn constant(&mut self, e: Node<'a>, k: Const) -> Reg {
        let function_ty = e.ty();
        self.constant_at(e.location(), function_ty, k)
    }

    /// Emit the `Const` literal node `node` denotes.
    pub(in crate::mir) fn literal(
        &mut self,
        node: &mojito_checked::checked::CheckedExpr,
        k: Const,
    ) -> Reg {
        self.constant_at(node.location(), node.ty.clone(), k)
    }

    /// Emit a `Const` writing a fresh register at `location`; a function
    /// constant takes `function_ty`, its expression's checked type.
    pub(in crate::mir) fn constant_at(
        &mut self,
        location: SourceSpan,
        function_ty: Option<Ty>,
        k: Const,
    ) -> Reg {
        let constant_ty = match &k {
            Const::Int(_) => Some(Ty::int()),
            Const::Float(_) => Some(Ty::float64()),
            Const::IntLiteral(_) => Some(Ty::int_literal()),
            Const::FloatLiteral(_) => Some(Ty::float_literal()),
            Const::Bool(_) => Some(Ty::bool()),
            Const::Str(_) => Some(Ty::string_literal()),
            Const::None => Some(Ty::none()),
            Const::Dtype(_) => Some(Ty::dtype()),
            Const::Function(_) | Const::FunctionInstance { .. } => function_ty,
            Const::Param(value) => value.meta().as_value().cloned(),
            Const::Value(value) => mojito_types::param_expr::MetaTy::of_value(value)
                .as_value()
                .cloned(),
            Const::UninitMem => None,
        };
        let d = match constant_ty {
            Some(ty) => self.fresh_typed(location, None, ty),
            None => self.fresh(location, None),
        };
        self.emit(MirInstr::Const { dest: d, k });
        d
    }

    pub(in crate::mir) fn materialize_register(
        &mut self,
        value: Reg,
        target: &Ty,
        source: SourceSpan,
    ) -> Reg {
        let Some(found) = self.f.reg_types.get(&value.0) else {
            return value;
        };
        let compatible = crate::mir::calls::literal_materializes_to(found, target);
        if !compatible {
            return value;
        }
        self.materialize_literal(value, target, source)
    }

    /// Materialize an exact-literal register as the checked `target`: a
    /// numeric scalar through `MaterializeLiteral`, a multi-lane vector as the
    /// one-element splat explicit `SIMD[dt, w](literal)` construction emits.
    /// A vector whose width is still a parameter expression splats too: the
    /// instance's width may be above one.
    pub(in crate::mir) fn materialize_literal(
        &mut self,
        value: Reg,
        target: &Ty,
        source: SourceSpan,
    ) -> Reg {
        if mojito_types::types::simd_slots(target)
            .is_some_and(|(_, width)| width.known().is_none_or(|width| width > 1))
        {
            return self.splat_scalar(value, target, source);
        }
        let dest = self.fresh_typed(source, None, target.clone());
        self.emit(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
        dest
    }

    /// Splat a one-lane register across the checked multi-lane `target`, as
    /// the one-element explicit `SIMD[dt, w](x)` construction does.
    pub(in crate::mir) fn splat_scalar(
        &mut self,
        value: Reg,
        target: &Ty,
        source: SourceSpan,
    ) -> Reg {
        let Some((dtype, width)) = mojito_types::types::simd_slots(target) else {
            return value;
        };
        let dest = self.fresh_typed(source, None, target.clone());
        self.emit(MirInstr::MakeSimd {
            dest,
            dtype,
            width,
            elems: vec![value],
        });
        dest
    }

    /// The type of the closure of the body `info` as a function value: its
    /// checked type, a storage value's environment read as the storage's
    /// captures.
    fn closure_value_ty(info: &NestedInfo) -> Option<Ty> {
        let mut ty = info.callable_ty.clone()?;
        ty.update(|kind| {
            if let Some(func) = kind.callable_mut()
                && let mojito_types::origin::CallableEnvironment::Storage(storage) =
                    &func.environment
            {
                func.environment =
                    mojito_types::origin::CallableEnvironment::Capturing(storage.captures.clone());
            }
        });
        Some(ty)
    }
}
