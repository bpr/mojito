//! The `expr_unconverted` expression dispatcher.

use super::*;

impl Flatten<'_> {
    pub(in crate::mir) fn expr_unconverted(&mut self, e: &Expr) -> Reg {
        match &e.kind {
            // --- Literals ------------------------------------------------------
            ExprKind::Int(n) => self.constant(e, Const::IntLiteral(n.clone())),
            ExprKind::Float(x) => self.constant(e, Const::FloatLiteral(x.clone())),
            ExprKind::Bool(b) => self.constant(e, Const::Bool(*b)),
            ExprKind::Str(s) => self.constant(e, Const::Str(s.clone())),
            ExprKind::None => self.constant(e, Const::None),
            ExprKind::Uninitialized => self.constant(e, Const::None),
            // The `p[]` dereference marker: the checker validated the pointer
            // receiver, and offset 0 is the whole lowering.
            ExprKind::EmptySubscript => self.constant(e, Const::Int(0)),
            ExprKind::Spread(_) => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(
                    "unexpanded call spread reached MIR lowering".to_string(),
                ));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }

            // --- Variable reads ------------------------------------------------
            // A bare read defaults to `Copy` — the lifecycle copy for owned
            // storage. Consuming conventions keep it; a read-convention call
            // argument is instead bound as a shallow place read where the
            // checker marked `BorrowReadArgument` (`lower_call_argument`), and
            // `x^` lowers as `Move` below.
            ExprKind::Identifier(name) => {
                if let Some(target) = self.resolved_callable(e) {
                    return self.constant(e, Const::Function(target));
                }
                if let Some(info) = self.nested_info(e) {
                    return self.load_nested_closure(name, &info, span(e));
                }
                if !self.vars.iter().any(|candidate| candidate == name)
                    && self.overloads.is_function(name)
                {
                    return self.constant(e, Const::Function(name.clone()));
                }
                let var = self.expression_var(name, e);
                let d = self.fresh(span(e), Some(var));
                if self.is_origin_bearing_pointer(e) {
                    // Reading a pointer variable produces its handle value;
                    // `UseVar` would read through the stored `Value::Ref` the
                    // way a `ref` binding does. `MakeRef` on the root forwards
                    // the existing handle unchanged.
                    self.emit(MirInstr::MakeRef {
                        dest: d,
                        place: MirPlace::root(var, self.var_types.get(&var).cloned()),
                    });
                    return d;
                }
                if let Some(loan) = self.aliases.get(&var).cloned() {
                    let mut place = loan.place;
                    place.through = Some(var);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                } else if self.runtime_aliases.contains(&var) {
                    let handle = self.fresh(e.source_span(), Some(var));
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place: {
                            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                            place.through = Some(var);
                            place
                        },
                    });
                    self.emit(MirInstr::ReadRef {
                        dest: d,
                        reference: handle,
                    });
                } else {
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Copy,
                    });
                    return d;
                }
                // A checked value-copy of an alias-bound variable (a borrowed
                // loop binding consumed by a `var` argument) must run the
                // referent's `__copyinit__` rather than alias its owning
                // storage — the alias slot's referent stays live in its
                // collection. Ordinary variables keep the `UseVar` path above:
                // their copy/move lifecycle is drop-elaborated.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                }) && !matches!(self.checked_ty(e), Some(Ty::Ref(_)))
                {
                    let copied = self.fresh_typed(
                        span(e),
                        Some(var),
                        self.checked_ty(e).unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::CopyValue {
                        dest: copied,
                        value: d,
                    });
                    return copied;
                }
                d
            }
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            ExprKind::Transfer(inner) => {
                if let ExprKind::Identifier(name) = &inner.kind {
                    let var = self.expression_var(name, inner);
                    let d = self.fresh(span(e), Some(var));
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Move,
                    });
                    d
                } else if let Some(place) = self.pure_field_place(inner) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else if let ExprKind::Index { object, .. } = &inner.kind
                    && matches!(self.checked_ty(object), Some(Ty::Tuple(_)))
                    && let Some(place) = self.try_place(inner)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else {
                    self.expr(inner)
                }
            }

            // --- Operators -----------------------------------------------------
            ExprKind::Prefix(op, a) => {
                let ra = self.expr(a);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::UnOp {
                    op: *op,
                    dest: d,
                    a: ra,
                });
                d
            }
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), a, b) => {
                self.short_circuit(*op, a, b, span(e))
            }
            // A checked nominal membership operation is an ordinary borrowed
            // `container.__contains__(value)` call.  Keeping it as a value-only
            // `BinOp` loses the receiver place; the VM would then install a
            // shallow struct value in the callee's `self` slot and destroy its
            // owned fields on return.  For pointer-backed collections that can
            // free the caller's storage.  Preserve source evaluation order
            // (value before container), the selected overload, and the normal
            // method-call place/capture contract.
            ExprKind::Infix(op @ (InfixOp::In | InfixOp::NotIn), value, container)
                if matches!(self.checked_ty(container), Some(Ty::Struct(..))) =>
            {
                let (argument, arg_place) = self.lower_call_argument(value);
                let (recv, recv_place) = self.lower_call_receiver(container);
                let contains = self.fresh_typed(span(e), None, Ty::Bool);
                self.emit_interior_invalidations(container, None);
                self.emit_call_invalidations(e, std::slice::from_ref(value), &[]);
                self.emit(MirInstr::MethodCall {
                    dest: contains,
                    recv,
                    method: "__contains__".to_string(),
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    reference_result: None,
                    result_adapter: None,
                    args: vec![argument],
                    kwargs: Vec::new(),
                    recv_place,
                    recv_writes: self.receiver_writes(e),
                    arg_places: vec![arg_place],
                    kwarg_places: Vec::new(),
                    capture_accesses: self.checked_call_capture_accesses(e),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                });
                self.emit_nested_closure_argument_keepalives(std::slice::from_ref(value), &[]);
                if matches!(op, InfixOp::NotIn) {
                    let dest = self.fresh_typed(span(e), None, Ty::Bool);
                    self.emit(MirInstr::UnOp {
                        op: PrefixOp::Not,
                        dest,
                        a: contains,
                    });
                    dest
                } else {
                    contains
                }
            }
            ExprKind::Infix(op, a, b) => {
                let ra = self.expr(a); // operands left-to-right (evaluation order is explicit)
                let rb = self.expr(b);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::BinOp {
                    op: *op,
                    dest: d,
                    a: ra,
                    b: rb,
                    resolved: self.resolved_callable(e),
                });
                d
            }

            // --- Calls / access ------------------------------------------------
            // NOTE: keyword args + default-slot matching (`call::match_call_slots`)
            // are a follow-up; the checker has already validated them, so only the
            // positional `args` are flattened here.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                if name == "__mojito_fieldwise_copy" {
                    return self.expr(
                        args.first()
                            .expect("checked synthesized fieldwise copy has one argument"),
                    );
                }
                if let Some(crate::SemanticAdjustment::ConstructTypeParam { param }) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::ConstructTypeParam { .. }
                        )
                    })
                {
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::ConstructTypeParam { dest, param });
                    return dest;
                }
                if let Some(crate::SemanticAdjustment::SizeOf { ty }) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(adjustment, crate::SemanticAdjustment::SizeOf { .. })
                    })
                {
                    let dest = self.fresh_typed(span(e), None, Ty::Int);
                    self.emit(MirInstr::SizeOf { dest, ty });
                    return dest;
                }
                // A checked pointer construction materializes the frame/slot
                // handle for its source place; the checked pointer type keeps
                // the origin while the runtime value erases it.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::PointerToPlace { .. })
                }) {
                    let value = &kwargs
                        .first()
                        .expect("checked pointer construction has a 'to=' argument")
                        .value;
                    let place = self.place(value);
                    let dest = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MakeRef { dest, place });
                    return dest;
                }
                // Compiler-private inline uninit-storage construction:
                // uninitialized, or holding a moved initial payload.
                if let Some(init) = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::UninitStorageMake { init, .. } => Some(init),
                        _ => None,
                    })
                {
                    let init = init.then(|| {
                        self.expr(args.first().expect("checked storage construction payload"))
                    });
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::UninitStorage { dest, init });
                    return dest;
                }
                if let Some(crate::SemanticAdjustment::ConstructVariant {
                    alternatives,
                    index,
                }) = self.checked_adjustments(e).into_iter().find(|adjustment| {
                    matches!(
                        adjustment,
                        crate::SemanticAdjustment::ConstructVariant { .. }
                    )
                }) {
                    let value = self.expr(
                        args.first()
                            .expect("checked Variant construction has one payload"),
                    );
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeVariant {
                        dest,
                        alternatives,
                        index,
                        value,
                    });
                    return dest;
                }
                // SIMD construction resolves its `[DType.<dt>, width]` parameters
                // here (the MIR is otherwise untyped about them).
                if let Some(r) = self.try_simd_call(e, args) {
                    return r;
                }
                // `objs[0](3)` / `grid[i, j](x)`: the checker re-dispatched
                // these value brackets as subscript-then-indirect-call, so the
                // brackets are runtime indices, never compile-time parameters
                // (and never the named-local callable path below).
                if let Some(plan) = self.element_invocation(e) {
                    let indices: Vec<&Expr> = param_args
                        .iter()
                        .filter_map(|argument| match argument {
                            ParamArg::Value(value) => Some(value),
                            _ => None,
                        })
                        .collect();
                    let receiver = Expr {
                        kind: ExprKind::Identifier(name.clone()),
                        span: e.span,
                        source: e.source.clone(),
                        syntax_id: crate::token::SyntaxId::fresh(),
                    };
                    return self
                        .lower_element_invocation(e, &receiver, plan, &indices, args, kwargs);
                }
                // A call to a nested `def` (a closure, called by name in scope):
                // rewrite to its lifted function, prepending the captured enclosing
                // locals as leading arguments (passed as places, so the `mut`
                // capture parameters write back — reference-capture semantics).
                if let Some(info) = self.nested_info(e) {
                    return self.lower_nested_call(e, &info, param_args, args, kwargs);
                }
                // A local with a function type (normally a callable parameter)
                // shadows any global function of the same name.
                if self.vars.iter().any(|candidate| candidate == name) {
                    let callee = self.expr(&Expr {
                        kind: ExprKind::Identifier(name.clone()),
                        span: e.span,
                        source: e.source.clone(),
                        syntax_id: crate::token::SyntaxId::fresh(),
                    });
                    let callable_ty = self
                        .vars
                        .iter()
                        .position(|candidate| candidate == name)
                        .and_then(|variable| self.var_types.get(&(variable as VarId)))
                        .cloned()
                        .or_else(|| self.f.reg_types.get(&callee.0).cloned());
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let param_decls = callable_ty
                        .as_ref()
                        .map(generic_callable_param_decls)
                        .unwrap_or_default();
                    let saved_anchor_permission = self.allow_argument_anchors;
                    self.allow_argument_anchors = self.call_anchors_arguments(e);
                    let (regs, arg_places) = self.lower_call_arguments(args, false);
                    let (kw, kwarg_places) = self.lower_call_keywords(kwargs, false);
                    self.allow_argument_anchors = saved_anchor_permission;
                    let place = self.resolved_place(name);
                    let callee_place = place.is_typed().then_some(place);
                    let dest = self.fresh(span(e), None);
                    self.emit_call_invalidations(e, args, kwargs);
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    let (instantiated_contract, instantiated_args) = self
                        .instantiated_callable_contract(e)
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
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        callee_place,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        param_decls,
                        instantiated_contract,
                        instantiated_args,
                    });
                    self.install_call_transfers(
                        e,
                        transfer_recv_place.as_ref(),
                        &transfer_arg_places,
                    );
                    return dest;
                }
                // `__RuntimeTuple` is the compiler-private heterogeneous pack
                // storage primitive. Public `Tuple` is an ordinary nominal
                // variadic struct and follows the call path below.
                if name == "__RuntimeTuple"
                    && kwargs.is_empty()
                    && !self.overloads.is_function(name)
                {
                    let regs = self.args(args);
                    let element_types = match self.checked_ty(e) {
                        Some(Ty::Tuple(elements)) => Some(elements),
                        _ => None,
                    };
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeTuple {
                        dest,
                        elems: regs,
                        element_types,
                    });
                    return dest;
                }
                // Compile-time parameter arguments (`Name[param_args](...)`),
                // evaluated before ordinary call arguments: a
                // **value** parameter is a comptime `Int` expression flattened to a
                // register; a **type** parameter is erased (`None`).
                let param_arg_regs = self.param_arg_regs(param_args);
                // Retain only checker-selected `mut`/`ref` caller places. A
                // syntactically simple copied argument remains eligible for
                // ASAP destruction after its value has been evaluated.
                // A call's argument list anchors nested loan-carrying
                // temporaries unless another channel already carries their
                // loans (see `call_anchors_arguments`). A construction's
                // aggregate result additionally carries its arguments' loans
                // forward instead (its binding — or its own anchor one call
                // level up — installs them).
                let view_result = self.borrows_view_result(e);
                let saved_anchor_permission = self.allow_argument_anchors;
                self.allow_argument_anchors = !matches!(
                    self.checked_ty(e),
                    Some(Ty::Struct(constructed, _)) if constructed == *name
                ) && self.call_anchors_arguments(e);
                let (regs, arg_places) = self.lower_call_arguments(args, view_result);
                self.allow_argument_anchors = saved_anchor_permission;
                // A copy construction (`Name(copy=place)`) binds its single
                // keyword to the copy constructor's borrowed `copy: Self`
                // parameter. Read a place source shallowly and retain it:
                // `construct_via_copy` runs `__copyinit__` on the live source
                // exactly once, where an ordinary value read would run the
                // user's copy constructor a second time for the argument
                // itself — observable through its side effects.
                let copy_construction_source = (args.is_empty()
                    && kwargs.len() == 1
                    && kwargs[0].name == "copy"
                    && matches!(
                        self.checked_ty(e),
                        Some(Ty::Struct(constructed, _)) if constructed == *name
                    ))
                .then(|| self.simple_place(&kwargs[0].value))
                .flatten();
                let (kw, kwarg_places) = if let Some(place) = copy_construction_source {
                    let source_expr = &kwargs[0].value;
                    let source = self.fresh_typed(
                        span(source_expr),
                        Some(place.root),
                        place
                            .ty
                            .clone()
                            .or_else(|| self.checked_ty(source_expr))
                            .unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::LoadPlace {
                        dest: source,
                        place: place.clone(),
                    });
                    (vec![("copy".to_string(), source)], vec![Some(place)])
                } else {
                    self.lower_call_keywords(kwargs, view_result)
                };
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
                let stringify = crate::symbol::is_stdlib_string_struct(name)
                    && (self.resolved_callable(e).as_deref() == Some("String")
                        || self.checked_ty(e) == Some(Ty::StringLiteral));
                // Builtin string producers wrapped by the nominal-String
                // conversion keep their own callee but type their register
                // as the compile-time string the wrap consumes.
                let literal_result = stringify
                    || (matches!(name.as_str(), "input" | "repr")
                        && self.implicit_conversion(e).is_some());
                let target = if stringify {
                    "String".to_string()
                } else {
                    self.resolved_callable(e)
                        .unwrap_or_else(|| self.overloaded_name(name, args.len()))
                };
                let d = if literal_result {
                    self.fresh_typed(span(e), None, Ty::StringLiteral)
                } else {
                    self.fresh(span(e), None)
                };
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                let transfer_arg_places = arg_places.clone();
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(&target),
                    raises: self.checked_raises(e),
                    args: regs,
                    kwargs: kw,
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                self.install_call_transfers(e, None, &transfer_arg_places);
                d
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                // `pointer.unsafe_origin_cast[...]()` retypes provenance only: the
                // runtime value is the receiver, unchanged, and the origin
                // parameter argument never lowers (origins erase).
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        crate::SemanticAdjustment::PointerOriginCast { .. }
                    )
                }) && let ExprKind::Member { object, .. } = &callee.kind
                {
                    return self.expr(object);
                }
                // Parameterized SIMD methods (`v.cast[DType.<dt>]()`) carry
                // their checker-resolved payload in the adjustment; the
                // receiver is the member callee's object.
                let simd_cast = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SimdCast { dtype, width } => {
                            Some((dtype, width))
                        }
                        _ => None,
                    });
                if let Some((dtype, width)) = simd_cast
                    && let ExprKind::Member { object, .. } = &callee.kind
                {
                    let value = self.expr(object);
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::SimdCast {
                        dest,
                        value,
                        dtype,
                        width: usize::try_from(width).unwrap_or(0),
                    });
                    return dest;
                }
                let simd_shuffle = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SimdShuffle { mask } => Some(mask),
                        _ => None,
                    });
                if let Some(mask) = simd_shuffle
                    && let ExprKind::Member { object, .. } = &callee.kind
                {
                    let value = self.expr(object);
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::SimdShuffle { dest, value, mask });
                    return dest;
                }
                if let Some(operation) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::VariantIs { .. }
                                | crate::SemanticAdjustment::VariantTypeSupported { .. }
                                | crate::SemanticAdjustment::VariantSet { .. }
                                | crate::SemanticAdjustment::VariantSetInitWith { .. }
                                | crate::SemanticAdjustment::VariantTake { .. }
                                | crate::SemanticAdjustment::VariantDeinitWith { .. }
                                | crate::SemanticAdjustment::VariantReplace { .. }
                        )
                    })
                {
                    let ExprKind::Member { object, .. } = &callee.kind else {
                        unreachable!("checked Variant operation has a member callee")
                    };
                    match operation {
                        crate::SemanticAdjustment::VariantIs { index, .. } => {
                            let variant = self.expr(object);
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::VariantIs {
                                dest,
                                variant,
                                index,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTypeSupported { supported } => {
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest,
                                k: Const::Bool(supported),
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantSet { index, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.set receiver is a writable place");
                            let value = self
                                .expr(args.first().expect("checked Variant.set has one payload"));
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantSet {
                                dest,
                                place,
                                index,
                                value,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantSetInitWith { index, .. } => {
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
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantDeinitWith { index, .. } => {
                            let index = index.to_owned();
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
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTake { index, checked, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.take receiver is an owned place");
                            let variant = self.fresh(span(object), None);
                            self.emit(MirInstr::MovePlace {
                                dest: variant,
                                place,
                            });
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantTake {
                                dest,
                                variant,
                                index,
                                checked,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantReplace {
                            input_index,
                            output_index,
                            checked,
                            ..
                        } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.replace receiver is writable");
                            let value = self.expr(
                                args.first()
                                    .expect("checked Variant.replace has one payload"),
                            );
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantReplace {
                                dest,
                                place,
                                input_index,
                                output_index,
                                value,
                                checked,
                            });
                            return dest;
                        }
                        _ => unreachable!("filtered Variant operation"),
                    }
                }
                // `a.b[i](x)`: the member-base element call re-dispatched by
                // the checker — the callee expression is the subscripted
                // receiver, and the brackets are runtime indices.
                if let Some(plan) = self.element_invocation(e) {
                    let indices: Vec<&Expr> = param_args
                        .iter()
                        .filter_map(|argument| match argument {
                            ParamArg::Value(value) => Some(value),
                            _ => None,
                        })
                        .collect();
                    return self.lower_element_invocation(e, callee, plan, &indices, args, kwargs);
                }
                if let Some(param_decls) = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::ParameterizedMethodCall { param_decls } => {
                            Some(param_decls)
                        }
                        _ => None,
                    },
                ) {
                    let ExprKind::Member { object, field } = &callee.kind else {
                        unreachable!("checked parameterized method call has a member callee")
                    };
                    // Keep this as a direct method invocation. In particular,
                    // do not synthesize a bound-method value (which would make
                    // its receiver/environment escapable).
                    let (recv, recv_place) = self.lower_call_receiver(object);
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let saved_anchor_permission = self.allow_argument_anchors;
                    self.allow_argument_anchors = self.call_anchors_arguments(e);
                    let (argument_regs, arg_places) = self.lower_call_arguments(args, false);
                    self.allow_argument_anchors = saved_anchor_permission;
                    let (keyword_regs, kwarg_places) = self.lower_call_keywords(kwargs, false);
                    let dest = self.fresh(span(e), None);
                    let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                    self.emit_interior_invalidations(object, None);
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::MethodCall {
                        dest,
                        recv,
                        method: field.clone(),
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        reference_result: self
                            .checked_call_contract(e)
                            .and_then(|contract| contract.reference_result),
                        result_adapter: self
                            .checked_call_contract(e)
                            .and_then(|contract| contract.result_adapter),
                        args: argument_regs,
                        kwargs: keyword_regs,
                        recv_place: if implicitly_copied_receiver {
                            None
                        } else {
                            recv_place
                        },
                        recv_writes: self.receiver_writes(e),
                        arg_places,
                        kwarg_places,
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs,
                        param_decls,
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return dest;
                }
                let mut callee_place = self.callable_receiver_place(callee);
                let callable_ty = self.checked_ty(callee);
                let lambda_callee = matches!(callee.kind, ExprKind::Lambda { .. });
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
                    });
                    let place = MirPlace::root(slot, callable_ty.clone());
                    callee_place = place.is_typed().then_some(place);
                }
                let param_arg_regs = self.param_arg_regs(param_args);
                let resolved = self.resolved_callable(e);
                let raises = self.checked_raises(e);
                self.emit_indirect_invocation(
                    e,
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
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                // `x.copy()` on a built-in copyable value has no callee: the
                // checker resolved it to the value read itself.
                if method == "copy"
                    && args.is_empty()
                    && kwargs.is_empty()
                    && self
                        .checked_ty(object)
                        .is_some_and(|ty| crate::checker::builtin_copy_is_value_read(&ty))
                {
                    return self.expr(object);
                }
                // `v.set(init_with=…)` infers its alternative from the factory
                // (no explicit type parameter), so it arrives as an ordinary
                // method call rather than a parameterized invoke.
                if let Some(index) =
                    self.checked_adjustments(e)
                        .iter()
                        .find_map(|adjustment| match adjustment {
                            crate::SemanticAdjustment::VariantSetInitWith { index, .. } => {
                                Some(*index)
                            }
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
                    return dest;
                }
                // The parameterless Variant owning operation
                // (`v^.deinit_with(handler)`) is spelled as an ordinary method
                // call rather than a parameterized invoke.
                if let Some(index) =
                    self.checked_adjustments(e)
                        .iter()
                        .find_map(|adjustment| match adjustment {
                            crate::SemanticAdjustment::VariantDeinitWith { index, .. } => {
                                Some(*index)
                            }
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
                    return dest;
                }
                // A callable-typed FIELD invocation (`holder.callback(1)`)
                // loads the stored value and calls indirectly; the callee
                // place is the field's, so a closure environment stays
                // reachable through stable storage.
                if let Some(crate::SemanticAdjustment::FieldInvocation { callable }) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::FieldInvocation { .. }
                        )
                    })
                {
                    let (recv, recv_place) = self.lower_call_receiver(object);
                    let callee = self.fresh_typed(span(e), None, callable.clone());
                    self.emit(MirInstr::GetField {
                        dest: callee,
                        base: recv,
                        field: method.clone(),
                    });
                    let callee_place = recv_place.map(|mut place| {
                        place.project(Proj::Field(method.clone()), callable.clone());
                        place
                    });
                    let resolved = self.resolved_callable(e);
                    let raises = self.checked_raises(e);
                    return self.emit_indirect_invocation(
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
                    );
                }
                let pointer_storage = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::PointerStorageTake { element } => {
                            Some((true, element))
                        }
                        crate::SemanticAdjustment::PointerStorageDestroy { element } => {
                            Some((false, element))
                        }
                        _ => None,
                    },
                );
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
                    return dest;
                }
                // Compiler-private inline uninit storage (`MaybeUninit`'s
                // field). `unsafe_write` stores through the payload projection
                // — the place is opaque to drop elaboration, so a previously
                // written payload is overwritten raw (it leaks by design).
                // `take`/`destroy` consume the transferred storage value.
                let uninit_storage = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::UninitStorageWrite { element } => {
                            Some((UninitStorageOp::Write, element))
                        }
                        crate::SemanticAdjustment::UninitStorageTake { element } => {
                            Some((UninitStorageOp::Take, element))
                        }
                        crate::SemanticAdjustment::UninitStorageDestroy { element } => {
                            Some((UninitStorageOp::Destroy, element))
                        }
                        _ => None,
                    },
                );
                if let Some((op, element)) = uninit_storage {
                    debug_assert!(kwargs.is_empty());
                    match op {
                        UninitStorageOp::Write => {
                            let src = self
                                .expr(args.first().expect("checked unsafe_write has one value"));
                            let mut place = self.place(object);
                            place.project(Proj::UninitPayload, element);
                            self.emit(MirInstr::Store { place, src });
                            let dest = self.fresh_typed(span(e), None, Ty::None);
                            self.emit(MirInstr::Const {
                                dest,
                                k: Const::None,
                            });
                            return dest;
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
                            return dest;
                        }
                    }
                }
                // `pointer.unsafe_offset(i)` is provenance-preserving element
                // arithmetic: the ordinary pointer `+` operation.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::PointerOffset)
                }) {
                    let pointer = self.expr(object);
                    let offset = self.expr(
                        args.first()
                            .expect("checked unsafe_offset has one argument"),
                    );
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::BinOp {
                        op: crate::ast::InfixOp::Add,
                        dest,
                        a: pointer,
                        b: offset,
                        resolved: None,
                    });
                    return dest;
                }
                // `pointer.unsafe_write(value)` / `unsafe_write(copy=v)`
                // initializes the pointee at offset 0 — the same store family
                // as `pointer[] = value`. An origin-bearing pointer writes its
                // source place (owner substitution when stably bound, else
                // through the runtime handle); a heap pointer stores through a
                // synthetic binding so chained receivers stay expressible.
                let pointer_write = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::PointerWrite { element, copy } => {
                            Some((element, copy))
                        }
                        _ => None,
                    },
                );
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
                        place.project(Proj::Index(index), element.clone());
                        self.emit(MirInstr::Store { place, src });
                    }
                    let dest = self.fresh_typed(span(e), None, Ty::None);
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    return dest;
                }
                let explicit_destroy = self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::ExplicitDestroy)
                });
                let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                if let ExprKind::Identifier(type_name) = &object.kind
                    && !self.vars.iter().any(|name| name == type_name)
                {
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
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&target),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // A **static** method on a parameterized type — the receiver is a
                // type, not a value (`Dict[Int, Int].fromkeys(...)` or the pointer
                // family's `UnsafePointer[T].alloc(n)`). Lower to a call on the
                // checker-selected symbol (overloaded statics carry their exact
                // spelling). The receiver's compile-time arguments are already
                // resolved into that selection by the checker and erase here —
                // a static's frame declares only the method's own parameters,
                // so struct arguments must not occupy its `param_arg_regs`
                // slots.
                if let ExprKind::TypeApply { name, .. } = &object.kind {
                    let regs = self.args(args);
                    let kw: Vec<(String, Reg)> = kwargs
                        .iter()
                        .map(|k| (k.name.clone(), self.expr(&k.value)))
                        .collect();
                    let d = self.fresh(span(e), None);
                    let target = self
                        .resolved_callable(e)
                        .unwrap_or_else(|| format!("{name}.{method}"));
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&target),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places: vec![None; args.len()],
                        kwarg_places: vec![None; kwargs.len()],
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // The single-argument spelling of the same static receiver
                // (`Box[String].filled(...)`) parses as a value subscript; the
                // checker routed it as a static call, and a subscript base
                // naming no local is likewise a type, never a place. The
                // bracket content is a compile-time argument — do not lower it.
                if let ExprKind::Index { object: base, .. } = &object.kind
                    && let ExprKind::Identifier(type_name) = &base.kind
                    && !self.vars.iter().any(|name| name == type_name)
                {
                    let regs = self.args(args);
                    let kw: Vec<(String, Reg)> = kwargs
                        .iter()
                        .map(|k| (k.name.clone(), self.expr(&k.value)))
                        .collect();
                    let d = self.fresh(span(e), None);
                    let target = self
                        .resolved_callable(e)
                        .unwrap_or_else(|| format!("{type_name}.{method}"));
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&target),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places: vec![None; args.len()],
                        kwarg_places: vec![None; kwargs.len()],
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // If the receiver is a place, load it through that place (indices
                // evaluated once) and keep the place for write-back; otherwise it is
                // a temporary evaluated for its value only.
                let receiver_expr = if explicit_destroy {
                    match &object.kind {
                        ExprKind::Transfer(inner) => inner.as_ref(),
                        _ => object.as_ref(),
                    }
                } else {
                    object.as_ref()
                };
                let (recv, recv_place) = self.lower_call_receiver(receiver_expr);
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
                    method: method.clone(),
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
            ExprKind::Member { object, field } => {
                // A pure field chain rooted at a variable (`p.a`, `p.a.b`) lowers to
                // a `LoadPlace` (a place read) so the ownership analysis sees *which*
                // field is read — enabling field-sensitive partial-move checking
                // (reading `p.b` after `p.a^` stays legal). A member of a temporary
                // or an indexed base keeps the register-based `GetField`.
                let descriptor_field = matches!(
                    self.checked_ty(object),
                    Some(Ty::Struct(name, args))
                        if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                            && args.is_empty()
                );
                if !descriptor_field && let Some(place) = self.pure_field_place(e) {
                    let place_root = place.root;
                    let place_ty = place.ty.clone();
                    let loaded = self.fresh_typed(
                        span(e),
                        Some(place_root),
                        place_ty
                            .clone()
                            .or_else(|| self.checked_ty(e))
                            .unwrap_or(Ty::Error),
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
                    // Reference-valued fields retain their existing handle/read
                    // path.  Their ordinary referent copies are selected by the
                    // checked `ReferenceResult` adjustment, not by this nominal
                    // field rule.
                    if !matches!(place_ty, Some(Ty::Ref(_)))
                        && self.checked_adjustments(e).iter().any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                        })
                    {
                        let copied = self.fresh_typed(
                            span(e),
                            Some(place_root),
                            self.checked_ty(e).unwrap_or(Ty::Error),
                        );
                        self.emit(MirInstr::CopyValue {
                            dest: copied,
                            value: loaded,
                        });
                        copied
                    } else {
                        loaded
                    }
                } else {
                    let base = if self.reference_result(object).is_some() {
                        self.lower_call_receiver(object).0
                    } else {
                        self.expr(object)
                    };
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::GetField {
                        dest: d,
                        base,
                        field: field.clone(),
                    });
                    // The same checked value-copy boundary as the place-read
                    // branch above: a field selected for a consuming value
                    // context must run its `__copyinit__` even when the base is
                    // a temporary or reference-projected call result, or the
                    // register copy aliases the base's heap storage past its
                    // lifetime.
                    if !matches!(self.checked_ty(e), Some(Ty::Ref(_)))
                        && self.checked_adjustments(e).iter().any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                        })
                    {
                        // Type the loaded register from the checked expression
                        // rather than leaving it to `GetField` instruction
                        // typing: in a generic body the declaration's raw field
                        // parameter would disagree with the copy's checked
                        // type.
                        if let Some(ty) = self.checked_ty(e) {
                            self.f.reg_types.insert(d.0, ty);
                        }
                        let copied = self.fresh_typed(
                            span(e),
                            None,
                            self.checked_ty(e).unwrap_or(Ty::Error),
                        );
                        self.emit(MirInstr::CopyValue {
                            dest: copied,
                            value: d,
                        });
                        copied
                    } else {
                        d
                    }
                }
            }
            // A variant projection spelled with a struct-name index
            // (`v[String]`) carries the same checked adjustment as the
            // type-token spelling below and lowers identically.
            ExprKind::Index { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(_))
                    && self.checked_adjustments(e).iter().any(|adjustment| {
                        matches!(adjustment, crate::SemanticAdjustment::VariantProject { .. })
                    }) =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("the guard established an identifier receiver");
                };
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("checked Variant projection carries a tag");
                let mut place = self.resolved_place(name);
                if place.root_ty.is_none() {
                    place.root_ty = Some(Ty::Variant(
                        self.checked_adjustments(e)
                            .into_iter()
                            .find_map(|adjustment| match adjustment {
                                crate::SemanticAdjustment::VariantProject {
                                    alternatives, ..
                                } => Some(alternatives),
                                _ => None,
                            })
                            .unwrap_or_default(),
                    ));
                }
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                place.project(Proj::Variant(index), ty);
                let root = place.root;
                let dest = self.fresh(span(e), Some(root));
                self.emit(MirInstr::LoadPlace { dest, place });
                // The checked value-copy boundary: a Copyable payload runs its
                // `__copyinit__` out of the variant's storage instead of
                // aliasing it past the owner's lifetime.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                }) {
                    let copied = self.fresh_typed(
                        span(e),
                        Some(root),
                        self.checked_ty(e).unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::CopyValue {
                        dest: copied,
                        value: dest,
                    });
                    // Keep the owning variant alive through the copy: the
                    // loaded register aliases its storage until `__copyinit__`
                    // has produced the independent value.
                    self.emit(MirInstr::KeepAlive { var: root });
                    return copied;
                }
                dest
            }
            ExprKind::Index { object, index } => {
                // An indexed reference-bearing aggregate element is a storage
                // place whose checked type is `ref T`; load through the stored
                // handle exactly like a direct reference field.  Ordinary
                // indexing remains the register-based operation below. A
                // checker-selected nominal accessor must stay on that dispatch
                // path: projecting the nominal struct as raw indexed storage
                // would lose its concrete `__getitem__$N` target.
                if matches!(self.checked_place_ty(e), Some(Ty::Ref(_)))
                    && self.resolved_callable(e).is_none()
                    && !matches!(self.checked_ty(object), Some(Ty::Struct(..)))
                    && let Some(place) = self.try_place(e)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit_interior_invalidations(e, None);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                // Dereferencing an origin-bearing pointer reads its source
                // place; the checker fixed the offset to 0. A stably bound
                // pointer substitutes the owner place directly, keeping the
                // owner touched (and so droppable) at each access; otherwise
                // the access reads through the runtime handle.
                if let Some(place) = self.pointer_deref_place(object) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                if self.is_origin_bearing_pointer(object) {
                    let reference = self.expr(object);
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::ReadRef { dest: d, reference });
                    return d;
                }
                let has_call = self.checked_call_contract(e).is_some();
                let (base, base_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let (idx, index_place) = self.lower_call_argument(index);
                let call = self.subscript_call_contract(e, &[(index.source_span(), idx)]);
                let intrinsic = call
                    .is_none()
                    .then(|| self.intrinsic_index_dispatch(object))
                    .flatten();
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(index, None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Index {
                    dest: d,
                    base,
                    index: idx,
                    base_place,
                    index_place,
                    call,
                    intrinsic,
                });
                d
            }

            // --- Aggregates ----------------------------------------------------
            ExprKind::ListLit(elems) => {
                if let Some((target, constructor)) = self.array_literal_plan(e) {
                    // One nominal constructor call: the element registers are
                    // the variadic arguments, `__list_literal__` selects the
                    // literal overload, and the `length` parameter argument
                    // reifies the constructed value's `value_params`.
                    let regs = self.args(elems);
                    let none = self.fresh_typed(span(e), None, Ty::None);
                    self.emit(MirInstr::Const {
                        dest: none,
                        k: Const::None,
                    });
                    let length = crate::types::array_parts(&target)
                        .map(|(_, length)| length)
                        .unwrap_or(elems.len() as i64);
                    let length_reg = self.fresh_typed(span(e), None, Ty::Int);
                    self.emit(MirInstr::Const {
                        dest: length_reg,
                        k: Const::Int(length),
                    });
                    let d = self.fresh_typed(span(e), None, target.clone());
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&constructor),
                        raises: None,
                        args: regs.clone(),
                        kwargs: vec![("__list_literal__".to_string(), none)],
                        arg_places: vec![None; regs.len()],
                        kwarg_places: vec![None],
                        capture_accesses: Vec::new(),
                        param_arg_regs: vec![
                            MirParamArg {
                                name: None,
                                value: None,
                            },
                            MirParamArg {
                                name: None,
                                value: Some(length_reg),
                            },
                        ],
                    });
                    return d;
                }
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    for element in elems {
                        let value = self.expr(element);
                        self.insert_nominal_collection(
                            element,
                            collection,
                            &target,
                            &insert,
                            vec![value],
                        );
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // The unchecked CFG helper has no semantic facts. Keep it
                // syntax-total by emitting an ordinary constructor call; the
                // production checked path above always carries an exact target
                // and insertion method.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("List"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::BraceLit(entries) => {
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    let dictionary = dict_elements(&target).is_some();
                    for (key, value) in entries {
                        let key = self.expr(key);
                        let mut arguments = vec![key];
                        if dictionary {
                            arguments.push(
                                self.expr(
                                    value
                                        .as_ref()
                                        .expect("checked dictionary display has paired values"),
                                ),
                            );
                        }
                        self.insert_nominal_collection(e, collection, &target, &insert, arguments);
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // As above, this is only the syntax-only CFG compatibility
                // path. A verified program never guesses its collection kind.
                let dictionary = entries.first().is_none_or(|(_, value)| value.is_some());
                let d = self.fresh(span(e), None);
                let regs = if dictionary {
                    entries
                        .iter()
                        .flat_map(|(key, value)| {
                            let key = self.expr(key);
                            let value = value.as_ref().map(|value| self.expr(value));
                            std::iter::once(key).chain(value)
                        })
                        .collect::<Vec<_>>()
                } else {
                    entries
                        .iter()
                        .map(|(key, _)| self.expr(key))
                        .collect::<Vec<_>>()
                };
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(if dictionary { "Dict" } else { "Set" }),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => self.comprehension(e, *kind, key.as_deref(), value, clauses),
            ExprKind::TupleLit(elems) => {
                if let Some((target, None)) = self.collection_plan(e)
                    && let Ty::Struct(name, _) = &target
                {
                    let regs = self.args(elems);
                    let dest = self.fresh_typed(span(e), None, target.clone());
                    self.emit(MirInstr::Call {
                        dest,
                        func: FuncRef::named(name),
                        raises: None,
                        args: regs.clone(),
                        kwargs: Vec::new(),
                        arg_places: vec![None; regs.len()],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                    });
                    return dest;
                }
                // Syntax-only lowering cannot select a variadic specialization,
                // but it still emits an ordinary public constructor call. The
                // private `MakeTuple` opcode is reserved for `__RuntimeTuple`.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("Tuple"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }

            // Walrus `:=` reaches MIR after type checking. Preserve an explicit
            // unsupported boundary rather than assigning accidental semantics.
            ExprKind::Named { name, value } => {
                let value = self.expr(value);
                let var = self.var(name);
                self.emit(MirInstr::DefVar {
                    var,
                    src: value,
                    binding_ty: None,
                });
                value
            }
            // Ternary `a if cond else b` — a value-producing branch (like the
            // short-circuit lowering, but both arms assign the result).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => self.ternary(cond, then_branch, else_branch, span(e)),
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            ExprKind::Compare { first, rest } => self.compare_chain(first, rest, span(e)),
            // Slice `object[lower:upper:step]` → a new List/String.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                let has_call = self.checked_call_contract(e).is_some();
                let (obj, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let lower = lower.as_ref().map(|b| self.expr(b));
                let upper = upper.as_ref().map(|b| self.expr(b));
                let step = step.as_ref().map(|b| self.expr(b));
                let call = self.subscript_call_contract(e, &[]);
                // No intrinsic slice channel remains: StringLiteral
                // positional slicing was removed at the audited head, so
                // every checked slice routes through a nominal call.
                let intrinsic = None;
                let kind = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            descriptors.first().copied().flatten()
                        }
                        _ => None,
                    })
                    .expect("checked slice has a selected descriptor");
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Slice {
                    dest: d,
                    object: obj,
                    kind,
                    lower,
                    upper,
                    step,
                    object_place,
                    arg_places: vec![None],
                    call,
                    intrinsic,
                });
                d
            }
            ExprKind::MultiIndex { object, args } => {
                // `ptr[unsafe_offset=i]` — the keyword spelling of pointer
                // indexed dereference — lowers exactly like `ptr[i]`: place
                // substitution or handle read for origin-bearing pointers,
                // else the pointer-intrinsic `Index` read.
                if matches!(self.checked_ty(object), Some(Ty::Pointer { .. }))
                    && let [crate::ast::SubscriptArg::Keyword { name, value }] = args.as_slice()
                    && name == "unsafe_offset"
                {
                    if let Some(place) = self.pointer_deref_place(object) {
                        let d = self.fresh(span(e), Some(place.root));
                        self.emit(MirInstr::LoadPlace { dest: d, place });
                        return d;
                    }
                    if self.is_origin_bearing_pointer(object) {
                        let reference = self.expr(object);
                        let d = self.fresh(span(e), None);
                        self.emit(MirInstr::ReadRef { dest: d, reference });
                        return d;
                    }
                    let base = self.expr(object);
                    let base_place = self.simple_place(object);
                    let (idx, index_place) = self.lower_call_argument(value);
                    let intrinsic = self.intrinsic_index_dispatch(object);
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::Index {
                        dest: d,
                        base,
                        index: idx,
                        base_place,
                        index_place,
                        call: None,
                        intrinsic,
                    });
                    return d;
                }
                let has_call = self.checked_call_contract(e).is_some();
                let (object, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let descriptors = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            Some(descriptors)
                        }
                        _ => None,
                    })
                    .expect("checked multi-subscript has descriptor metadata");
                let mut arg_places = Vec::with_capacity(args.len());
                let mut kwargs = Vec::new();
                let mut kwarg_places = Vec::new();
                let mut parameter_sources = Vec::new();
                let lowered_args: Vec<_> = args
                    .iter()
                    .zip(descriptors)
                    .filter_map(|(argument, descriptor)| match argument {
                        crate::ast::SubscriptArg::Keyword { name, value } => {
                            debug_assert!(descriptor.is_none());
                            let (register, place) = self.lower_call_argument(value);
                            kwarg_places.push(place);
                            parameter_sources.push((value.source_span(), register));
                            kwargs.push((name.clone(), MirSubscriptArg::Index(register)));
                            None
                        }
                        crate::ast::SubscriptArg::KeywordSlice {
                            name,
                            lower,
                            upper,
                            step,
                            ..
                        } => {
                            kwarg_places.push(None);
                            kwargs.push((
                                name.clone(),
                                MirSubscriptArg::Slice {
                                    kind: descriptor
                                        .expect("keyword slice argument has descriptor kind"),
                                    lower: lower.as_ref().map(|value| self.expr(value)),
                                    upper: upper.as_ref().map(|value| self.expr(value)),
                                    step: step.as_ref().map(|value| self.expr(value)),
                                },
                            ));
                            None
                        }
                        crate::ast::SubscriptArg::Index(value) => {
                            debug_assert!(descriptor.is_none());
                            let (register, place) = self.lower_call_argument(value);
                            arg_places.push(place);
                            parameter_sources.push((value.source_span(), register));
                            Some(MirSubscriptArg::Index(register))
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            arg_places.push(None);
                            Some(MirSubscriptArg::Slice {
                                kind: descriptor.expect("slice argument has descriptor kind"),
                                lower: lower.as_ref().map(|value| self.expr(value)),
                                upper: upper.as_ref().map(|value| self.expr(value)),
                                step: step.as_ref().map(|value| self.expr(value)),
                            })
                        }
                    })
                    .collect();
                let call = self.subscript_call_contract(e, &parameter_sources);
                let dest = self.fresh(span(e), None);
                for argument in args {
                    if let crate::ast::SubscriptArg::Index(argument) = argument {
                        self.emit_interior_invalidations(argument, None);
                    }
                }
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::MultiIndex {
                    dest,
                    object,
                    args: lowered_args,
                    object_place,
                    arg_places,
                    kwargs,
                    kwarg_places,
                    call,
                });
                dest
            }
            // The production discovery path rewrites every concrete `t"…"`
            // occurrence into its lazy `TString` specialization's construction
            // before MIR, so this arm is the output-identical eager fallback:
            // the stage-composed seam (which skips discovery by design) and
            // t-strings inside retained abstract bound-generic bodies lower to
            // `"" + String(part) + …` concatenation here.
            ExprKind::TString { parts, .. } => {
                let mut result = self.fresh(span(e), None);
                self.emit(MirInstr::Const {
                    dest: result,
                    k: Const::Str(String::new()),
                });
                for part in parts {
                    let piece = match part {
                        TStringPart::Literal(text) => {
                            let register = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest: register,
                                k: Const::Str(text.clone()),
                            });
                            register
                        }
                        TStringPart::Expr(value) => {
                            let argument = self.expr(value);
                            // Interpolation's implicit `String(value)` call has
                            // no source expression of its own.  Give the
                            // synthetic result its checked intrinsic type here
                            // instead of asking declaration-based MIR closure to
                            // rediscover the return type of the builtin.
                            let register = self.fresh_typed(span(value), None, Ty::StringLiteral);
                            self.emit(MirInstr::Call {
                                dest: register,
                                func: FuncRef::named("String"),
                                raises: None,
                                args: vec![argument],
                                kwargs: Vec::new(),
                                arg_places: vec![None],
                                kwarg_places: Vec::new(),
                                capture_accesses: Vec::new(),
                                param_arg_regs: Vec::new(),
                            });
                            register
                        }
                    };
                    let joined = self.fresh(span(e), None);
                    self.emit(MirInstr::BinOp {
                        op: InfixOp::Add,
                        dest: joined,
                        a: result,
                        b: piece,
                        resolved: None,
                    });
                    result = joined;
                }
                result
            }
            ExprKind::TypeApply { name, .. }
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::VariantProject { .. })
                }) =>
            {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("checked Variant projection carries a tag");
                let mut place = self.resolved_place(name);
                if place.root_ty.is_none() {
                    place.root_ty = Some(Ty::Variant(
                        self.checked_adjustments(e)
                            .into_iter()
                            .find_map(|adjustment| match adjustment {
                                crate::SemanticAdjustment::VariantProject {
                                    alternatives, ..
                                } => Some(alternatives),
                                _ => None,
                            })
                            .unwrap_or_default(),
                    ));
                }
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                place.project(Proj::Variant(index), ty);
                let root = place.root;
                let dest = self.fresh(span(e), Some(root));
                self.emit(MirInstr::LoadPlace { dest, place });
                // The checked value-copy boundary: a Copyable payload runs its
                // `__copyinit__` out of the variant's storage instead of
                // aliasing it past the owner's lifetime.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                }) {
                    let copied = self.fresh_typed(
                        span(e),
                        Some(root),
                        self.checked_ty(e).unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::CopyValue {
                        dest: copied,
                        value: dest,
                    });
                    // Keep the owning variant alive through the copy: the
                    // loaded register aliases its storage until `__copyinit__`
                    // has produced the independent value.
                    self.emit(MirInstr::KeepAlive { var: root });
                    return copied;
                }
                dest
            }
            ExprKind::TypeApply { name, .. } if self.nested_info(e).is_some() => {
                let info = self
                    .nested_info(e)
                    .expect("guard established a checked nested declaration");
                let dest = self.load_nested_closure(name, &info, span(e));
                // The closure slot carries the declaration's generic callable
                // type; this expression carries the checker's concrete Origin
                // substitution and must win at the MIR value boundary.
                if let Some(specialized) = self.checked_ty(e) {
                    self.f.reg_types.insert(dest.0, specialized);
                }
                dest
            }
            ExprKind::TypeApply { .. } if self.resolved_callable(e).is_some() => self.constant(
                e,
                Const::Function(
                    self.resolved_callable(e)
                        .expect("checked callable TypeApply has a lowered target"),
                ),
            ),
            // A lambda expression materializes its hidden definition's
            // closure at the expression's evaluation point — copy/move
            // captures snapshot here, once per evaluation.
            ExprKind::Lambda { .. } => match self.nested_info(e) {
                Some(info) => self.emit_nested_closure(&info, span(e), false),
                None => {
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::Unsupported(
                        "lambda expression lost its checked nested declaration".to_string(),
                    ));
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::None,
                    });
                    dest
                }
            },
            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. } => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked expression reached MIR lowering: {:?}",
                    e.kind
                )));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }
        }
    }
}
