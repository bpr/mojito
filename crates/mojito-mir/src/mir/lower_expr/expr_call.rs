//! Direct-call and callable-value invocation arms of `expr_unconverted`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_ast::ast::KwArg;
impl Flatten<'_> {
    pub(super) fn call_expr(
        &mut self,
        e: &Expr,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        if name == "__mojito_fieldwise_copy" {
            return self.expr(
                args.first()
                    .expect("checked synthesized fieldwise copy has one argument"),
            );
        }
        if let Some(zero) = self.default_scalar_construction(e) {
            return zero;
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
            return dest;
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::SizeOf { ty }) =
            self.checked_adjustments(e).into_iter().find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::SizeOf { .. }
                )
            })
        {
            // A symbolic type has no layout to take the size of; the
            // elaborator substitutes the instance's type, and only the
            // erased oracle reaches the template's own instruction.
            let dest = self.fresh_typed(span(e), None, Ty::Int);
            self.emit(MirInstr::SizeOf { dest, ty });
            return dest;
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::TypeName { text, ty }) =
            self.checked_adjustments(e).into_iter().find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::TypeName { .. }
                )
            })
        {
            // A type naming a compile-time parameter is spelled per
            // instance, so the template carries the type to the elaborator.
            if mojito_types::types::is_symbolic(&ty) {
                let dest = self.fresh_typed(span(e), None, Ty::StringLiteral);
                self.emit(MirInstr::TypeName { dest, ty });
                return dest;
            }
            return self.constant(e, Const::Str(text));
        }
        // A checked pointer construction materializes the frame/slot
        // handle for its source place; the checked pointer type keeps
        // the origin while the runtime value erases it.
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { .. }
            )
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
                mojito_checked::checked::SemanticAdjustment::UninitStorageMake { init, .. } => {
                    Some(init)
                }
                _ => None,
            })
        {
            let init = init
                .then(|| self.expr(args.first().expect("checked storage construction payload")));
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::UninitStorage { dest, init });
            return dest;
        }
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructVariant {
            alternatives,
            index,
        }) = self.checked_adjustments(e).into_iter().find(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ConstructVariant { .. }
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
        if let Some(mojito_checked::checked::SemanticAdjustment::ConstructVariantInitWith {
            alternatives,
            index,
        }) = self.checked_adjustments(e).into_iter().find(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ConstructVariantInitWith { .. }
            )
        }) {
            // Invoke the zero-parameter factory, then wrap its result:
            // the payload temporary is consumed by the construction.
            let factory = self.expr(
                &kwargs
                    .first()
                    .expect("checked Variant(init_with=) has one factory")
                    .value,
            );
            let payload = self.fresh_typed(
                span(e),
                None,
                alternatives.get(index).cloned().unwrap_or(Ty::Error),
            );
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
            });
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value: payload,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return dest;
        }
        // SIMD construction resolves its `[DType.<dt>, width]` parameters
        // here (the MIR is otherwise untyped about them).
        if let Some(r) = self.try_simd_call(e, args, kwargs) {
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
                kind: ExprKind::Identifier(name.to_owned()),
                span: e.span,
                source: e.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            };
            return self.lower_element_invocation(e, &receiver, plan, &indices, args, kwargs);
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
                kind: ExprKind::Identifier(name.to_owned()),
                span: e.span,
                source: e.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            });
            let callable_ty = self
                .vars
                .iter()
                .position(|candidate| candidate == name)
                .and_then(|variable| self.var_types.get(&(variable as VarId)))
                .cloned()
                .or_else(|| self.f.reg_types.get(&callee.0).cloned());
            let param_arg_regs = self.param_arg_regs(param_args, &span(e));
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
            self.install_call_transfers(e, transfer_recv_place.as_ref(), &transfer_arg_places);
            return dest;
        }
        // `__RuntimeTuple` is the compiler-private heterogeneous pack
        // storage primitive. Public `Tuple` is an ordinary nominal
        // variadic struct and follows the call path below.
        if name == "__RuntimeTuple" && kwargs.is_empty() && !self.overloads.is_function(name) {
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
        self.direct_call(e, name, param_args, args, kwargs)
    }

    pub(super) fn invoke_expr(
        &mut self,
        e: &Expr,
        callee: &Expr,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        if let Some(value) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::DtypeFloatQuery { value } => {
                    Some(value)
                }
                _ => None,
            })
        {
            return self.constant(e, Const::Int(value));
        }
        // `pointer.unsafe_origin_cast[...]()` retypes provenance only: the
        // runtime value is the receiver, unchanged, and the origin
        // parameter argument never lowers (origins erase).
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::PointerOriginCast { .. }
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
                mojito_checked::checked::SemanticAdjustment::SimdCast { dtype, width } => {
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
                width,
            });
            return dest;
        }
        // `v.to_bits[DType.<dt>]()` lowers like `cast`, as a lane-wise
        // bit reinterpretation of the member callee's object.
        let simd_to_bits = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdToBits { dtype, width } => {
                    Some((dtype, width))
                }
                _ => None,
            });
        if let Some((dtype, width)) = simd_to_bits
            && let ExprKind::Member { object, .. } = &callee.kind
        {
            let value = self.expr(object);
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::SimdBitcast {
                dest,
                value,
                dtype,
                width,
            });
            return dest;
        }
        let simd_shuffle = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask, joined } => {
                    Some((mask, joined))
                }
                _ => None,
            });
        if let Some((mask, joined)) = simd_shuffle
            && let ExprKind::Member { object, .. } = &callee.kind
        {
            let value = self.expr(object);
            let other = joined.then(|| self.expr(&args[0]));
            let dest = self.fresh(span(e), None);
            self.emit(MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            });
            return dest;
        }
        if let Some(operation) = self.checked_adjustments(e).into_iter().find(|adjustment| {
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
        }) {
            return self.variant_operation(e, callee, &operation, args, kwargs);
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
        if let Some(param_decls) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ParameterizedMethodCall {
                    param_decls,
                } => Some(param_decls),
                _ => None,
            })
        {
            return self.parameterized_method_call(
                e,
                callee,
                param_args,
                args,
                kwargs,
                param_decls,
            );
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
        let param_arg_regs = self.param_arg_regs(param_args, &span(e));
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

    /// The ordinary direct call: compile-time arguments, runtime arguments
    /// and keywords, then the `Call` on the checker-selected target.
    fn direct_call(
        &mut self,
        e: &Expr,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        // Compile-time parameter arguments (`Name[param_args](...)`),
        // evaluated before ordinary call arguments: a
        // **value** parameter is a comptime `Int` expression flattened to a
        // register; a **type** parameter is erased (`None`).
        let mut param_arg_regs = self.param_arg_regs(param_args, &span(e));
        param_arg_regs.extend(self.inferred_param_arg_regs(e));
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
            && (self.resolved_callable(e).as_deref() == Some("String")
                || self.checked_ty(e) == Some(Ty::StringLiteral));
        // A call's argument list anchors nested loan-carrying
        // temporaries unless another channel already carries their
        // loans (see `call_anchors_arguments`). A construction's
        // aggregate result additionally carries its arguments' loans
        // forward instead (its binding — or its own anchor one call
        // level up — installs them). The stringify conversion is the
        // exception: its result is a fresh owned string that carries
        // no loans, so a view argument anchors as in a plain call.
        let view_result = self.borrows_view_result(e);
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = (stringify
            || !matches!(
                self.checked_ty(e),
                Some(Ty::Struct(constructed, _)) if constructed == *name
            ))
            && self.call_anchors_arguments(e);
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
        let (regs, arg_places, kw, kwarg_places) = match self.checked_ty(e) {
            Some(Ty::Struct(constructed, _)) if constructed == *name => self
                .fieldwise_keywords_positional(&constructed, (regs, arg_places, kw, kwarg_places)),
            _ => (regs, arg_places, kw, kwarg_places),
        };
        // Builtin string producers wrapped by the nominal-String
        // conversion keep their own callee but type their register
        // as the compile-time string the wrap consumes.
        let literal_result = stringify
            || (matches!(name, "input" | "repr") && self.implicit_conversion(e).is_some());
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
        let instantiated_args = self.instantiated_args(e);
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
            receiver: None,
            instantiated_args,
            spread: args
                .iter()
                .position(|argument| matches!(argument.kind, ExprKind::Spread(_))),
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        self.install_call_transfers(e, None, &transfer_arg_places);
        d
    }

    /// A checked `Variant` operation spelled as a parameterized member call.
    fn variant_operation(
        &mut self,
        e: &Expr,
        callee: &Expr,
        operation: &mojito_checked::checked::SemanticAdjustment,
        args: &[Expr],
        kwargs: &[KwArg],
    ) -> Reg {
        let ExprKind::Member { object, .. } = &callee.kind else {
            unreachable!("checked Variant operation has a member callee")
        };
        match *operation {
            mojito_checked::checked::SemanticAdjustment::VariantIs { index, .. } => {
                let variant = self.expr(object);
                let dest = self.fresh(span(e), None);
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
                let dest = self.fresh(span(e), None);
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
                let value = self.expr(args.first().expect("checked Variant.set has one payload"));
                let dest = self.fresh(span(e), None);
                self.emit_interior_invalidations(e, None);
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
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantDeinitWith { index, .. } => {
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
                dest
            }
            mojito_checked::checked::SemanticAdjustment::VariantTake { index, checked, .. } => {
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
                dest
            }
            _ => unreachable!("filtered Variant operation"),
        }
    }

    /// A checked parameterized method call (`recv.method[params](args)`).
    fn parameterized_method_call(
        &mut self,
        e: &Expr,
        callee: &Expr,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[KwArg],
        param_decls: Vec<ParamDecl>,
    ) -> Reg {
        let ExprKind::Member { object, field } = &callee.kind else {
            unreachable!("checked parameterized method call has a member callee")
        };
        // A parameterized **static** call (`Bag[Int, String].has[Bool]()`,
        // `Lanes.ident[Int](3)`): the receiver is a type, so this
        // is a call on the checker-selected symbol with the
        // method's own compile-time arguments, exactly like the
        // unparameterized static spelling.
        if let Some(name) = self.type_receiver_name(object) {
            // A per-call clone declares no compile-time
            // parameters: the source type arguments are baked
            // into its symbol and occupy no slots.
            let param_arg_regs = if param_decls.is_empty() {
                Vec::new()
            } else {
                self.param_arg_regs(param_args, &span(e))
            };
            let saved_anchor_permission = self.allow_argument_anchors;
            self.allow_argument_anchors = self.call_anchors_arguments(e);
            let (regs, arg_places) = self.lower_call_arguments(args, false);
            self.allow_argument_anchors = saved_anchor_permission;
            let (kw, kwarg_places) = self.lower_call_keywords(kwargs, false);
            let dest = self.fresh(span(e), None);
            let target = self
                .resolved_callable(e)
                .unwrap_or_else(|| format!("{name}.{field}"));
            self.emit_call_invalidations(e, args, kwargs);
            let receiver = self.static_receiver(object);
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises: self.checked_raises(e),
                args: regs,
                kwargs: kw,
                arg_places,
                kwarg_places,
                capture_accesses: self.checked_call_capture_accesses(e),
                param_arg_regs,
                receiver,
                instantiated_args: Vec::new(),
                spread: None,
            });
            self.emit_nested_closure_argument_keepalives(args, kwargs);
            return dest;
        }
        // Keep this as a direct method invocation. In particular,
        // do not synthesize a bound-method value (which would make
        // its receiver/environment escapable).
        if let Some(dest) = self.lower_elided_receiver_call(e, object, param_args, args, kwargs) {
            return dest;
        }
        let (recv, recv_place) = self.lower_method_receiver(e, object);
        let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
        let recv = if implicitly_copied_receiver {
            self.copy_consuming_receiver(object, recv, recv_place.as_ref())
        } else {
            recv
        };
        // A per-call clone has baked its method parameters into
        // the selected symbol. Source brackets remain in the AST,
        // but an empty declaration list means they occupy no MIR
        // ABI slots.
        let param_arg_regs = if param_decls.is_empty() {
            Vec::new()
        } else {
            self.param_arg_regs(param_args, &span(e))
        };
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = self.call_anchors_arguments(e);
        let (argument_regs, arg_places) = self.lower_call_arguments(args, false);
        self.allow_argument_anchors = saved_anchor_permission;
        let (keyword_regs, kwarg_places) = self.lower_call_keywords(kwargs, false);
        let dest = self.fresh(span(e), None);
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
        dest
    }
}
