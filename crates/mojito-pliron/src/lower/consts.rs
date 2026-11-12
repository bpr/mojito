//! Constant and literal materialization plus unary operators.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use mojito_types::types::storage_simd;

impl FnLowering<'_> {
    pub(super) fn lower_const(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        k: &MirConst,
    ) -> Result<(), PlironError> {
        match k {
            MirConst::Int(value) => {
                let constant = self.int_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                self.int_consts.insert(dest.0, *value);
                Ok(())
            }
            MirConst::Float(value) => {
                let constant = self.float_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            MirConst::Bool(value) => {
                let constant = self.bool_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            MirConst::Dtype(dtype) => {
                let constant = self.dtype_constant(ctx, *dtype);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            // A numeric literal is zero-sized: its readers take the number
            // from the register's type ([`Self::register_literal`]).
            MirConst::IntLiteral(_) | MirConst::FloatLiteral(_) => {
                self.erased.insert(dest.0);
                Ok(())
            }
            // A string literal is zero-sized: its readers take the text
            // from the register's type ([`Self::string_constant`]).
            MirConst::Str(_) if self.register_string_literal(dest).is_some() => {
                self.erased.insert(dest.0);
                Ok(())
            }
            MirConst::Str(text) => {
                self.str_consts.insert(dest.0, text.as_bytes().to_vec());
                Ok(())
            }
            // The unit constant is zero-sized: consumers type it `None` and
            // never read a materialized value (`print` writes its constant
            // text, stores are no-ops).
            MirConst::None => {
                self.erased.insert(dest.0);
                Ok(())
            }
            // A bare function value is the two-word callable with a null
            // environment; its thunk ignores the environment argument.
            MirConst::Function(name) => self.lower_make_closure(ctx, dest, name, &[]),
            // The elaborator folds every parameter constant; concrete MIR
            // has none.
            MirConst::Param(value) => Err(self.unsupported_reg(
                format!("parameter constant `{value}` reached native lowering"),
                dest,
            )),
            // The elaborator names every function value's instance.
            MirConst::FunctionInstance { function, .. } => Err(self.unsupported_reg(
                format!("unelaborated instance of `{function}` reached native lowering"),
                dest,
            )),
            MirConst::Value(value) => self.lower_parameter_value(ctx, dest, value),
            MirConst::UninitMem => self.lower_uninit_mem(ctx, dest),
        }
    }

    /// `#interp.uninitmem` of the register's type, as `KGENToLLVM` lowers
    /// it: a scalar's `llvm.undef`, an aggregate's storage left unwritten,
    /// and nothing for a zero-sized value.
    fn lower_uninit_mem(&mut self, ctx: &mut Context, dest: Reg) -> Result<(), PlironError> {
        let ty = self
            .func
            .reg_types
            .get(&dest.0)
            .cloned()
            .ok_or_else(|| self.unsupported_reg("untyped `uninitmem`".to_string(), dest))?;
        // A vector is the poison vector its lanes are inserted into, as an
        // element list builds it, held in its own typed storage: a load of
        // never-written bytes would be a poison byte.
        if let Some((dtype, width)) = simd_dims(&ty).filter(|&(_, width)| width > 1) {
            let width = width as usize;
            let vector_ty = self.simd_vector_ty(ctx, dtype, width);
            let poison = PoisonOp::new(ctx, vector_ty);
            self.append(ctx, poison.get_operation(), Some(dest));
            self.simd_store_vector(ctx, dest, dtype, width, poison.get_result(ctx));
            return Ok(());
        }
        match lower_ty(
            self.name,
            &ty,
            &self.layout,
            self.mir_declarations,
            self.reg_span(dest),
        )? {
            LowerTy::Scalar(scalar) => {
                let undef = UndefOp::new(ctx, scalar.handle(ctx));
                self.append(ctx, undef.get_operation(), Some(dest));
                self.reg_values.insert(dest.0, undef.get_result(ctx));
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.reg_values.insert(dest.0, storage);
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
            }
        }
        Ok(())
    }

    /// The static address of a closed parameter list's elements: a private
    /// constant global holding each element at its native layout.
    pub(super) fn lower_param_list_address(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        values: &MirConst,
    ) -> Result<(), PlironError> {
        use mojito_types::ct::CtValue;
        let MirConst::Value(CtValue::Tuple(elements)) = values else {
            return Err(self.unsupported_reg(
                format!("parameter list `{values:?}` reached native lowering unclosed"),
                dest,
            ));
        };
        let register_ty = self.func.reg_types.get(&dest.0).cloned();
        let Some(TyKind::Pointer { element, .. }) = register_ty.as_ref().map(Ty::kind) else {
            return Err(self.unsupported_reg(
                "a parameter list's address needs a pointer register".to_string(),
                dest,
            ));
        };
        // A `!kgen.string` element is a descriptor over constant-pool bytes.
        if matches!(element.kind(), TyKind::KgenString)
            && let Some(strings) = elements
                .iter()
                .map(|value| match value {
                    CtValue::KgenString(text) => Some(text.as_bytes().to_vec()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
        {
            let global = self.shared.intern_kgen_string_list(ctx, &strings);
            let address = self.global_address(ctx, &global, dest);
            self.reg_values.insert(dest.0, address);
            return Ok(());
        }
        // A thin callable element is the `{ invoke, env }` value of its
        // function's thunk.
        if matches!(element.kind(), TyKind::Func(_)) {
            let mut thunks = Vec::with_capacity(elements.len());
            for value in elements {
                let target = match value {
                    // Concrete MIR's list holds instances by their bare
                    // symbol; the elaborator names every specialization.
                    CtValue::FuncSymbol(function) if function.is_bare() => {
                        self.signatures.get(function.symbol.as_str())
                    }
                    _ => None,
                };
                let Some(target) = target else {
                    return Err(self.unsupported_reg(
                        format!("a static parameter list of `{element}` elements (`{value}`)"),
                        dest,
                    ));
                };
                thunks.push(self.shared.ensure_thunk(ctx, target, 0));
            }
            let global = self.shared.intern_function_list(ctx, &thunks);
            let address = self.global_address(ctx, &global, dest);
            self.reg_values.insert(dest.0, address);
            return Ok(());
        }
        let mut bytes = Vec::new();
        for value in elements {
            let encoded = match (element.kind(), value) {
                (kind, CtValue::Int(value)) if kind.is_int() => Some(value.to_le_bytes().to_vec()),
                (kind, CtValue::IntLiteral(value)) if kind.is_int() => {
                    value.to_i64().map(|v| v.to_le_bytes().to_vec())
                }
                (kind, CtValue::UInt(value)) if kind.is_uint() => {
                    Some(value.to_le_bytes().to_vec())
                }
                (TyKind::Index, CtValue::Index(value)) => Some(value.to_le_bytes().to_vec()),
                (kind, CtValue::IntLiteral(value)) if kind.is_uint() => {
                    value.to_u64().map(|v| v.to_le_bytes().to_vec())
                }
                (kind, CtValue::Float(bits)) if kind.is_float64() => {
                    Some(bits.to_le_bytes().to_vec())
                }
                (kind, CtValue::FloatLiteral(value)) if kind.is_float64() => {
                    value.to_f64().map(|v| v.to_bits().to_le_bytes().to_vec())
                }
                (kind, CtValue::Bool(value)) if kind.is_bool() => Some(vec![u8::from(*value)]),
                (TyKind::KgenDtype, CtValue::Dtype(dtype)) => Some(vec![dtype.code()]),
                (_, value)
                    if mojito_types::types::is_dtype(element)
                        && let Some(dtype) = mojito_types::types::dtype_of_value(value) =>
                {
                    Some(vec![dtype.code()])
                }
                (ty, value) if let Some(dtype) = scalar_simd_dtype(ty) => {
                    mojito_types::ct::CtLane::from_value(value, dtype)
                        .map(|lane| scalar_lane_bytes(dtype, &lane))
                }
                _ => None,
            };
            let Some(encoded) = encoded else {
                return Err(self.unsupported_reg(
                    format!("a static parameter list of `{element}` elements (`{value}`)"),
                    dest,
                ));
            };
            bytes.extend(encoded);
        }
        let align = match element.kind() {
            kind if kind.is_bool() => 1,
            TyKind::KgenDtype => 1,
            _ if mojito_types::types::is_dtype(element) => 1,
            ty if let Some(dtype) = scalar_simd_dtype(ty) => lane_bytes(dtype) as u32,
            _ => 8,
        };
        let global = self.shared.intern_param_list(ctx, &bytes, align);
        let address = self.global_address(ctx, &global, dest);
        self.reg_values.insert(dest.0, address);
        Ok(())
    }

    /// A parameter value constant (`Self.key`, `Self.e`, a `comptime for`
    /// variable over pairs): a `UInt` as its scalar, a vector built lane by lane as its explicit
    /// construction is (a one-lane vector is its lane scalar), and an
    /// aggregate — or a string read at the nominal `String` — written
    /// fieldwise into fresh storage, each string leaf a fresh buffer. A
    /// pointer into frozen bytes (a literal's address) is the address of
    /// those bytes interned as a constant global.
    fn lower_parameter_value(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: &mojito_types::ct::CtValue,
    ) -> Result<(), PlironError> {
        use mojito_types::ct::CtValue;
        if self.held_parameter_value(ctx, dest, value)? {
            return Ok(());
        }
        match value {
            // The `NoneType` value is zero-sized, as `MirConst::None` is,
            // and so is a generator's.
            CtValue::None | CtValue::Generator(_) => {
                self.erased.insert(dest.0);
                Ok(())
            }
            CtValue::UInt(bits) => {
                let constant = self.parameter_scalar(ctx, value, ScalarTy::UInt, dest)?;
                self.reg_values.insert(dest.0, constant);
                self.int_consts.insert(dest.0, *bits as i64);
                Ok(())
            }
            CtValue::Int(n) | CtValue::Index(n) => {
                let constant = self.parameter_scalar(ctx, value, ScalarTy::Int, dest)?;
                self.reg_values.insert(dest.0, constant);
                self.int_consts.insert(dest.0, *n);
                Ok(())
            }
            CtValue::Simd { dtype, lanes } => {
                let vector = self.parameter_vector(ctx, *dtype, lanes, dest)?;
                // A width-1 SIMD register, or its storage, is its lane
                // scalar, as `lower_ty` lays it out.
                match self
                    .func
                    .reg_types
                    .get(&dest.0)
                    .map(mojito_types::types::storage_value_ty)
                {
                    Some(ty) if scalar_simd_dtype(ty.kind()).is_some() => {
                        self.reg_values.insert(dest.0, vector);
                    }
                    _ => self.simd_store_vector(ctx, dest, *dtype, lanes.len(), vector),
                }
                Ok(())
            }
            // A `!kgen.string` constant is its interned literal bytes.
            CtValue::KgenString(text) => {
                self.str_consts.insert(dest.0, text.as_bytes().to_vec());
                Ok(())
            }
            CtValue::Struct { .. }
            | CtValue::Tuple(_)
            | CtValue::Str(_)
            | CtValue::PopArray { .. } => {
                // A struct value is typed by its own name, a `#pop.array` by
                // its attribute type; a tuple, a string, and a pack's `{}`
                // (whose name is the `ParameterList`/`TypeList` template, its
                // list carried by the type) by the nominal instance their
                // register names.
                let ty = match value {
                    CtValue::Struct { name, .. }
                        if !mojito_types::types::ParamDecl::is_pack_value(value) =>
                    {
                        TyKind::Struct(name.clone(), Vec::new()).into()
                    }
                    CtValue::PopArray { .. } => value.pop_array_type().ok_or_else(|| {
                        self.unsupported_reg(format!("parameter value `{value}`"), dest)
                    })?,
                    _ => self.func.reg_types.get(&dest.0).cloned().ok_or_else(|| {
                        self.unsupported_reg(format!("untyped parameter value `{value}`"), dest)
                    })?,
                };
                let LowerTy::Aggregate { layout, .. } = lower_ty(
                    self.name,
                    &ty,
                    &self.layout,
                    self.mir_declarations,
                    self.reg_span(dest),
                )?
                else {
                    return Err(self.unsupported_reg(format!("parameter value `{value}`"), dest));
                };
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.store_parameter_value(ctx, storage, value, &ty, dest)?;
                self.reg_values.insert(dest.0, storage);
                Ok(())
            }
            CtValue::Pointer {
                memory: Some(slots),
                offset,
            } if let Some(bytes) = frozen_bytes(slots) => {
                let global = self.shared.intern_string(ctx, &bytes);
                let base = self.global_address(ctx, &global, dest);
                let pointer = if *offset == 0 {
                    base
                } else {
                    let bytes = self.int_constant(ctx, *offset);
                    let byte_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
                    let gep =
                        GetElementPtrOp::new(ctx, base, vec![GepIndex::Value(bytes)], byte_ty);
                    self.append(ctx, gep.get_operation(), Some(dest));
                    gep.get_result(ctx)
                };
                self.reg_values.insert(dest.0, pointer);
                Ok(())
            }
            _ => Err(self.unsupported_reg(
                format!("parameter value `{value}` reached native lowering"),
                dest,
            )),
        }
    }

    /// A frozen held struct (one field, `TrivialRegisterPassable`) lowered
    /// as its innermost field's value when its type classifies as a scalar
    /// or as zero-sized, as `lower_ty` holds it; `false` for any other value,
    /// which keeps its fieldwise storage.
    fn held_parameter_value(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: &mojito_types::ct::CtValue,
    ) -> Result<bool, PlironError> {
        use mojito_types::ct::CtValue;
        let CtValue::Struct { name, .. } = value else {
            return Ok(false);
        };
        let mut leaf = value;
        while let CtValue::Struct { name, fields } = leaf
            && self.mir_declarations.register_field(name).is_some()
            && let [(_, field)] = fields.as_slice()
        {
            leaf = field;
        }
        if std::ptr::eq(leaf, value) {
            return Ok(false);
        }
        let ty: Ty = TyKind::Struct(name.clone(), Vec::new()).into();
        match lower_ty(
            self.name,
            &ty,
            &self.layout,
            self.mir_declarations,
            self.reg_span(dest),
        )? {
            LowerTy::Scalar(scalar) => {
                let constant = self.parameter_scalar(ctx, leaf, scalar, dest)?;
                self.reg_values.insert(dest.0, constant);
                Ok(true)
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(true)
            }
            LowerTy::Aggregate { .. } => Ok(false),
        }
    }

    /// Store a frozen value of type `ty` at `address`: a struct's fields at
    /// its layout offsets, a nominal `Tuple`'s elements in its pack storage,
    /// and a scalar leaf as its constant.
    fn store_parameter_value(
        &mut self,
        ctx: &mut Context,
        address: Value,
        value: &mojito_types::ct::CtValue,
        ty: &Ty,
        dest: Reg,
    ) -> Result<(), PlironError> {
        use mojito_types::ct::CtValue;
        let (leaves, leaf_tys): (Vec<&CtValue>, Vec<Ty>) = match (value, ty.kind()) {
            (CtValue::Simd { dtype, lanes }, _) if lanes.len() > 1 => {
                let vector = self.parameter_vector(ctx, *dtype, lanes, dest)?;
                let store = StoreOp::new(ctx, vector, address);
                self.append(ctx, store.get_operation(), Some(dest));
                return Ok(());
            }
            (CtValue::Pointer { memory, offset }, TyKind::Pointer { element, .. }) => {
                let layout = self.layout.layout_of(element).map_err(|error| {
                    self.unsupported_reg(format!("frozen pointer element layout ({error})"), dest)
                })?;
                let base = if let Some(slots) = memory {
                    let bytes = layout.size.checked_mul(slots.len() as u64).ok_or_else(|| {
                        self.unsupported_reg(
                            "frozen pointer allocation size overflows".to_string(),
                            dest,
                        )
                    })?;
                    let bytes = self.uint_constant(ctx, bytes);
                    let base = self.emit_alloc(ctx, bytes, layout.align, dest);
                    for (index, slot) in slots.iter().enumerate() {
                        if let Some(slot) = slot {
                            let address =
                                self.gep_byte(ctx, base, index as u64 * layout.size, dest);
                            self.store_parameter_value(ctx, address, slot, element, dest)?;
                        }
                    }
                    base
                } else {
                    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                    let null = ZeroOp::new(ctx, ptr_ty);
                    self.append(ctx, null.get_operation(), Some(dest));
                    null.get_result(ctx)
                };
                let pointer = if *offset == 0 {
                    base
                } else {
                    let bytes = self.int_constant(ctx, offset.wrapping_mul(layout.size as i64));
                    let byte_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
                    let gep =
                        GetElementPtrOp::new(ctx, base, vec![GepIndex::Value(bytes)], byte_ty);
                    self.append(ctx, gep.get_operation(), Some(dest));
                    gep.get_result(ctx)
                };
                let store = StoreOp::new(ctx, pointer, address);
                self.append(ctx, store.get_operation(), Some(dest));
                return Ok(());
            }
            (CtValue::Str(text), TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                let global = self.shared.intern_string(ctx, text.as_bytes());
                let source = self.global_address(ctx, &global, dest);
                let length = self.uint_constant(ctx, text.len() as u64);
                let data = self.emit_alloc(ctx, length, 1, dest);
                self.mem_copy_dynamic(ctx, data, source, length, dest);
                self.store_string_fields(ctx, address, data, length, length, dest);
                return Ok(());
            }
            // A frozen enum case: its tag, then its payload, if any, at the
            // variant's payload offset.
            (CtValue::Variant { index, payload, .. }, TyKind::Variant(alternatives)) => {
                let layout = self.layout.variant_layout(alternatives).map_err(|error| {
                    self.unsupported_reg(format!("frozen variant layout ({error})"), dest)
                })?;
                let tag = self.tag_constant(ctx, *index as u32);
                let store = StoreOp::new(ctx, tag, address);
                self.append(ctx, store.get_operation(), Some(dest));
                if let (Some(payload), Some(payload_ty)) = (payload, alternatives.get(*index)) {
                    let payload_address = self.gep_byte(ctx, address, layout.payload_offset, dest);
                    self.store_parameter_value(ctx, payload_address, payload, payload_ty, dest)?;
                }
                return Ok(());
            }
            (CtValue::Struct { .. } | CtValue::Tuple(_), TyKind::Struct(name, _))
                if !ty.is_simd() =>
            {
                let Some(decl) = self.struct_decls.get(name.as_str()) else {
                    return Err(self.unsupported_reg(format!("parameter value of `{name}`"), dest));
                };
                let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, ty)| ty.clone()).collect();
                match (value, field_tys.as_slice()) {
                    (CtValue::Struct { fields, .. }, _) => {
                        (fields.iter().map(|(_, field)| field).collect(), field_tys)
                    }
                    // A `Tuple` is one field, its private pack storage.
                    (CtValue::Tuple(_), [storage]) => {
                        return self.store_parameter_value(ctx, address, value, storage, dest);
                    }
                    _ => {
                        return Err(
                            self.unsupported_reg(format!("parameter value `{value}`"), dest)
                        );
                    }
                }
            }
            (
                CtValue::Tuple(elements),
                TyKind::Tuple(element_tys) | TyKind::RuntimePack(element_tys),
            ) => (elements.iter().collect(), element_tys.clone()),
            // A `#pop.array`'s elements at its element stride.
            (CtValue::PopArray { elements, .. }, TyKind::PopArray { element, .. }) => (
                elements.iter().collect(),
                vec![element.clone(); elements.len()],
            ),
            _ => {
                let LowerTy::Scalar(scalar) = lower_ty(
                    self.name,
                    ty,
                    &self.layout,
                    self.mir_declarations,
                    self.reg_span(dest),
                )?
                else {
                    return Err(self.unsupported_reg(format!("parameter value `{value}`"), dest));
                };
                let constant = self.parameter_scalar(ctx, value, scalar, dest)?;
                let store = StoreOp::new(ctx, constant, address);
                self.append(ctx, store.get_operation(), Some(dest));
                return Ok(());
            }
        };
        let composed = self.struct_layout_of(&leaf_tys, dest)?;
        for ((leaf, leaf_ty), offset) in leaves.into_iter().zip(&leaf_tys).zip(&composed.offsets) {
            let leaf_address = if *offset == 0 {
                address
            } else {
                self.gep_byte(ctx, address, *offset, dest)
            };
            self.store_parameter_value(ctx, leaf_address, leaf, leaf_ty, dest)?;
        }
        Ok(())
    }

    fn parameter_vector(
        &mut self,
        ctx: &mut Context,
        dtype: Dtype,
        lanes: &[mojito_types::ct::CtLane],
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let lanes = self.parameter_lanes(ctx, dtype, lanes, dest)?;
        if let [lane] = lanes.as_slice() {
            return Ok(*lane);
        }
        let vector_ty = self.simd_vector_ty(ctx, dtype, lanes.len());
        let poison = PoisonOp::new(ctx, vector_ty);
        self.append(ctx, poison.get_operation(), Some(dest));
        let mut vector = poison.get_result(ctx);
        for (index, lane) in lanes.iter().enumerate() {
            let position = self.int_constant(ctx, index as i64);
            let insert = InsertElementOp::new(ctx, vector, *lane, position);
            self.append(ctx, insert.get_operation(), Some(dest));
            vector = insert.get_result(ctx);
        }
        Ok(vector)
    }

    /// One closed scalar of a parameter value at its storage type.
    fn parameter_scalar(
        &mut self,
        ctx: &mut Context,
        value: &mojito_types::ct::CtValue,
        scalar: ScalarTy,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        use mojito_types::ct::CtValue;
        match value {
            CtValue::Int(value) | CtValue::Index(value) => {
                self.materialize_exact(ctx, &ExactNum::Int((*value).into()), scalar, dest)
            }
            CtValue::UInt(value) => {
                self.materialize_exact(ctx, &ExactNum::Int((*value).into()), scalar, dest)
            }
            CtValue::IntLiteral(value) => {
                self.materialize_exact(ctx, &ExactNum::Int(value.clone()), scalar, dest)
            }
            CtValue::Float(bits) => self.materialize_exact(
                ctx,
                &ExactNum::Float(f64::from_bits(*bits).into()),
                scalar,
                dest,
            ),
            CtValue::FloatLiteral(value) => {
                self.materialize_exact(ctx, &ExactNum::Float(value.clone()), scalar, dest)
            }
            CtValue::Bool(value) => Ok(self.bool_constant(ctx, *value)),
            CtValue::Dtype(dtype) => Ok(self.dtype_constant(ctx, *dtype)),
            CtValue::Simd { dtype, lanes } if lanes.len() == 1 => self
                .parameter_lanes(ctx, *dtype, lanes, dest)
                .map(|lanes| lanes[0]),
            other => Err(self.unsupported_reg(format!("parameter value `{other}`"), dest)),
        }
    }

    /// The lanes of a closed vector parameter value at its dtype.
    fn parameter_lanes(
        &mut self,
        ctx: &mut Context,
        dtype: Dtype,
        lanes: &[mojito_types::ct::CtLane],
        dest: Reg,
    ) -> Result<Vec<Value>, PlironError> {
        use mojito_types::ct::CtLane;
        let target = ScalarTy::of_dtype(dtype);
        lanes
            .iter()
            .map(|lane| match lane {
                CtLane::Int(lane) => {
                    self.materialize_exact(ctx, &ExactNum::Int((*lane).into()), target, dest)
                }
                // A float lane is already rounded to its dtype, and may be
                // an infinity or a NaN a folded operator produced.
                CtLane::Float(bits) if dtype.is_narrow_float() => {
                    Ok(self.narrow_float_constant(ctx, dtype, f64::from_bits(*bits)))
                }
                CtLane::Float(bits) => Ok(self.float_constant(ctx, f64::from_bits(*bits))),
                CtLane::Bool(lane) => Ok(self.bool_constant(ctx, *lane)),
            })
            .collect()
    }

    pub(super) fn lower_materialize(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        target_ty: &Ty,
    ) -> Result<(), PlironError> {
        // A vector storage converts the literal at its lane and splats it,
        // as `#pop.int_literal_convert` at a `!kgen.simd<w, dt>` folds.
        if let Some((dtype, width)) = simd_dims(target_ty)
            && let Some(width) = usize::try_from(width).ok().filter(|width| *width > 1)
        {
            return self.lower_make_simd(ctx, dest, dtype, width, &[value]);
        }
        let target = match storage_simd(target_ty).as_ref().map(Ty::kind) {
            Some(kind) if kind.is_int() => ScalarTy::Int,
            Some(kind) if kind.is_uint() => ScalarTy::UInt,
            Some(kind) if kind.is_float64() => ScalarTy::Float64,
            Some(ty) if let Some(dtype) = scalar_simd_dtype(ty) => ScalarTy::of_dtype(dtype),
            _ => {
                return Err(
                    self.unsupported_reg(format!("literal materialization to `{target_ty}`"), dest)
                );
            }
        };
        let literal = self.register_literal(value).ok_or_else(|| {
            self.unsupported_reg(
                "literal materialization of a non-literal register".into(),
                dest,
            )
        })?;
        let constant = self.materialize_exact(ctx, &literal, target, dest)?;
        self.reg_values.insert(dest.0, constant);
        Ok(())
    }

    /// The number the numeric literal register `reg` holds, which its
    /// closed `IntLiteral[v]` or `FloatLiteral[v]` type carries: the
    /// register itself is zero-sized, as upstream's literal struct is.
    /// `None` for a register of any other type.
    pub(super) fn register_literal(&self, reg: Reg) -> Option<ExactNum> {
        self.func
            .reg_types
            .get(&reg.0)
            .and_then(mojito_vm::runtime::literal_of)
    }

    /// The text the string literal register `reg` holds, which its closed
    /// `StringLiteral[v]` type carries: the register itself is zero-sized,
    /// as upstream's literal struct is. `None` for a register of any other
    /// type.
    pub(super) fn register_string_literal(&self, reg: Reg) -> Option<&str> {
        self.func
            .reg_types
            .get(&reg.0)
            .and_then(|ty| ty.kind().string_literal_text())
    }

    /// The bytes of the string constant `reg` is: a string literal's text,
    /// which its type carries ([`Self::register_string_literal`]), or a
    /// `!kgen.string` constant's. Its readers intern the bytes only where
    /// they use them.
    pub(super) fn string_constant(&self, reg: Reg) -> Option<Vec<u8>> {
        self.register_string_literal(reg)
            .map(|text| text.as_bytes().to_vec())
            .or_else(|| self.str_consts.get(&reg.0).cloned())
    }

    /// Whether `reg` is a string constant ([`Self::string_constant`]).
    pub(super) fn is_string_constant(&self, reg: Reg) -> bool {
        self.register_string_literal(reg).is_some() || self.str_consts.contains_key(&reg.0)
    }

    /// Whether `reg` is a numeric literal register, closed or not.
    pub(super) fn is_literal_register(&self, reg: Reg) -> bool {
        self.func
            .reg_types
            .get(&reg.0)
            .is_some_and(Ty::is_numeric_literal)
    }

    /// An operator over two numeric literals: the zero-sized literal its
    /// result type carries, or the `Bool` its operands' types decide, as
    /// upstream's literal dunders read their `value` parameters.
    pub(super) fn lower_literal_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        if self.is_literal_register(dest) {
            self.erased.insert(dest.0);
            return Ok(());
        }
        let decided = self
            .register_literal(a)
            .zip(self.register_literal(b))
            .and_then(|(left, right)| left.compare_by(op, &right))
            .ok_or_else(|| {
                self.unsupported_reg(format!("operator `{op:?}` over literals"), dest)
            })?;
        let value = self.bool_constant(ctx, decided);
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    pub(super) fn lower_unop(
        &mut self,
        ctx: &mut Context,
        op: PrefixOp,
        dest: Reg,
        a: Reg,
    ) -> Result<(), PlironError> {
        if let Some((dtype, width)) = self.func.reg_types.get(&a.0).and_then(simd_dims)
            && width > 1
        {
            return self.lower_simd_unop(ctx, op, dest, a, dtype, width as usize);
        }
        // An operator over a numeric literal yields the literal its result
        // type carries, or the `Bool` of its `not`.
        if let Some(literal) = self.register_literal(a) {
            if self.is_literal_register(dest) {
                self.erased.insert(dest.0);
                return Ok(());
            }
            if op == PrefixOp::Not {
                let value = self.bool_constant(ctx, !literal.is_nonzero());
                self.reg_values.insert(dest.0, value);
                return Ok(());
            }
            return Err(self.unsupported_reg(format!("operator `{op:?}` on a literal"), dest));
        }
        let operand_ty = self.binop_operand_ty(a, a)?;
        let value = self.operand_value(ctx, a, operand_ty)?;
        match (op, operand_ty) {
            (PrefixOp::Neg, ScalarTy::Int) => {
                let zero = self.int_constant(ctx, 0);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Neg, ScalarTy::Float64) => {
                let neg =
                    FNegOp::new_with_fast_math_flags(ctx, value, FastmathFlagsAttr::default());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Not | PrefixOp::Invert, ScalarTy::Bool) => {
                let one = self.bool_constant(ctx, true);
                let not = XorOp::new(ctx, value, one);
                self.define(ctx, dest, not.get_operation(), not.get_result(ctx))
            }
            // Sized-lane negation: `0 - x` wraps at the lane width for
            // integers; narrow float negation is exact, so no widen/round
            // dance.
            (PrefixOp::Neg, ScalarTy::Sized(dtype)) if dtype.is_narrow_float() => {
                let neg =
                    FNegOp::new_with_fast_math_flags(ctx, value, FastmathFlagsAttr::default());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Neg, ScalarTy::Sized(dtype)) => {
                let zero = self.sized_int_constant(ctx, dtype, 0);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (op, other) => Err(self.unsupported_reg(
                format!("operator `{op:?}` on `{}` operand", other.name()),
                dest,
            )),
        }
    }
}

/// The bytes one lane of `dtype` occupies in memory (a `bool` lane is a
/// byte).
const fn lane_bytes(dtype: Dtype) -> usize {
    (dtype.bit_width() as usize).div_ceil(8)
}

/// One scalar lane of `dtype` at its native layout, little-endian, the bits
/// the VM keeps for it: an integer wrapped to the lane, a float rounded to
/// it.
fn scalar_lane_bytes(dtype: Dtype, lane: &mojito_types::ct::CtLane) -> Vec<u8> {
    use mojito_types::ct::CtLane;
    let bits = match lane {
        CtLane::Int(value) => *value as u64,
        CtLane::Float(bits) => match dtype {
            Dtype::Float16 => u64::from(mojito_common::float::f16_bits(f64::from_bits(*bits))),
            Dtype::Float32 => u64::from((f64::from_bits(*bits) as f32).to_bits()),
            _ => *bits,
        },
        CtLane::Bool(value) => u64::from(*value),
    };
    bits.to_le_bytes()[..lane_bytes(dtype)].to_vec()
}

/// The bytes a frozen pointer's memory holds when every slot is an
/// initialized `UInt8` lane, as a string literal's are.
fn frozen_bytes(slots: &[Option<mojito_types::ct::CtValue>]) -> Option<Vec<u8>> {
    use mojito_types::ct::{CtLane, CtValue};
    slots
        .iter()
        .map(|slot| match slot {
            Some(CtValue::Simd {
                dtype: mojito_ast::ast::Dtype::UInt8,
                lanes,
            }) if let [CtLane::Int(byte)] = lanes.as_slice() => u8::try_from(*byte).ok(),
            _ => None,
        })
        .collect()
}
