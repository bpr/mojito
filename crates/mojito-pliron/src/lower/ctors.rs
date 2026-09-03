//! Constructor and `String` constructor/builtin lowering, plus
//! `MakeRef`/`ReadRef`/`WriteRef`.

use super::*;

impl<'a> FnLowering<'a> {
    /// A constructor call to declared struct `name`: the fieldwise copy form
    /// (`Type(copy=value)`), the compiled `__init__` overload with fresh
    /// storage as its `out self`, or fieldwise per-field stores — the VM's
    /// dispatch order.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_constructor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<(), PlironError> {
        if mojito_symbol::symbol::is_stdlib_string_struct(name) {
            return self.lower_string_ctor(ctx, dest, args, kwargs);
        }
        let struct_ty = match self.func.reg_types.get(&dest.0) {
            Some(ty @ Ty::Struct(..)) => ty.clone(),
            _ => Ty::Struct(name.to_string(), Vec::new()),
        };
        let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
        let LowerTy::Aggregate { ty, layout } = lowered else {
            return Err(self.unsupported_reg(format!("constructor for `{name}`"), dest));
        };
        if args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy" {
            let source_reg = kwargs[0].1;
            let source_place = kwarg_places
                .first()
                .and_then(Option::as_ref)
                .cloned()
                .or_else(|| self.loaded_places.get(&source_reg.0).cloned());
            let src = if let Some(place) = source_place {
                self.place_address(ctx, &place, dest)?.0
            } else {
                self.reg_ptr(ctx, source_reg)?
            };
            return self.copy_aggregate(ctx, dest, &ty, layout, src);
        }
        if let Some(init) = self.constructor_init(name, args.len()) {
            let params = self.signatures[&init].params.clone();
            let owned = self.signatures[&init].owned_params.clone();
            let by_reference = self.signatures[&init].ref_params.clone();
            if params.is_empty() {
                return Err(
                    self.unsupported_reg(format!("`{init}` without an `out self` parameter"), dest)
                );
            }
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let rest = &params[1..];
            let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
            let rest_by_reference = if by_reference.len() > 1 {
                &by_reference[1..]
            } else {
                &[]
            };
            let mut lowered = vec![storage];
            if kwargs.is_empty()
                && args.len() == rest.len()
                && !rest_by_reference.iter().any(|&by_ref| by_ref)
            {
                for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                    let owned = rest_owned.get(i).copied().unwrap_or(false);
                    lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
                }
            } else {
                lowered.extend(self.bind_call_slots(
                    ctx,
                    dest,
                    &init,
                    rest,
                    rest_owned,
                    rest_by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?);
            }
            self.emit_bound_call(ctx, dest, &init, lowered)?;
            // `__init__` returns nothing; the constructed value is the
            // storage its `out self` wrote through.
            self.erased.remove(&dest.0);
            self.reg_values.insert(dest.0, storage);
            // The constructed value owns its heap: consumers relocate it
            // (`DefVar`, stores) and a discarded result releases invisibly.
            // Without the mark, a store forks the value and the original's
            // buffers lose their releasing owner. A cross-block lifetime
            // keeps the pre-existing shared-bytes behavior instead of
            // rejecting.
            if self.owns_heap(&ty)
                && self
                    .last_uses
                    .get(&dest.0)
                    .is_none_or(|(block, _)| *block == self.position.0)
            {
                self.mark_owned_temp(dest, (*ty).clone())?;
            }
            return Ok(());
        }
        let decl = self.struct_decls[name];
        if !decl.fieldwise_init {
            return Err(self.unsupported_reg(
                format!("constructor for `{name}` without a compiled `__init__`"),
                dest,
            ));
        }
        if !kwargs.is_empty() {
            return Err(self.unsupported_reg(
                format!("keyword arguments in the fieldwise constructor of `{name}`"),
                dest,
            ));
        }
        if args.len() != decl.fields.len() {
            return Err(self.unsupported_reg(
                format!(
                    "fieldwise constructor of `{name}` expects {} arguments, got {}",
                    decl.fields.len(),
                    args.len()
                ),
                dest,
            ));
        }
        let fields: Vec<(String, Ty)> = decl.fields.clone();
        let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
        let composed = self.struct_layout_of(&field_tys, dest)?;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for ((arg, field_ty), offset) in args.iter().zip(&field_tys).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, field_ty, *arg)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// The nominal String constructor — the VM's literal-to-struct bridge
    /// (`materialize_string_struct`): the stdlib body never executes; the
    /// byte buffer fills from the string source instead. A compile-time
    /// literal copies out of the constant pool; an owned runtime string's
    /// allocation is stolen; a borrowed runtime string is copied.
    pub(super) fn lower_string_ctor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy" {
            // `String(copy=value)` deep-copies through the native bridge, the
            // VM's `construct_via_copy` over the stdlib copy constructor.
            let ty = Ty::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                vec![],
            );
            let lowered = lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))?;
            let LowerTy::Aggregate { ty, layout } = lowered else {
                return Err(self.unsupported_reg("String copy layout".into(), dest));
            };
            let source_reg = kwargs[0].1;
            let source_place = self.loaded_places.get(&source_reg.0).cloned();
            let src = if let Some(place) = source_place {
                self.place_address(ctx, &place, dest)?.0
            } else {
                self.reg_ptr(ctx, source_reg)?
            };
            return self.copy_aggregate(ctx, dest, &ty, layout, src);
        }
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("String constructor contract".into(), dest));
        }
        let source = args[0];
        let storage = self.entry_alloca(ctx, 24, 8);
        if let Some(bytes) = self.str_consts.get(&source.0).cloned() {
            let len = bytes.len() as u64;
            let global = self.shared.intern_string(ctx, &bytes);
            let len_value = self.uint_constant(ctx, len);
            let data = self.emit_alloc(ctx, len_value, 1, dest);
            if len > 0 {
                let literal = self.global_address(ctx, &global, dest);
                self.mem_copy(ctx, data, literal, len, dest);
            }
            self.store_string_fields(ctx, storage, data, len_value, len_value, dest);
        } else if let Some(descriptor) = self.str_runtime.get(&source.0).copied() {
            let data = if descriptor.owned && self.owned_temps.remove(&source.0).is_some() {
                // Steal the dedicated allocation — the temporary transfers
                // into the String.
                descriptor.data
            } else {
                let data = self.emit_alloc(ctx, descriptor.len, 1, dest);
                self.mem_copy_dynamic(ctx, data, descriptor.data, descriptor.len, dest);
                data
            };
            self.store_string_fields(ctx, storage, data, descriptor.len, descriptor.len, dest);
        } else if matches!(self.func.reg_types.get(&source.0), Some(Ty::StringLiteral)) {
            // A runtime StringLiteral value (typed storage): copy the bytes
            // its borrowed descriptor points at.
            let ptr = self.reg_ptr(ctx, source)?;
            let (src_data, len) = self.string_parts(ctx, ptr, dest);
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, src_data, len, dest);
            self.store_string_fields(ctx, storage, data, len, len, dest);
        } else {
            return Err(
                self.unsupported_reg("String constructor over an unsupported source".into(), dest)
            );
        }
        self.reg_values.insert(dest.0, storage);
        let ty = self
            .func
            .reg_types
            .get(&dest.0)
            .cloned()
            .unwrap_or_else(|| {
                Ty::Struct(
                    mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                    vec![],
                )
            });
        self.mark_owned_temp(dest, ty)?;
        Ok(())
    }

    /// The `String(x)` builtin — the VM's `format_value` over one argument:
    /// a literal stays compile-time; scalars format through `mjrt_fmt_*`
    /// into a dedicated allocation (an owned runtime string); a nominal
    /// String reads back as a borrowed runtime string.
    pub(super) fn lower_string_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("String conversion contract".into(), dest));
        }
        let arg = args[0];
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.str_consts.insert(dest.0, bytes);
            return Ok(());
        }
        if let Some(ty) = self.func.reg_types.get(&arg.0)
            && let Ty::Struct(name, _) = ty
            && mojito_symbol::symbol::is_stdlib_string_struct(name)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: false,
                },
            );
            return Ok(());
        }
        // A runtime StringLiteral value reads back as a borrowed string.
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::StringLiteral))
            && !self.pending_literals.contains_key(&arg.0)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: false,
                },
            );
            return Ok(());
        }
        // A nominal struct converts through its `write_to` conformance over
        // a fresh accumulator — the VM's `format_value` struct arm. The
        // accumulated buffer transfers into the resulting owned string.
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
            && !mojito_symbol::symbol::is_stdlib_string_struct(&name)
        {
            let writer = self.entry_alloca(ctx, 16, 8);
            self.mem_zero(ctx, writer, 16);
            self.append_struct_via_write_to(ctx, arg, &name, writer, dest)?;
            let (data, len) = self.string_parts(ctx, writer, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            // The accumulator alloca is exactly the 16-byte `MjStrDesc`
            // StringLiteral storage; aggregate consumers read it directly.
            self.reg_values.insert(dest.0, writer);
            self.mark_owned_temp(dest, Ty::StringLiteral)?;
            return Ok(());
        }
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A runtime FloatLiteral value rejects — the VM formats its
            // exact rational, which f64 storage cannot reproduce.
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => {
                    if !self.pending_literals.contains_key(&arg.0) {
                        return Err(self.unsupported_reg(
                            "String conversion of a runtime FloatLiteral value".into(),
                            dest,
                        ));
                    }
                    ScalarTy::Float64
                }
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, arg, ty)?;
        let (text, len) = self.format_scalar(ctx, ty, value, dest)?;
        let data = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, data, text, len, dest);
        self.str_runtime.insert(
            dest.0,
            RuntimeStr {
                data,
                len,
                owned: true,
            },
        );
        self.mark_owned_temp(dest, Ty::StringLiteral)?;
        Ok(())
    }

    /// The `Error(x)` builtin. Before Stage 4's tagged outcomes the only
    /// consumer is `Raise`, which reads the message bytes and exits — so an
    /// error value lowers as its message string pair.
    /// The lowered function-return value kind: the reference pointer for a
    /// reference-returning function, else the checked return type's lowering.
    pub(super) fn return_value_lower(&self) -> Result<Option<LowerTy>, PlironError> {
        if self.func.returns_reference {
            return Ok(Some(LowerTy::Scalar(ScalarTy::Ptr)));
        }
        match self.func.ret_ty.as_ref() {
            Some(Ty::None) | None => Ok(None),
            Some(other) => Ok(Some(lower_ty(self.name, other, &self.layout, None)?)),
        }
    }

    /// `MakeRef`: materialize a reference to a verified place — its address.
    /// A place through a local reference forwards (and extends) the stored
    /// handle.
    pub(super) fn lower_make_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
    ) -> Result<(), PlironError> {
        let place = place.clone();
        if matches!(place.proj.last(), Some(Proj::UninitPayload)) {
            let mut storage_place = place.clone();
            storage_place.proj.pop();
            let (storage, _) = self.place_address(ctx, &storage_place, dest)?;
            self.guard_uninit_present(ctx, storage, TrapCategory::UninitRead, dest)?;
        }
        let (address, ty) = self.place_address(ctx, &place, dest)?;
        // A bare reference-typed variable re-borrows: its slot stores a
        // handle (reference slots always hold real referent addresses), and
        // the made reference is that stored handle, collapsing the chain
        // like the VM's recursive `Value::Ref` reads. A projected place
        // whose designated element is itself a reference (a `List[ref T]`
        // element) instead addresses the slot — its consumers dereference
        // explicitly.
        // A projected place ENDING at a stored reference also forwards when
        // the destination is typed as the stored handle itself (`ref s =
        // self.src` reborrows): the ref-field slot holds a real referent
        // address, so the handle is the loaded slot value, not the slot
        // address. A storage-borrow destination (`ref (ref T)`) keeps the
        // slot address; its consumers dereference explicitly.
        let forwards_stored_handle = !place.proj.is_empty()
            && match (&ty, self.func.reg_types.get(&dest.0)) {
                (Ty::Ref(stored), Some(Ty::Ref(dest_ref))) => dest_ref.referent == stored.referent,
                _ => false,
            };
        if (place.proj.is_empty() || forwards_stored_handle)
            && let Ty::Ref(reference) = &ty
        {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            if matches!(*reference.referent, Ty::Pointer { .. }) {
                self.pointer_slot_refs.insert(dest.0);
            }
            self.reg_values.insert(dest.0, load.get_result(ctx));
            return Ok(());
        }
        if matches!(ty, Ty::Pointer { .. }) {
            self.pointer_slot_refs.insert(dest.0);
        }
        self.reg_values.insert(dest.0, address);
        Ok(())
    }

    /// `ReadRef`: read the referent behind a handle. The read itself is a
    /// plain structural snapshot; an explicit following `CopyValue` owns
    /// user-visible copy construction. Keeping those operations distinct
    /// prevents reference adapters from invoking `__copyinit__` twice.
    pub(super) fn lower_read_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        reference: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // A handle addressing pointer-typed storage dereferences through the
        // stored pointer (the VM's reference-pointer boundary).
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference read".into(), dest));
        };
        self.load_moved_from(ctx, pointer, &ty, dest)
    }

    /// `WriteRef`: write a value through a handle into the referent storage.
    pub(super) fn lower_write_ref(
        &mut self,
        ctx: &mut Context,
        reference: Reg,
        value: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // See `lower_read_ref`: a handle addressing pointer-typed storage
        // dereferences through the stored pointer.
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(value));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference write".into(), value));
        };
        self.store_to(ctx, pointer, &ty, value)
    }
}
