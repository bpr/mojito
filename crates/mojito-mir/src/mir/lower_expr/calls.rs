//! Indirect/element invocations, nested closures, keep-alives, and
//! register materialization.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Flatten<'_> {
    /// A `@staticmethod` reached through an instance (`value.static_method()`,
    /// `v.is_type_supported[Int]()`): the checker selected a target without a
    /// receiver, so evaluate the receiver for its effect only and call the
    /// static symbol with the arguments alone — exactly the type-receiver
    /// spelling's lowering. `None` when the call keeps its receiver.
    pub(super) fn lower_elided_receiver_call(
        &mut self,
        e: &Expr,
        object: &Expr,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Reg> {
        let contract = self.checked_call_contract(e)?;
        if !contract.receiver_elided {
            return None;
        }
        let _ = self.lower_call_receiver(object);
        let (param_arg_regs, instantiated_args) = if contract.param_decls.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let mut param_arg_regs = self.param_arg_regs(param_args, &span(e));
            param_arg_regs.extend(self.inferred_param_arg_regs(e));
            (param_arg_regs, self.instantiated_args(e))
        };
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = self.call_anchors_arguments(e);
        let (regs, arg_places) = self.lower_call_arguments(args, false);
        self.allow_argument_anchors = saved_anchor_permission;
        let (kw, kwarg_places) = self.lower_call_keywords(kwargs, false);
        let dest = self.fresh(span(e), None);
        self.emit_call_invalidations(e, args, kwargs);
        self.emit(MirInstr::Call {
            dest,
            func: FuncRef::named(&contract.target),
            raises: self.checked_raises(e),
            args: regs,
            kwargs: kw,
            arg_places,
            kwarg_places,
            capture_accesses: self.checked_call_capture_accesses(e),
            param_arg_regs,
            receiver: None,
            instantiated_args,
            spread: None,
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
        e: &Expr,
        callee: Reg,
        callee_place: Option<MirPlace>,
        callable_ty: Option<&Ty>,
        resolved: Option<String>,
        raises: Option<Ty>,
        param_arg_regs: Vec<MirParamArg>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        call_site_invalidations: bool,
    ) -> Reg {
        let param_decls = callable_ty
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = self.call_anchors_arguments(e);
        let (arg_regs, arg_places) = self.lower_call_arguments(args, false);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs, false);
        self.allow_argument_anchors = saved_anchor_permission;
        let dest = self.fresh(span(e), None);
        if call_site_invalidations {
            self.emit_call_invalidations(e, args, kwargs);
        } else {
            for argument in args {
                self.emit_interior_invalidations(argument, None);
            }
            for argument in kwargs {
                self.emit_interior_invalidations(&argument.value, None);
            }
        }
        let capture_accesses = self.checked_call_capture_accesses(e);
        let (instantiated_contract, instantiated_args) = self
            .instantiated_callable_contract(e)
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        let transfer_arg_places = arg_places.clone();
        let transfer_recv_place = callee_place.clone();
        self.emit(MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            raises,
            args: arg_regs,
            kwargs: kw_regs,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(e, transfer_recv_place.as_ref(), &transfer_arg_places);
        dest
    }

    /// Lower the bare element-call spelling (`objs[0](3)`, `a.b[i](x)`,
    /// `grid[i, j](x)`): read the element through the checker-selected
    /// `__getitem__` contract, then dispatch the element value through the
    /// shared indirect-call emission. The receiver's call-span invalidations
    /// fire before any reference materialization so a generation-replacing
    /// getter (a Dict lookup) retires the previous generation, not the one
    /// this call establishes.
    pub(super) fn lower_element_invocation(
        &mut self,
        e: &Expr,
        receiver: &Expr,
        plan: mojito_checked::checked::CheckedElementInvocation,
        // Original checked-tree nodes: MIR fact lookup is pointer-keyed, so a
        // cloned index subtree would lower without its recorded facts (caller
        // places, conversions, invalidations).
        indices: &[&Expr],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Reg {
        let (base, base_place) = self.lower_call_receiver(receiver);
        // The getter's reference designates storage inside the receiver, so
        // the receiver's root owns a loan the checked origin cannot name.
        let receiver_root = base_place.as_ref().map(|place| place.root);
        let mut index_regs = Vec::with_capacity(indices.len());
        let mut index_places = Vec::with_capacity(indices.len());
        let mut sources = Vec::with_capacity(indices.len());
        for index in indices {
            let (register, place) = self.lower_call_argument(index);
            sources.push((index.source_span(), register));
            index_regs.push(register);
            index_places.push(place);
        }
        for index in indices {
            self.emit_interior_invalidations(index, None);
        }
        self.emit_interior_invalidations(e, None);
        let reference_result = plan.getter.reference_result.clone();
        let call = Some(self.mir_subscript_call_contract(plan.getter, &sources));
        let element_ty = call
            .as_ref()
            .map(|contract| contract.result_ty.clone())
            .expect("element-call plan carries the getter contract");
        let element = self.fresh_typed(e.source_span(), None, element_ty);
        if let [index] = index_regs.as_slice() {
            self.emit(MirInstr::Index {
                dest: element,
                base,
                index: *index,
                base_place,
                index_place: index_places.pop().flatten(),
                call,
                intrinsic: None,
            });
        } else {
            self.emit(MirInstr::MultiIndex {
                dest: element,
                object: base,
                args: index_regs.into_iter().map(MirSubscriptArg::Index).collect(),
                object_place: base_place,
                arg_places: index_places,
                kwargs: Vec::new(),
                kwarg_places: Vec::new(),
                call,
            });
        }
        let (callee, callee_place) = match reference_result {
            Some(reference) => {
                let place =
                    self.materialize_call_reference_place(e, element, &reference, receiver_root);
                let value = self.fresh_typed(
                    e.source_span(),
                    Some(place.root),
                    place.ty.clone().unwrap_or(Ty::Error),
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
            e,
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
        e: &Expr,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Reg> {
        let (dtype, width) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructSimd { dtype, width } => {
                    Some((dtype, width))
                }
                _ => None,
            })?;
        // `SIMD[DType.bool, N](fill=b)` is the one-lane splat, by keyword.
        // A nullary construction splats a zero lane.
        let elems = match (kwargs, args) {
            ([fill], _) => vec![self.expr(&fill.value)],
            ([], []) if dtype.known() == Some(Dtype::Bool) => {
                vec![self.constant(e, Const::Bool(false))]
            }
            ([], []) => vec![self.constant(e, Const::IntLiteral(0.into()))],
            _ => self.args(args),
        };
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::MakeSimd {
            dest: d,
            dtype,
            width,
            elems,
        });
        Some(d)
    }

    /// Lower a call to a nested `def` through the same closure-environment path as
    /// a first-class closure value. This preserves reference handles across sibling
    /// calls and recursion; it does not rely on call-return write-back.
    pub(in crate::mir) fn lower_nested_call(
        &mut self,
        e: &Expr,
        info: &NestedInfo,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Reg {
        let name = match &e.kind {
            ExprKind::Call { name, .. } => name.as_str(),
            _ => unreachable!("nested direct call has call syntax"),
        };
        let callee = self.load_nested_closure(name, info, span(e));
        let callable_ty = info
            .callable_ty
            .clone()
            .or_else(|| self.f.reg_types.get(&callee.0).cloned());
        let mut param_arg_regs = self.param_arg_regs(param_args, &span(e));
        param_arg_regs.extend(self.inferred_param_arg_regs(e));
        let param_decls = callable_ty
            .as_ref()
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = self.call_anchors_arguments(e);
        let (arg_regs, arg_places) = self.lower_call_arguments(args, false);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs, false);
        self.allow_argument_anchors = saved_anchor_permission;
        let d = self.fresh(span(e), None);
        self.emit_call_invalidations(e, args, kwargs);
        let transfer_arg_places = arg_places.clone();
        let callee_place = self
            .owner_vars
            .contains_key(&info.binding)
            .then(|| self.binding_place(info.binding, name));
        let capture_accesses = self.checked_call_capture_accesses(e);
        let (instantiated_contract, instantiated_args) = self
            .instantiated_callable_contract(e)
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        self.emit(MirInstr::CallIndirect {
            dest: d,
            callee,
            // The checked owner already selects this exact lifted closure.
            // `resolved` is reserved for nominal/trait `__call__` dispatch;
            // attaching that abstract target here can disagree with an erased
            // variadic closure ABI even though execution never consults it.
            resolved: None,
            raises: self.checked_raises(e),
            args: arg_regs,
            kwargs: kw_regs,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(e, None, &transfer_arg_places);
        let mut owners = Vec::new();
        let mut seen = HashSet::new();
        for capture in &info.captures {
            self.collect_capture_keepalives(capture, &mut owners, &mut seen);
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
        d
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
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) {
        let mut owners = Vec::new();
        for expression in args
            .iter()
            .chain(kwargs.iter().map(|argument| &argument.value))
        {
            let expression = match &expression.kind {
                ExprKind::Named { value, .. } => value.as_ref(),
                _ => expression,
            };
            // A closure argument is either a binding naming a nested def or a
            // lambda expression materialized in place.
            let (ExprKind::Identifier(_) | ExprKind::Lambda { .. }) = &expression.kind else {
                continue;
            };
            let Some(info) = self.nested_info(expression) else {
                continue;
            };
            let mut seen = HashSet::new();
            let callable_capture = NestedCapture {
                name: info.source_name.clone(),
                binding: info.binding,
                ty: info.callable_ty.clone().unwrap_or_else(|| Ty::Param {
                    binder: mojito_types::param_expr::ParamRef {
                        id: mojito_types::param_expr::ParamId::new("$capture", 0),
                        name: "$capture".into(),
                    },
                    bounds: Vec::new(),
                    callable_bound: None,
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
        if !info.materialized_here && !self.owner_vars.contains_key(&info.binding) {
            // A lifted body has no direct access to an outer frame's closure slot.
            // Its inherited/self callable is reconstructed from the environment
            // parameters forwarded into this frame; direct declarations never use
            // this path after their statement has materialized them.
            return self.emit_nested_closure(info, at, true);
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
            });
        }
        dest
    }

    /// A nullary scalar built-in construction (`Int()`, `UInt()`,
    /// `Float64()`, `Bool()`) is its zero; `None` for any other expression.
    /// `UInt` has no constant of its own, so its zero converts an `Int` one.
    pub(in crate::mir) fn default_scalar_construction(&mut self, e: &Expr) -> Option<Reg> {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &e.kind
        else {
            return None;
        };
        if !(param_args.is_empty() && args.is_empty() && kwargs.is_empty()) {
            return None;
        }
        let zero = match (name.as_str(), self.checked_ty(e)?) {
            ("Int" | "UInt", Ty::Int | Ty::UInt) => Const::Int(0),
            ("Float64", Ty::Float64) => Const::Float(0.0),
            ("Bool", Ty::Bool) => Const::Bool(false),
            _ => return None,
        };
        let value = self.constant(e, zero);
        if name != "UInt" {
            return Some(value);
        }
        let dest = self.fresh_typed(span(e), None, Ty::UInt);
        self.emit(MirInstr::Call {
            dest,
            func: FuncRef::named("UInt"),
            raises: None,
            args: vec![value],
            kwargs: Vec::new(),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        Some(dest)
    }

    /// Emit a `Const` writing a fresh register.
    pub(in crate::mir) fn constant(&mut self, e: &Expr, k: Const) -> Reg {
        let constant_ty = match &k {
            Const::Int(_) => Some(Ty::Int),
            Const::Float(_) => Some(Ty::Float64),
            Const::IntLiteral(_) => Some(Ty::IntLiteral),
            Const::FloatLiteral(_) => Some(Ty::FloatLiteral),
            Const::Bool(_) => Some(Ty::Bool),
            Const::Str(_) => Some(Ty::StringLiteral),
            Const::None => Some(Ty::None),
            Const::Dtype(_) => Some(Ty::Dtype),
            Const::Function(_) => self.checked_ty(e),
            Const::Param(value) => value.meta().as_value().cloned(),
        };
        let d = match constant_ty {
            Some(ty) => self.fresh_typed(span(e), None, ty),
            None => self.fresh(span(e), None),
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
        let compatible =
            match found {
                Ty::IntLiteral => {
                    matches!(target, Ty::Int | Ty::UInt | Ty::Float64)
                        || mojito_types::types::is_scalar_simd(target)
                }
                Ty::FloatLiteral => {
                    matches!(target, Ty::Float64)
                        || mojito_types::types::scalar_simd_dtype(target)
                            .is_some_and(mojito_ast::ast::Dtype::is_float)
                }
                _ => false,
            } || mojito_types::types::simd_shape(target).is_some_and(|(dtype, width)| {
                width > 1
                    && matches!(found, Ty::IntLiteral | Ty::FloatLiteral)
                    && mojito_types::types::splats_to(
                        found,
                        &mojito_types::types::SimdDtype::Known(dtype),
                    )
            });
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
