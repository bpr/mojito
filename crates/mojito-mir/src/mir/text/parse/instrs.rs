//! Function body decoding: blocks, instructions, terminators, and
//! region blocks.

use super::*;

impl Decoder {
    pub(super) fn function(&mut self, value: &Value) -> Option<(String, MirFunction)> {
        let fields = self.record(value, "fn").ok()?;
        let name = self.symbol(self.field(fields, "name").ok()?)?;
        self.mark(format!("function/{name}"), value.span);
        let n_regs = self
            .uint(self.field(fields, "registers").ok()?)?
            .try_into()
            .ok()?;
        let n_vars = self.uint(self.field(fields, "vars").ok()?)?;
        let var_names = self.strings(self.field(fields, "var_names").ok()?);
        let n_params = self.uint(self.field(fields, "params").ok()?)?;
        let param_types = self.types(self.field(fields, "param_types").ok()?);
        let owned_params = self.bools(self.field(fields, "owned_params").ok()?);
        let deinit_params = self.bools(self.field(fields, "deinit_params").ok()?);
        let ref_params = self.bools(self.field(fields, "ref_params").ok()?);
        let returns_reference = self.boolean(self.field(fields, "returns_reference").ok()?)?;
        let var_tys = self.type_map(
            self.field(fields, "var_types").ok()?,
            "var_type",
            "var",
            "$v",
        );
        let ret_ty = self.option_ty(self.field(fields, "return_type").ok());
        let raises = self.boolean(self.field(fields, "raises").ok()?)?;
        let error_ty = self.option_ty(self.field(fields, "error_type").ok());
        let reg_types_raw = self.type_map(
            self.field(fields, "register_types").ok()?,
            "reg_type",
            "reg",
            "%r",
        );
        let reg_types = reg_types_raw
            .into_iter()
            .map(|(id, ty)| (id as u32, ty))
            .collect();
        let spans = self.locations(self.field(fields, "locations").ok()?);
        let blocks = self.blocks(self.field(fields, "blocks").ok()?, &name);
        if var_names.len() != n_vars {
            self.error(
                value.span,
                format!(
                    "function `{name}` declares {n_vars} vars but has {} names",
                    var_names.len()
                ),
            );
        }
        for (label, length) in [
            ("param_types", param_types.len()),
            ("owned_params", owned_params.len()),
            ("deinit_params", deinit_params.len()),
            ("ref_params", ref_params.len()),
        ] {
            if length != n_params {
                self.error(value.span, format!("function `{name}` declares {n_params} params but `{label}` has {length} entries"));
            }
        }
        self.unknown(
            fields,
            &[
                "name",
                "registers",
                "vars",
                "var_names",
                "params",
                "param_types",
                "owned_params",
                "deinit_params",
                "ref_params",
                "returns_reference",
                "var_types",
                "return_type",
                "raises",
                "error_type",
                "register_types",
                "locations",
                "blocks",
            ],
        );
        Some((
            name,
            MirFunction {
                blocks,
                n_regs,
                n_vars,
                var_names,
                n_params,
                param_types,
                owned_params,
                deinit_params,
                ref_params,
                returns_reference,
                var_tys: var_tys
                    .into_iter()
                    .map(|(id, ty)| (id as u32, ty))
                    .collect(),
                ret_ty,
                raises,
                error_ty,
                spans,
                reg_types,
            },
        ))
    }

    pub(super) fn blocks(&mut self, value: &Value, function: &str) -> Vec<MirBlock> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .enumerate()
            .filter_map(|(id, value)| {
                let fields = self.record(value, &format!("bb{id}")).ok()?;
                self.mark(format!("function/{function}/bb{id}"), value.span);
                let instrs = self
                    .list(self.field(fields, "instructions").ok()?)
                    .ok()?
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        self.mark(
                            format!("function/{function}/bb{id}/instruction/{index}"),
                            value.span,
                        );
                        self.instruction(value)
                    })
                    .collect();
                let term_value = self.field(fields, "terminator").ok()?;
                self.mark(
                    format!("function/{function}/bb{id}/terminator"),
                    term_value.span,
                );
                let term = self.term(term_value)?;
                Some(MirBlock { instrs, term })
            })
            .collect()
    }

    pub(super) fn instruction(&mut self, value: &Value) -> Option<MirInstr> {
        let (tag, fields) = self.any_record(value)?;
        match tag {
            "loans.establish" => Some(MirInstr::EstablishLoans {
                reference: self.req(value, fields, "reference", Self::var)?,
                loans: self.req(value, fields, "loans", |d, v| Some(d.loans(v)))?,
                marker: self.req(value, fields, "marker", Self::reg)?,
                dest_interior: self.req(value, fields, "dest_interior", |d, v| {
                    Some(
                        d.option_value(Some(v))
                            .and_then(|v| d.mir_interior_origin(v)),
                    )
                })?,
            }),
            "interiors.invalidate" => Some(MirInstr::InvalidateInteriors {
                base: self.req(value, fields, "base", Self::mir_interior_origin)?,
                except: self.req(value, fields, "except", |d, v| Some(d.option_var(v)))?,
                include_base_generation: self.req(
                    value,
                    fields,
                    "include_base_generation",
                    Self::boolean,
                )?,
                marker: self.req(value, fields, "marker", Self::reg)?,
            }),
            "ref.make" => Some(MirInstr::MakeRef {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
            }),
            "ref.read" => Some(MirInstr::ReadRef {
                dest: self.req(value, fields, "dest", Self::reg)?,
                reference: self.req(value, fields, "reference", Self::reg)?,
            }),
            "value.copy" => Some(MirInstr::CopyValue {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
            }),
            "ref.write" => Some(MirInstr::WriteRef {
                reference: self.req(value, fields, "reference", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
            }),
            "closure.make" => Some(MirInstr::MakeClosure {
                dest: self.req(value, fields, "dest", Self::reg)?,
                function: self.req(value, fields, "function", Self::symbol)?,
                captures: self.req(value, fields, "captures", |d, v| {
                    Some(d.closure_captures(v))
                })?,
            }),
            "lifetime.keep_alive" => Some(MirInstr::KeepAlive {
                var: self.req(value, fields, "var", Self::var)?,
            }),
            "const" => Some(MirInstr::Const {
                dest: self.req(value, fields, "dest", Self::reg)?,
                k: self.req(value, fields, "value", Self::constant)?,
            }),
            "layout.size_of" => Some(MirInstr::SizeOf {
                dest: self.req(value, fields, "dest", Self::reg)?,
                ty: self.req(value, fields, "type", Self::ty)?,
            }),
            "type.construct" => Some(MirInstr::ConstructTypeParam {
                dest: self.req(value, fields, "dest", Self::reg)?,
                // The writer spells the parameter with `symbol` (a bare atom
                // for identifier-safe names), so accept both spellings here.
                param: self.req(value, fields, "param", Self::symbol)?,
            }),
            "literal.materialize" => Some(MirInstr::MaterializeLiteral {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                target: self.req(value, fields, "target", Self::ty)?,
            }),
            "var.use" => Some(MirInstr::UseVar {
                dest: self.req(value, fields, "dest", Self::reg)?,
                var: self.req(value, fields, "var", Self::var)?,
                mode: self.req(value, fields, "mode", Self::use_mode)?,
            }),
            "place.move" => Some(MirInstr::MovePlace {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
            }),
            "var.store" => Some(MirInstr::DefVar {
                var: self.req(value, fields, "var", Self::var)?,
                src: self.req(value, fields, "src", Self::reg)?,
                binding_ty: self.req(value, fields, "binding_type", |d, v| {
                    Some(d.option_ty(Some(v)))
                })?,
            }),
            "unary" => Some(MirInstr::UnOp {
                op: self.req(value, fields, "op", Self::prefix_op)?,
                dest: self.req(value, fields, "dest", Self::reg)?,
                a: self.req(value, fields, "a", Self::reg)?,
            }),
            "binary" => Some(MirInstr::BinOp {
                op: self.req(value, fields, "op", Self::infix_op)?,
                dest: self.req(value, fields, "dest", Self::reg)?,
                a: self.req(value, fields, "a", Self::reg)?,
                b: self.req(value, fields, "b", Self::reg)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
            }),
            "call" => Some(MirInstr::Call {
                dest: self.req(value, fields, "dest", Self::reg)?,
                func: FuncRef(self.req(value, fields, "func", Self::symbol)?),
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_ty(Some(v))))?,
                args: self.req(value, fields, "args", |d, v| Some(d.regs(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.kwargs(v)))?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                kwarg_places: self.req(value, fields, "kwarg_places", |d, v| {
                    Some(d.places_option(v))
                })?,
                capture_accesses: self.req(value, fields, "capture_accesses", |d, v| {
                    Some(d.mir_capture_accesses(v))
                })?,
                param_arg_regs: self.req(value, fields, "param_arg_regs", |d, v| {
                    Some(d.mir_param_args(v))
                })?,
            }),
            "call.indirect" => Some(MirInstr::CallIndirect {
                dest: self.req(value, fields, "dest", Self::reg)?,
                callee: self.req(value, fields, "callee", Self::reg)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_ty(Some(v))))?,
                args: self.req(value, fields, "args", |d, v| Some(d.regs(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.kwargs(v)))?,
                callee_place: self.req(value, fields, "callee_place", |d, v| {
                    Some(d.option_place(v))
                })?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                kwarg_places: self.req(value, fields, "kwarg_places", |d, v| {
                    Some(d.places_option(v))
                })?,
                capture_accesses: self.req(value, fields, "capture_accesses", |d, v| {
                    Some(d.mir_capture_accesses(v))
                })?,
                param_arg_regs: self.req(value, fields, "param_arg_regs", |d, v| {
                    Some(d.mir_param_args(v))
                })?,
                param_decls: self
                    .req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?,
                instantiated_contract: self.req(
                    value,
                    fields,
                    "instantiated_contract",
                    |d, v| Some(d.option_ty(Some(v))),
                )?,
                instantiated_args: self.req(value, fields, "instantiated_args", |d, v| {
                    Some(d.ty_args(v))
                })?,
            }),
            "call.method" => Some(MirInstr::MethodCall {
                dest: self.req(value, fields, "dest", Self::reg)?,
                recv: self.req(value, fields, "recv", Self::reg)?,
                method: self.req(value, fields, "method", Self::symbol)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_ty(Some(v))))?,
                reference_result: self.req(value, fields, "reference_result", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.ref_ty(v)))
                })?,
                result_adapter: self.req(value, fields, "result_adapter", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.result_adapter(v)))
                })?,
                args: self.req(value, fields, "args", |d, v| Some(d.regs(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.kwargs(v)))?,
                recv_place: self
                    .req(value, fields, "recv_place", |d, v| Some(d.option_place(v)))?,
                recv_writes: self.req(value, fields, "recv_writes", Self::boolean)?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                kwarg_places: self.req(value, fields, "kwarg_places", |d, v| {
                    Some(d.places_option(v))
                })?,
                capture_accesses: self.req(value, fields, "capture_accesses", |d, v| {
                    Some(d.mir_capture_accesses(v))
                })?,
                param_arg_regs: self.req(value, fields, "param_arg_regs", |d, v| {
                    Some(d.mir_param_args(v))
                })?,
                param_decls: self
                    .req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?,
            }),
            "pointer.take" | "pointer.destroy" => {
                let dest = self.req(value, fields, "dest", Self::reg)?;
                let pointer = self.req(value, fields, "pointer", Self::reg)?;
                let index = self.req(value, fields, "index", Self::reg)?;
                let element = self.req(value, fields, "element", Self::ty)?;
                Some(if tag == "pointer.take" {
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
                })
            }
            "uninit.make" => Some(MirInstr::UninitStorage {
                dest: self.req(value, fields, "dest", Self::reg)?,
                init: self.req(value, fields, "init", |d, v| Some(d.option_reg(v)))?,
            }),
            "uninit.take" | "uninit.destroy" => {
                let dest = self.req(value, fields, "dest", Self::reg)?;
                let storage = self.req(value, fields, "storage", Self::reg)?;
                let element = self.req(value, fields, "element", Self::ty)?;
                Some(if tag == "uninit.take" {
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
                })
            }
            "field.get" => Some(MirInstr::GetField {
                dest: self.req(value, fields, "dest", Self::reg)?,
                base: self.req(value, fields, "base", Self::reg)?,
                field: self.req(value, fields, "field", Self::symbol)?,
            }),
            "index.get" => Some(MirInstr::Index {
                dest: self.req(value, fields, "dest", Self::reg)?,
                base: self.req(value, fields, "base", Self::reg)?,
                index: self.req(value, fields, "index", Self::reg)?,
                base_place: self
                    .req(value, fields, "base_place", |d, v| Some(d.option_place(v)))?,
                index_place: self
                    .req(value, fields, "index_place", |d, v| Some(d.option_place(v)))?,
                call: self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.subscript_call(v)))
                })?,
                intrinsic: self.req(value, fields, "intrinsic", |d, v| {
                    Some(
                        d.option_value(Some(v))
                            .and_then(|v| d.intrinsic_subscript(v)),
                    )
                })?,
            }),
            "slice.get" => Some(MirInstr::Slice {
                dest: self.req(value, fields, "dest", Self::reg)?,
                object: self.req(value, fields, "object", Self::reg)?,
                kind: self.req(value, fields, "kind", Self::slice_kind)?,
                lower: self.req(value, fields, "lower", |d, v| Some(d.option_reg(v)))?,
                upper: self.req(value, fields, "upper", |d, v| Some(d.option_reg(v)))?,
                step: self.req(value, fields, "step", |d, v| Some(d.option_reg(v)))?,
                object_place: self.req(value, fields, "object_place", |d, v| {
                    Some(d.option_place(v))
                })?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                call: self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.subscript_call(v)))
                })?,
                intrinsic: self.req(value, fields, "intrinsic", |d, v| {
                    Some(
                        d.option_value(Some(v))
                            .and_then(|v| d.intrinsic_subscript(v)),
                    )
                })?,
            }),
            "index.multi" => Some(MirInstr::MultiIndex {
                dest: self.req(value, fields, "dest", Self::reg)?,
                object: self.req(value, fields, "object", Self::reg)?,
                args: self.req(value, fields, "args", |d, v| Some(d.subscript_args(v)))?,
                object_place: self.req(value, fields, "object_place", |d, v| {
                    Some(d.option_place(v))
                })?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.subscript_kwargs(v)))?,
                kwarg_places: self.req(value, fields, "kwarg_places", |d, v| {
                    Some(d.places_option(v))
                })?,
                call: self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.subscript_call(v)))
                })?,
            }),
            "index.multi_set" => Some(MirInstr::MultiSet {
                receiver: self.req(value, fields, "receiver", Self::reg)?,
                receiver_place: self.req(value, fields, "receiver_place", |d, v| {
                    Some(d.option_place(v))
                })?,
                args: self.req(value, fields, "args", |d, v| Some(d.subscript_args(v)))?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                value: self.req(value, fields, "value", Self::reg)?,
                value_place: self
                    .req(value, fields, "value_place", |d, v| Some(d.option_place(v)))?,
                value_keyword: self.req(value, fields, "value_keyword", Self::boolean)?,
                call: self.req(value, fields, "call", Self::subscript_call)?,
            }),
            "place.store" => Some(MirInstr::Store {
                place: self.req(value, fields, "place", Self::place)?,
                src: self.req(value, fields, "src", Self::reg)?,
            }),
            "place.store_ref" => Some(MirInstr::StoreRef {
                place: self.req(value, fields, "place", Self::place)?,
                reference: self.req(value, fields, "reference", Self::reg)?,
            }),
            "place.load" => Some(MirInstr::LoadPlace {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
            }),
            "tuple.make" => Some(MirInstr::MakeTuple {
                dest: self.req(value, fields, "dest", Self::reg)?,
                elems: self.req(value, fields, "elems", |d, v| Some(d.regs(v)))?,
                element_types: self.req(value, fields, "element_types", |d, v| {
                    Some(d.option_value(Some(v)).map(|v| d.types(v)))
                })?,
            }),
            "variant.make" => Some(MirInstr::MakeVariant {
                dest: self.req(value, fields, "dest", Self::reg)?,
                alternatives: self.req(value, fields, "alternatives", |d, v| Some(d.types(v)))?,
                index: self.req(value, fields, "index", Self::uint)?,
                value: self.req(value, fields, "value", Self::reg)?,
            }),
            "variant.is" | "variant.get" => {
                let dest = self.req(value, fields, "dest", Self::reg)?;
                let variant = self.req(value, fields, "variant", Self::reg)?;
                let index = self.req(value, fields, "index", Self::uint)?;
                Some(if tag == "variant.is" {
                    MirInstr::VariantIs {
                        dest,
                        variant,
                        index,
                    }
                } else {
                    MirInstr::VariantGet {
                        dest,
                        variant,
                        index,
                    }
                })
            }
            "variant.set" => Some(MirInstr::VariantSet {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
                index: self.req(value, fields, "index", Self::uint)?,
                value: self.req(value, fields, "value", Self::reg)?,
            }),
            "variant.take" => Some(MirInstr::VariantTake {
                dest: self.req(value, fields, "dest", Self::reg)?,
                variant: self.req(value, fields, "variant", Self::reg)?,
                index: self.req(value, fields, "index", Self::uint)?,
                checked: self.req(value, fields, "checked", Self::boolean)?,
            }),
            "variant.set_init_with" => Some(MirInstr::VariantSetInitWith {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
                index: self.req(value, fields, "index", Self::uint)?,
                factory: self.req(value, fields, "factory", Self::reg)?,
            }),
            "variant.deinit_with" => Some(MirInstr::VariantDeinitWith {
                dest: self.req(value, fields, "dest", Self::reg)?,
                variant: self.req(value, fields, "variant", Self::reg)?,
                handler: self.req(value, fields, "handler", Self::reg)?,
                index: self.req(value, fields, "index", Self::uint)?,
            }),
            "variant.replace" => Some(MirInstr::VariantReplace {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
                input_index: self.req(value, fields, "input_index", Self::uint)?,
                output_index: self.req(value, fields, "output_index", Self::uint)?,
                value: self.req(value, fields, "value", Self::reg)?,
                checked: self.req(value, fields, "checked", Self::boolean)?,
            }),
            "simd.make" => Some(MirInstr::MakeSimd {
                dest: self.req(value, fields, "dest", Self::reg)?,
                dtype: self.req(value, fields, "dtype", Self::dtype)?,
                width: self.req(value, fields, "width", Self::uint)?,
                elems: self.req(value, fields, "elems", |d, v| Some(d.regs(v)))?,
            }),
            "simd.cast" => Some(MirInstr::SimdCast {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                dtype: self.req(value, fields, "dtype", Self::dtype)?,
                width: self.req(value, fields, "width", Self::uint)?,
            }),
            "simd.shuffle" => Some(MirInstr::SimdShuffle {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                mask: self.req(value, fields, "mask", |d, v| {
                    Some(
                        d.list(v)
                            .map(|values| values.iter().filter_map(|v| d.uint(v)).collect())
                            .unwrap_or_default(),
                    )
                })?,
            }),
            "raise" => Some(MirInstr::Raise {
                src: self.req(value, fields, "src", Self::reg)?,
            }),
            "try" => {
                let body = self.req(value, fields, "body", |d, v| Some(d.region_blocks(v)))?;
                let handler = {
                    let field = self.required(value, fields, "handler")?;
                    match self.option_value(Some(field)) {
                        None => None,
                        Some(inner) => {
                            let handler_fields = self.record(inner, "handler").ok()?;
                            let error_var =
                                self.req(inner, handler_fields, "error_var", |d, v| {
                                    Some(d.option_var(v))
                                })?;
                            let blocks = self.req(inner, handler_fields, "blocks", |d, v| {
                                Some(d.region_blocks(v))
                            })?;
                            self.unknown(handler_fields, &["error_var", "blocks"]);
                            Some((error_var, blocks))
                        }
                    }
                };
                let orelse = {
                    let field = self.required(value, fields, "orelse")?;
                    self.option_value(Some(field))
                        .map(|v| self.region_blocks(v))
                };
                let finalbody = {
                    let field = self.required(value, fields, "finalbody")?;
                    self.option_value(Some(field))
                        .map(|v| self.region_blocks(v))
                };
                let cleanup = self.req(value, fields, "cleanup", |d, v| Some(d.vars(v)))?;
                Some(MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    cleanup,
                })
            }
            "drop.reg" => Some(MirInstr::Drop {
                reg: self.req(value, fields, "reg", Self::reg)?,
            }),
            "drop.var" => Some(MirInstr::DropVar {
                var: self.req(value, fields, "var", Self::var)?,
            }),
            "consume.var" => Some(MirInstr::ConsumeVar {
                var: self.req(value, fields, "var", Self::var)?,
            }),
            "consume.place" => Some(MirInstr::ConsumePlace {
                place: self.req(value, fields, "place", Self::place)?,
                marker: self.req(value, fields, "marker", Self::reg)?,
            }),
            "unsupported" => Some(MirInstr::Unsupported(self.req(
                value,
                fields,
                "message",
                Self::string,
            )?)),
            "iter.init" => Some(MirInstr::GetIter {
                source: self.req(value, fields, "source", Self::var)?,
                dest: self.req(value, fields, "dest", Self::var)?,
                mode: self.req(value, fields, "mode", Self::iteration_mode)?,
                prepare: self.req(value, fields, "prepare", |d, v| Some(d.strings(v)))?,
            }),
            "iter.has_next" => Some(MirInstr::HasNext {
                dest: self.req(value, fields, "dest", Self::reg)?,
                iter: self.req(value, fields, "iter", Self::var)?,
                method: self.req(value, fields, "method", |d, v| Some(d.option_symbol(v)))?,
            }),
            "iter.next" => Some(MirInstr::Next {
                dest: self.req(value, fields, "dest", Self::reg)?,
                iter: self.req(value, fields, "iter", Self::var)?,
                call: self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.iterator_call(v)))
                })?,
            }),
            "iter.try_next" => Some(MirInstr::TryNext {
                dest: self.req(value, fields, "dest", Self::reg)?,
                yielded: self.req(value, fields, "yielded", Self::reg)?,
                iter: self.req(value, fields, "iter", Self::var)?,
                call: self.req(value, fields, "call", Self::iterator_call)?,
                exhaustion: self.req(value, fields, "exhaustion", Self::ty)?,
            }),
            other => {
                self.error(value.span, format!("unknown instruction `{other}`"));
                None
            }
        }
    }

    pub(super) fn term(&mut self, value: &Value) -> Option<MirTerm> {
        let (tag, fields) = self.any_record(value)?;
        match tag {
            "jump" => Some(MirTerm::Jump(self.req(
                value,
                fields,
                "target",
                Self::block_id,
            )?)),
            "branch" => Some(MirTerm::Branch {
                cond: self.req(value, fields, "condition", Self::reg)?,
                then_b: self.req(value, fields, "then", Self::block_id)?,
                else_b: self.req(value, fields, "else", Self::block_id)?,
            }),
            "return" => Some(MirTerm::Return(self.req(
                value,
                fields,
                "value",
                |d, v| Some(d.option_reg(v)),
            )?)),
            "return.cleanup" => Some(MirTerm::ReturnWithCleanup {
                value: self.req(value, fields, "value", |d, v| Some(d.option_reg(v)))?,
                cleanup: self.req(value, fields, "cleanup", |d, v| Some(d.vars(v)))?,
            }),
            "falloff" => Some(MirTerm::FallOff),
            "escape" => Some(MirTerm::EscapeJump {
                target: self.req(value, fields, "target", Self::block_id)?,
                cleanup: self.req(value, fields, "cleanup", |d, v| Some(d.vars(v)))?,
            }),
            other => {
                self.error(value.span, format!("unknown terminator `{other}`"));
                None
            }
        }
    }

    /// Blocks of one `try` sub-region. Each region carries its own dense
    /// `bb0..bbN` namespace (see `docs/mir-text-format.md`); region-local
    /// blocks are deliberately absent from the artifact source map — the
    /// enclosing instruction's path already brackets them, and the canonical
    /// verifier only resolves function-level block paths.
    pub(super) fn region_blocks(&mut self, value: &Value) -> Vec<MirBlock> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .enumerate()
            .filter_map(|(id, value)| {
                let fields = self.record(value, &format!("bb{id}")).ok()?;
                let instrs = {
                    let field = self.required(value, fields, "instructions")?;
                    self.list(field)
                        .ok()?
                        .iter()
                        .filter_map(|v| self.instruction(v))
                        .collect()
                };
                let term = self.req(value, fields, "terminator", Self::term)?;
                self.unknown(fields, &["instructions", "terminator"]);
                Some(MirBlock { instrs, term })
            })
            .collect()
    }
}
