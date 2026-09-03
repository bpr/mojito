//! Indirect/element invocations, nested closures, keep-alives, and
//! register materialization.

use super::*;

impl Flatten<'_> {
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
                let place = self.materialize_call_reference_place(e, element, reference);
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
    pub(in crate::mir) fn try_simd_call(&mut self, e: &Expr, args: &[Expr]) -> Option<Reg> {
        let (dtype, width) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructSimd { dtype, width } => {
                    usize::try_from(width).ok().map(|width| (dtype, width))
                }
                _ => None,
            })?;
        let elems = self.args(args);
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
        let param_arg_regs = self.param_arg_regs(param_args, &span(e));
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
                ty: info.callable_ty.clone().unwrap_or(Ty::Param {
                    name: "$capture".to_string(),
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
                place: self.binding_place(capture.binding, &capture.name),
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
            Const::Function(_) => self.checked_ty(e),
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
        let compatible = match (found, target) {
            (Ty::IntLiteral, Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }) => true,
            (Ty::FloatLiteral, Ty::Float64) => true,
            (Ty::FloatLiteral, Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            _ => false,
        };
        if !compatible {
            return value;
        }
        let dest = self.fresh_typed(source, None, target.clone());
        self.emit(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
        dest
    }
}
