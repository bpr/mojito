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
        self.emit(MirInstr::Call {
            dest,
            func: FuncRef::named(&contract.target),
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
        self.emit(MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            raises,
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
        let getter = Some(self.mir_subscript_call_contract(plan.getter, &operands));
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
            _ => self.args(args),
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

    /// Lower a call of a nested `def` by name. A body whose captures are
    /// all reference captures this frame holds is called directly, as the
    /// pin calls the lifted closure generator: a `call` of the lifted body
    /// passing the captured places as its leading reference arguments. A
    /// body with a copied or moved capture, or one whose captures this
    /// lifted frame does not hold (a sibling it reaches only through a
    /// captured closure value), is called through its closure value.
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
        self.emit(MirInstr::CallIndirect {
            dest: d,
            callee,
            // The checked owner already selects this exact lifted closure.
            // `resolved` is reserved for nominal/trait `__call__` dispatch;
            // attaching that abstract target here can disagree with an erased
            // variadic closure ABI even though execution never consults it.
            resolved: None,
            raises: call.raises(),
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
        d
    }

    /// Whether a call of `info` by name is a direct call of its lifted
    /// body: every capture is a reference capture, and the frame holds
    /// each captured place, its own local or a forwarded environment
    /// parameter.
    pub(in crate::mir) fn calls_nested_directly(&self, info: &NestedInfo) -> bool {
        info.captures.iter().all(|capture| {
            matches!(
                capture.kind,
                mojito_ast::ast::CaptureKind::Imm
                    | mojito_ast::ast::CaptureKind::Mut
                    | mojito_ast::ast::CaptureKind::Ref
            ) && (info.materialized_here || self.owner_vars.contains_key(&capture.binding))
        })
    }

    /// The direct call of the lifted body of `info`: its captured places,
    /// loaded in capture order, lead the explicit arguments and are
    /// retained as the call's places for the body's leading reference
    /// parameters.
    fn lower_direct_nested_call(
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
        let captured: Vec<(Reg, MirPlace)> = info
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
                (dest, place)
            })
            .collect();
        let d = self.fresh(call.location(), None);
        self.emit_call_invalidations(call, args, kwargs);
        let capture_accesses = self.call_capture_accesses(call);
        let transfer_arg_places = arg_places.clone();
        // A generic nested `def` called by name binds its own parameters
        // from the call's solved arguments, as a module `def` does.
        let instantiated_args = call
            .instantiated_callable_contract()
            .map_or_else(|| call.instantiated_args(), |(_, arguments)| arguments);
        let spread = Self::spread_position(args).map(|position| position + captured.len());
        // The captured places fill the lifted body's leading parameters,
        // which its declaration lists.
        self.record_call(d, Some(info.mangled.clone()), 0);
        let mut all_args: Vec<Reg> = captured.iter().map(|(reg, _)| *reg).collect();
        all_args.extend(arg_regs);
        let mut all_places: Vec<Option<MirPlace>> =
            captured.into_iter().map(|(_, place)| Some(place)).collect();
        all_places.extend(arg_places);
        self.emit(MirInstr::Call {
            dest: d,
            func: FuncRef::named(&info.mangled),
            raises: call.raises(),
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
        d
    }

    /// Keep the owners of the reference captures of `info` alive through
    /// a call of it.
    fn emit_capture_keepalives(&mut self, info: &NestedInfo) {
        let mut owners = Vec::new();
        let mut seen = HashSet::new();
        for capture in &info.captures {
            self.collect_capture_keepalives(capture, &mut owners, &mut seen);
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
        // those owners alive transitively; owned copy/move environment entries
        // are self-contained and deliberately stop the walk.
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

    pub(in crate::mir) fn emit_nested_closure(
        &mut self,
        info: &NestedInfo,
        at: SourceSpan,
        forward_existing_environment: bool,
    ) -> Reg {
        let captures = info
            .captures
            .iter()
            .map(|capture| MirClosureCapture {
                place: self.capture_place(capture),
                mode: if forward_existing_environment {
                    // In a lifted body these names are already references into
                    // the declaration-created environment. Recursion and calls
                    // to inherited siblings forward those handles; they must
                    // never repeat a copy/move capture operation.
                    MirCaptureMode::Reference
                } else {
                    match capture.kind {
                        mojito_ast::ast::CaptureKind::Copy => MirCaptureMode::Copy,
                        mojito_ast::ast::CaptureKind::Move => MirCaptureMode::Move,
                        mojito_ast::ast::CaptureKind::Imm
                        | mojito_ast::ast::CaptureKind::Mut
                        | mojito_ast::ast::CaptureKind::Ref => MirCaptureMode::Reference,
                    }
                },
                constructor: None,
            })
            .collect();
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at, None, ty.clone()),
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
        if !self.owner_vars.contains_key(&info.binding) {
            // A declaration without a slot makes its closure at each
            // first-class use. A lifted body has no direct access to an
            // outer frame's closure slot: its inherited callable is
            // reconstructed from the environment parameters forwarded into
            // this frame.
            return self.emit_nested_closure(info, at, !info.materialized_here);
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
            Const::Function(_) => function_ty,
            Const::Param(value) => value.meta().as_value().cloned(),
            Const::Value(value) => mojito_types::param_expr::MetaTy::of_value(value)
                .as_value()
                .cloned(),
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
}
