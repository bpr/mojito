//! Instruction operand decoding: places, loans, captures, call and
//! subscript arguments, registers, and constants.

use super::*;

impl Decoder {
    pub(super) fn use_mode(&mut self, value: &Value) -> Option<UseMode> {
        match self.atom(value)? {
            "copy" => Some(UseMode::Copy),
            "move" => Some(UseMode::Move),
            "borrow_shared" => Some(UseMode::BorrowShared),
            "borrow_mut" => Some(UseMode::BorrowMut),
            other => {
                self.error(value.span, format!("unknown use mode `{other}`"));
                None
            }
        }
    }

    pub(super) fn prefix_op(&mut self, value: &Value) -> Option<PrefixOp> {
        match self.atom(value)? {
            "neg" => Some(PrefixOp::Neg),
            "not" => Some(PrefixOp::Not),
            other => {
                self.error(value.span, format!("unknown unary operator `{other}`"));
                None
            }
        }
    }

    pub(super) fn infix_op(&mut self, value: &Value) -> Option<InfixOp> {
        Some(match self.atom(value)? {
            "add" => InfixOp::Add,
            "sub" => InfixOp::Sub,
            "mul" => InfixOp::Mul,
            "div" => InfixOp::Div,
            "floor_div" => InfixOp::FloorDiv,
            "mod" => InfixOp::Mod,
            "mat_mul" => InfixOp::MatMul,
            "shl" => InfixOp::Shl,
            "shr" => InfixOp::Shr,
            "bit_and" => InfixOp::BitAnd,
            "bit_or" => InfixOp::BitOr,
            "bit_xor" => InfixOp::BitXor,
            "pow" => InfixOp::Pow,
            "eq" => InfixOp::Eq,
            "ne" => InfixOp::Ne,
            "lt" => InfixOp::Lt,
            "gt" => InfixOp::Gt,
            "le" => InfixOp::Le,
            "ge" => InfixOp::Ge,
            "and" => InfixOp::And,
            "or" => InfixOp::Or,
            "in" => InfixOp::In,
            "not_in" => InfixOp::NotIn,
            other => {
                self.error(value.span, format!("unknown binary operator `{other}`"));
                return None;
            }
        })
    }

    pub(super) fn slice_kind(&mut self, value: &Value) -> Option<SliceKind> {
        match self.atom(value)? {
            "slice" => Some(SliceKind::Slice),
            "contiguous_slice" => Some(SliceKind::ContiguousSlice),
            "strided_slice" => Some(SliceKind::StridedSlice),
            other => {
                self.error(value.span, format!("unknown slice kind `{other}`"));
                None
            }
        }
    }

    pub(super) fn iteration_mode(&mut self, value: &Value) -> Option<IterationMode> {
        match self.atom(value)? {
            "borrowed" => Some(IterationMode::Borrowed),
            "owned" => Some(IterationMode::Owned),
            other => {
                self.error(value.span, format!("unknown iteration mode `{other}`"));
                None
            }
        }
    }

    pub(super) fn intrinsic_subscript(&mut self, value: &Value) -> Option<MirIntrinsicSubscript> {
        match self.atom(value)? {
            "tuple_storage" => Some(MirIntrinsicSubscript::TupleStorage),
            "variadic_storage" => Some(MirIntrinsicSubscript::VariadicStorage),
            "simd" => Some(MirIntrinsicSubscript::Simd),
            "pointer" => Some(MirIntrinsicSubscript::Pointer),
            "comptime_list" => Some(MirIntrinsicSubscript::ComptimeList),
            other => {
                self.error(value.span, format!("unknown intrinsic subscript `{other}`"));
                None
            }
        }
    }

    pub(super) fn result_adapter(&mut self, value: &Value) -> Option<CheckedResultAdapter> {
        match self.atom(value)? {
            "copy_iterator_reference" => Some(CheckedResultAdapter::CopyIteratorReference),
            other => {
                self.error(value.span, format!("unknown result adapter `{other}`"));
                None
            }
        }
    }

    pub(super) fn place(&mut self, value: &Value) -> Option<MirPlace> {
        let fields = self.record(value, "place").ok()?;
        let root = self.req(value, fields, "root", Self::var)?;
        let root_ty = self.req(value, fields, "root_type", |d, v| {
            Some(d.option_ty(Some(v)))
        })?;
        let (proj, projection_tys) = {
            let field = self.required(value, fields, "projections")?;
            let mut proj = Vec::new();
            let mut tys = Vec::new();
            if let Ok(values) = self.list(field) {
                for entry in values {
                    let Ok(entry_fields) = self.record(entry, "projection") else {
                        continue;
                    };
                    let op = self.req(entry, entry_fields, "op", Self::projection);
                    let ty = self.req(entry, entry_fields, "type", Self::ty);
                    self.unknown(entry_fields, &["op", "type"]);
                    if let (Some(op), Some(ty)) = (op, ty) {
                        proj.push(op);
                        tys.push(ty);
                    }
                }
            }
            (proj, tys)
        };
        let ty = self.req(value, fields, "type", |d, v| Some(d.option_ty(Some(v))))?;
        let through = self.req(value, fields, "through", |d, v| Some(d.option_var(v)))?;
        self.unknown(
            fields,
            &["root", "root_type", "projections", "type", "through"],
        );
        Some(MirPlace {
            root,
            root_ty,
            proj,
            projection_tys,
            ty,
            through,
        })
    }

    pub(super) fn projection(&mut self, value: &Value) -> Option<Proj> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "uninit_payload" => Some(Proj::UninitPayload),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "field" => self.symbol(inner).map(Proj::Field),
                "index" => self.reg(inner).map(Proj::Index),
                "const_index" => self.uint(inner).map(Proj::ConstIndex),
                "variant" => self.uint(inner).map(Proj::Variant),
                other => {
                    self.error(value.span, format!("unknown projection `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected projection");
                None
            }
        }
    }

    pub(super) fn option_place(&mut self, value: &Value) -> Option<MirPlace> {
        self.option_value(Some(value)).and_then(|v| self.place(v))
    }

    pub(super) fn places_option(&mut self, value: &Value) -> Vec<Option<MirPlace>> {
        self.list(value)
            .map(|values| values.iter().map(|v| self.option_place(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn loans(&mut self, value: &Value) -> Vec<MirLoan> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.loan(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn loan(&mut self, value: &Value) -> Option<MirLoan> {
        let fields = self.record(value, "loan").ok()?;
        let place = self.req(value, fields, "place", Self::place)?;
        let mutable = self.req(value, fields, "mutable", Self::boolean)?;
        let interior = self.req(value, fields, "interior", |d, v| {
            Some(
                d.option_value(Some(v))
                    .and_then(|v| d.mir_interior_origin(v)),
            )
        })?;
        self.unknown(fields, &["place", "mutable", "interior"]);
        Some(MirLoan {
            place,
            mutable,
            interior,
        })
    }

    pub(super) fn mir_interior_origin(&mut self, value: &Value) -> Option<MirInteriorOrigin> {
        let fields = self.record(value, "interior_origin").ok()?;
        let root = self.req(value, fields, "root", Self::var)?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        self.unknown(fields, &["root", "path"]);
        Some(MirInteriorOrigin { root, path })
    }

    pub(super) fn mir_capture_accesses(&mut self, value: &Value) -> Vec<MirCaptureAccess> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.mir_capture_access(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn mir_capture_access(&mut self, value: &Value) -> Option<MirCaptureAccess> {
        let fields = self.record(value, "capture_access").ok()?;
        let root = self.req(value, fields, "root", Self::var)?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        let access = self.req(value, fields, "access", Self::capture_access_kind)?;
        self.unknown(fields, &["root", "path", "access"]);
        Some(MirCaptureAccess { root, path, access })
    }

    pub(super) fn mir_param_args(&mut self, value: &Value) -> Vec<MirParamArg> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.mir_param_arg(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn mir_param_arg(&mut self, value: &Value) -> Option<MirParamArg> {
        let fields = self.record(value, "param_arg").ok()?;
        let name = self.req(value, fields, "name", |d, v| Some(d.option_symbol(v)))?;
        let param_value = self.req(value, fields, "value", |d, v| Some(d.option_reg(v)))?;
        self.unknown(fields, &["name", "value"]);
        Some(MirParamArg {
            name,
            value: param_value,
        })
    }

    pub(super) fn subscript_args(&mut self, value: &Value) -> Vec<MirSubscriptArg> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.subscript_arg(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn subscript_arg(&mut self, value: &Value) -> Option<MirSubscriptArg> {
        match &value.kind {
            ValueKind::Positional(tag, inner) if tag == "index" => {
                self.reg(inner).map(MirSubscriptArg::Index)
            }
            ValueKind::Record(tag, fields) if tag == "slice_arg" => {
                let kind = self.req(value, fields, "kind", Self::slice_kind)?;
                let lower = self.req(value, fields, "lower", |d, v| Some(d.option_reg(v)))?;
                let upper = self.req(value, fields, "upper", |d, v| Some(d.option_reg(v)))?;
                let step = self.req(value, fields, "step", |d, v| Some(d.option_reg(v)))?;
                self.unknown(fields, &["kind", "lower", "upper", "step"]);
                Some(MirSubscriptArg::Slice {
                    kind,
                    lower,
                    upper,
                    step,
                })
            }
            _ => {
                self.error(value.span, "expected subscript argument");
                None
            }
        }
    }

    pub(super) fn subscript_kwargs(&mut self, value: &Value) -> Vec<(String, MirSubscriptArg)> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|entry| {
                let fields = self.record(entry, "keyword").ok()?;
                let name = self.req(entry, fields, "name", Self::symbol);
                let arg = self.req(entry, fields, "value", Self::subscript_arg);
                self.unknown(fields, &["name", "value"]);
                Some((name?, arg?))
            })
            .collect()
    }

    pub(super) fn subscript_call(&mut self, value: &Value) -> Option<MirSubscriptCall> {
        let fields = self.record(value, "subscript_call").ok()?;
        let target = self.req(value, fields, "target", Self::symbol)?;
        let raises = self.req(value, fields, "raises", |d, v| Some(d.option_ty(Some(v))))?;
        let result_ty = self.req(value, fields, "result_type", Self::ty)?;
        let receiver_requires_place =
            self.req(value, fields, "receiver_requires_place", Self::boolean)?;
        let receiver_convention = self.req(value, fields, "receiver_convention", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.convention(v)))
        })?;
        let arguments = self.req(value, fields, "arguments", |d, v| Some(d.call_arguments(v)))?;
        let capture_accesses = self.req(value, fields, "capture_accesses", |d, v| {
            Some(d.mir_capture_accesses(v))
        })?;
        let reference_result = self.req(value, fields, "reference_result", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.ref_ty(v)))
        })?;
        let param_arg_regs = self.req(value, fields, "param_arg_regs", |d, v| {
            Some(d.mir_param_args(v))
        })?;
        let param_decls = self.req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?;
        self.unknown(
            fields,
            &[
                "target",
                "raises",
                "result_type",
                "receiver_requires_place",
                "receiver_convention",
                "arguments",
                "capture_accesses",
                "reference_result",
                "param_arg_regs",
                "param_decls",
            ],
        );
        Some(MirSubscriptCall {
            target,
            raises,
            result_ty,
            receiver_requires_place,
            receiver_convention,
            arguments,
            capture_accesses,
            reference_result,
            param_arg_regs,
            param_decls,
        })
    }

    pub(super) fn call_arguments(&mut self, value: &Value) -> Vec<CheckedCallArgument> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.call_argument(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn call_argument(&mut self, value: &Value) -> Option<CheckedCallArgument> {
        let fields = self.record(value, "call_argument").ok()?;
        let source = self.req(value, fields, "source", Self::call_argument_source)?;
        let parameter_ty = self.req(value, fields, "parameter_type", Self::ty)?;
        let requires_place = self.req(value, fields, "requires_place", Self::boolean)?;
        let convention = self.req(value, fields, "convention", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.convention(v)))
        })?;
        self.unknown(
            fields,
            &["source", "parameter_type", "requires_place", "convention"],
        );
        Some(CheckedCallArgument {
            source,
            parameter_ty,
            requires_place,
            convention,
        })
    }

    pub(super) fn call_argument_source(
        &mut self,
        value: &Value,
    ) -> Option<CheckedCallArgumentSource> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "default" => Some(CheckedCallArgumentSource::Default),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "positional" => self.uint(inner).map(CheckedCallArgumentSource::Positional),
                "keyword" => self.uint(inner).map(CheckedCallArgumentSource::Keyword),
                other => {
                    self.error(value.span, format!("unknown argument source `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected argument source");
                None
            }
        }
    }

    pub(super) fn iterator_call(&mut self, value: &Value) -> Option<CheckedIteratorCall> {
        let fields = self.record(value, "iterator_call").ok()?;
        let target = self.req(value, fields, "target", Self::symbol)?;
        let result_ty = self.req(value, fields, "result_type", Self::ty)?;
        let reference_result = self.req(value, fields, "reference_result", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.ref_ty(v)))
        })?;
        let raises = self.req(value, fields, "raises", |d, v| Some(d.option_ty(Some(v))))?;
        let result_adapter = self.req(value, fields, "result_adapter", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.result_adapter(v)))
        })?;
        self.unknown(
            fields,
            &[
                "target",
                "result_type",
                "reference_result",
                "raises",
                "result_adapter",
            ],
        );
        Some(CheckedIteratorCall {
            target,
            result_ty,
            reference_result,
            raises,
            result_adapter,
        })
    }

    pub(super) fn closure_captures(&mut self, value: &Value) -> Vec<MirClosureCapture> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|entry| {
                let fields = self.record(entry, "closure_capture").ok()?;
                let place = self.req(entry, fields, "place", Self::place);
                let mode = self.req(entry, fields, "mode", Self::capture_mode);
                self.unknown(fields, &["place", "mode"]);
                Some(MirClosureCapture {
                    place: place?,
                    mode: mode?,
                })
            })
            .collect()
    }

    pub(super) fn capture_mode(&mut self, value: &Value) -> Option<MirCaptureMode> {
        match self.atom(value)? {
            "reference" => Some(MirCaptureMode::Reference),
            "copy" => Some(MirCaptureMode::Copy),
            "move" => Some(MirCaptureMode::Move),
            other => {
                self.error(value.span, format!("unknown capture mode `{other}`"));
                None
            }
        }
    }

    pub(super) fn reg(&mut self, value: &Value) -> Option<Reg> {
        self.identity(Some(value), "%r").map(|v| Reg(v as u32))
    }

    pub(super) fn var(&mut self, value: &Value) -> Option<u32> {
        self.identity(Some(value), "$v").map(|v| v as u32)
    }

    pub(super) fn block_id(&mut self, value: &Value) -> Option<usize> {
        self.identity(Some(value), "bb")
    }

    pub(super) fn regs(&mut self, value: &Value) -> Vec<Reg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.reg(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn vars(&mut self, value: &Value) -> Vec<u32> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.var(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn option_reg(&mut self, value: &Value) -> Option<Reg> {
        self.option_value(Some(value)).and_then(|v| self.reg(v))
    }

    pub(super) fn option_var(&mut self, value: &Value) -> Option<u32> {
        self.option_value(Some(value)).and_then(|v| self.var(v))
    }

    pub(super) fn option_symbol(&mut self, value: &Value) -> Option<String> {
        self.option_value(Some(value)).and_then(|v| self.symbol(v))
    }

    pub(super) fn kwargs(&mut self, value: &Value) -> Vec<(String, Reg)> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|entry| {
                let fields = self.record(entry, "keyword").ok()?;
                let name = self.req(entry, fields, "name", Self::symbol);
                let reg = self.req(entry, fields, "value", Self::reg);
                self.unknown(fields, &["name", "value"]);
                Some((name?, reg?))
            })
            .collect()
    }

    pub(super) fn constant(&mut self, value: &Value) -> Option<Const> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "none" => Some(Const::None),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "int" => self.int64(inner).map(Const::Int),
                "int_literal" => self.int_literal(inner).map(Const::IntLiteral),
                "float" => self
                    .float_bits(inner)
                    .map(|bits| Const::Float(f64::from_bits(bits))),
                "float_literal" => self.float_literal(inner).map(Const::FloatLiteral),
                "bool" => self.boolean(inner).map(Const::Bool),
                "string" => self.string(inner).map(Const::Str),
                "function" => self.symbol(inner).map(Const::Function),
                other => {
                    self.error(value.span, format!("unknown constant `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected constant");
                None
            }
        }
    }
}
