//! Display: stdout writes (the `input` prompt), intrinsic scalar
//! formatting, and string comparison operators.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl FnLowering<'_> {
    /// Write the UTF-8 bytes of a string-valued register to stdout when the
    /// register holds one of the supported string shapes — a string
    /// constant (a literal's text its type carries), a runtime string pair,
    /// or a nominal String or `StringSpan`. Returns whether the register was such a string.
    pub(super) fn try_write_string_bytes(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<bool, PlironError> {
        if let Some(bytes) = self.string_constant(arg) {
            self.write_literal_bytes(ctx, &bytes, dest);
            return Ok(true);
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            self.write_stdout(ctx, descriptor.data, descriptor.len, dest);
            return Ok(true);
        }
        // A nominal String's byte buffer (the VM's `write_to` bridge reads
        // the same bytes), or a `StringSpan` view's (`{data, size}` at the
        // same offsets).
        let is_string = self
            .func
            .reg_types
            .get(&arg.0)
            .and_then(|ty| ty.kind().as_non_simd_struct())
            .is_some_and(|(name, _)| {
                mojito_symbol::symbol::is_stdlib_string_struct(name)
                    || mojito_types::types::is_stdlib_string_span_struct(name)
            });
        if is_string {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(true);
        }
        Ok(false)
    }

    /// The display bytes of one `DType` value, or with `repr` its
    /// representation bytes, as a `(data, len)` pair over interned constants. A numeric value is a `SIMD` one and a `Bool` the
    /// declared struct, which display through their declared members
    /// ([`Self::declared_display_text`]).
    pub(super) fn format_scalar(
        &mut self,
        ctx: &mut Context,
        ty: ScalarTy,
        value: Value,
        repr: bool,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        match ty {
            ScalarTy::Dtype => Ok(self.dtype_text(ctx, value, repr, dest)),
            ScalarTy::Ptr => Err(self.unsupported_reg("display of a Pointer".into(), dest)),
            ScalarTy::Bool => {
                Err(self.unsupported_reg("display of a `Bool` value without its type".into(), dest))
            }
            ScalarTy::Int | ScalarTy::UInt | ScalarTy::Float64 | ScalarTy::Sized(_) => {
                Err(self.unsupported_reg("display of a `SIMD` value without its type".into(), dest))
            }
        }
    }

    /// A `DType`'s display text — its name, or with `repr` its `DType.<name>`
    /// spelling — as a `(data, len)` pair over interned constants, selected
    /// by comparing the code against each dtype's.
    pub(super) fn dtype_text(
        &mut self,
        ctx: &mut Context,
        code: Value,
        repr: bool,
        dest: Reg,
    ) -> (Value, Value) {
        let text = |dtype: Dtype| {
            if repr {
                format!("DType.{}", dtype.name())
            } else {
                dtype.name().to_string()
            }
        };
        let [first, rest @ ..] = Dtype::ALL;
        let first_text = text(first);
        let global = self.shared.intern_string(ctx, first_text.as_bytes());
        let mut data = self.global_address(ctx, &global, dest);
        let mut len = self.uint_constant(ctx, first_text.len() as u64);
        for dtype in rest {
            let candidate = text(dtype);
            let global = self.shared.intern_string(ctx, candidate.as_bytes());
            let candidate_data = self.global_address(ctx, &global, dest);
            let candidate_len = self.uint_constant(ctx, candidate.len() as u64);
            let expected = self.dtype_constant(ctx, dtype);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, code, expected);
            self.append(ctx, matches.get_operation(), Some(dest));
            let matches = matches.get_result(ctx);
            let selected_data = SelectOp::new(ctx, matches, candidate_data, data);
            self.append(ctx, selected_data.get_operation(), Some(dest));
            data = selected_data.get_result(ctx);
            let selected_len = SelectOp::new(ctx, matches, candidate_len, len);
            self.append(ctx, selected_len.get_operation(), Some(dest));
            len = selected_len.get_result(ctx);
        }
        (data, len)
    }

    /// Intern `bytes` and write them to stdout.
    pub(super) fn write_literal_bytes(&mut self, ctx: &mut Context, bytes: &[u8], dest: Reg) {
        let global = self.shared.intern_string(ctx, bytes);
        self.write_global(ctx, &global, bytes.len() as u64, dest);
    }

    /// Write `len` bytes of a constant-pool global to stdout.
    pub(super) fn write_global(
        &mut self,
        ctx: &mut Context,
        global: &Identifier,
        len: u64,
        dest: Reg,
    ) {
        let data = self.global_address(ctx, global, dest);
        let len = self.uint_constant(ctx, len);
        self.write_stdout(ctx, data, len, dest);
    }

    /// `mjrt_write_stdout(data, len)` — writes exactly the given bytes or
    /// traps (category 4).
    pub(super) fn write_stdout(&mut self, ctx: &mut Context, data: Value, len: Value, dest: Reg) {
        let write_ty = self.shared.ensure_rt(ctx, "mjrt_write_stdout");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_write_stdout".try_into().expect("valid identifier")),
            write_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(dest));
    }

    /// The address of a module global in the current block.
    pub(super) fn global_address(
        &mut self,
        ctx: &mut Context,
        global: &Identifier,
        dest: Reg,
    ) -> Value {
        let address = AddressOfOp::new(ctx, global.clone(), 0);
        self.append(ctx, address.get_operation(), Some(dest));
        address.get_result(ctx)
    }

    /// Runtime string equality: equal lengths and equal bytes, via an inline
    /// byte-compare loop over slot-backed state (mem2reg promotes it).
    pub(super) fn lower_str_runtime_eq(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (a_data, a_len) = self.string_bytes(ctx, a, dest)?;
        let (b_data, b_len) = self.string_bytes(ctx, b, dest)?;
        let i1_handle: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i8_handle: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let result_slot = self.entry_typed_alloca(ctx, i1_handle);
        let index_slot = self.entry_typed_alloca(ctx, i64_handle);
        let len_eq = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, a_len, b_len);
        self.append(ctx, len_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, len_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let region = self.region.expect("lowering is inside a function");
        let head = BasicBlock::new(ctx, None, vec![]);
        head.insert_at_back(region, ctx);
        let body = BasicBlock::new(ctx, None, vec![]);
        body.insert_at_back(region, ctx);
        let done = BasicBlock::new(ctx, None, vec![]);
        done.insert_at_back(region, ctx);
        let enter = BrOp::new(ctx, head, vec![]);
        self.append(ctx, enter.get_operation(), Some(dest));
        // head: continue while `index < len` and no mismatch was found.
        self.current = Some(head);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let in_range = ICmpOp::new(ctx, ICmpPredicateAttr::ULT, index.get_result(ctx), a_len);
        self.append(ctx, in_range.get_operation(), Some(dest));
        let live = AndOp::new(ctx, in_range.get_result(ctx), result.get_result(ctx));
        self.append(ctx, live.get_operation(), Some(dest));
        let branch = CondBrOp::new(ctx, live.get_result(ctx), body, vec![], done, vec![]);
        self.append(ctx, branch.get_operation(), Some(dest));
        // body: compare one byte, fold into the result, advance.
        self.current = Some(body);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let a_byte_ptr = GetElementPtrOp::new(
            ctx,
            a_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, a_byte_ptr.get_operation(), Some(dest));
        let a_byte = LoadOp::new(ctx, a_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, a_byte.get_operation(), Some(dest));
        let b_byte_ptr = GetElementPtrOp::new(
            ctx,
            b_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, b_byte_ptr.get_operation(), Some(dest));
        let b_byte = LoadOp::new(ctx, b_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, b_byte.get_operation(), Some(dest));
        let byte_eq = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            a_byte.get_result(ctx),
            b_byte.get_result(ctx),
        );
        self.append(ctx, byte_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, byte_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let next =
            AddOp::new_with_overflow_flag(ctx, index.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, next.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, next.get_result(ctx), index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let advance = BrOp::new(ctx, head, vec![]);
        self.append(ctx, advance.get_operation(), Some(dest));
        // done: the folded verdict, negated for `!=`.
        self.current = Some(done);
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let mut value = result.get_result(ctx);
        if matches!(op, InfixOp::Ne) {
            let truth = self.bool_constant(ctx, true);
            let flipped = XorOp::new(ctx, value, truth);
            self.append(ctx, flipped.get_operation(), Some(dest));
            value = flipped.get_result(ctx);
        }
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// Compile-time folding of the string operators the VM evaluates on
    /// `Value::Str`: `+` yields the literal its result type carries (or a
    /// new `!kgen.string` constant), and `==` and `!=` fold to Bool
    /// constants. Both operands are string constants.
    pub(super) fn lower_str_literal_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (Some(lhs), Some(rhs)) = (self.string_constant(a), self.string_constant(b)) else {
            return Err(self.unsupported_reg("runtime StringLiteral operand".into(), dest));
        };
        match op {
            InfixOp::Add => {
                if self.register_string_literal(dest).is_some() {
                    self.erased.insert(dest.0);
                } else {
                    let mut bytes = lhs;
                    bytes.extend_from_slice(&rhs);
                    self.str_consts.insert(dest.0, bytes);
                }
                Ok(())
            }
            InfixOp::Eq | InfixOp::Ne => {
                let equal = lhs == rhs;
                let value = if matches!(op, InfixOp::Eq) {
                    equal
                } else {
                    !equal
                };
                let constant = self.bool_constant(ctx, value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            other => Err(self.unsupported_reg(
                format!("operator `{other:?}` on StringLiteral operands"),
                dest,
            )),
        }
    }
}
