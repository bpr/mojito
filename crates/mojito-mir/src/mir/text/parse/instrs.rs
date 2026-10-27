//! Function body decoding: blocks, instructions, terminators, and
//! region blocks.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::{LoopLabel, MirCase, MirElif, MirRefinedBound, MirResultAdapter, TryLabel};
use mojito_hir::hir::VarId;
use mojito_types::types::TyKind;

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
        let error_slot = self
            .field(fields, "error_slot")
            .ok()
            .and_then(|slot| self.var(slot));
        let reg_types_raw = self.type_map(
            self.field(fields, "register_types").ok()?,
            "reg_type",
            "reg",
            "%r",
        );
        let reg_types: HashMap<u32, Ty> = reg_types_raw
            .into_iter()
            .map(|(id, ty)| (id as u32, ty))
            .collect();
        self.register_binders = reg_types
            .iter()
            .filter_map(|(register, ty)| match ty.kind() {
                TyKind::Param { binder, .. } => Some((*register, binder.clone())),
                _ => None,
            })
            .collect();
        let spans = self.locations(self.field(fields, "locations").ok()?);
        self.unlabelled_tries = false;
        self.legacy_raises = false;
        let mut blocks = self.blocks(self.field(fields, "blocks").ok()?, &name);
        let unlabelled = std::mem::take(&mut self.unlabelled_tries);
        if unlabelled {
            infer_try_labels(&mut blocks, &mut 0, None);
        }
        let legacy_raises = std::mem::take(&mut self.legacy_raises) || unlabelled;
        let removed_params = self
            .field(fields, "removed_params")
            .ok()
            .map(|value| self.param_ids(value))
            .unwrap_or_default();
        let callee_removed_params = self
            .field(fields, "callee_removed_params")
            .ok()
            .map(|value| self.callee_params(value))
            .unwrap_or_default();
        let removed_args = self
            .field(fields, "removed_args")
            .ok()
            .and_then(|value| self.list(value).ok())
            .map(|values| values.iter().filter_map(|value| self.var(value)).collect())
            .unwrap_or_default();
        let callee_removed_args = self
            .field(fields, "callee_removed_args")
            .ok()
            .map(|value| self.callee_args(value))
            .unwrap_or_default();
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
                "error_slot",
                "register_types",
                "locations",
                "blocks",
                "removed_params",
                "callee_removed_params",
                "removed_args",
                "callee_removed_args",
            ],
        );
        let mut function = MirFunction {
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
            error_slot,
            spans,
            reg_types,
            removed_params,
            callee_removed_params,
            removed_args,
            callee_removed_args,
            lowered_calls: Vec::new(),
        };
        if legacy_raises {
            upgrade_error_slots(&mut function);
        }
        if function.error_slot.is_none() {
            upgrade_function_error_slot(&mut function);
        }
        Some((name, function))
    }

    /// A function's `removed_params` (schema 1.60): its own binders the
    /// pre-elaboration pipeline removed.
    fn param_ids(&mut self, value: &Value) -> Vec<mojito_types::param_expr::ParamId> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| {
                let fields = self.record(value, "param_id").ok()?;
                let owner = self.req(value, fields, "owner", Self::string)?;
                let slot = self.req(value, fields, "slot", Self::uint)?;
                self.unknown(fields, &["owner", "slot"]);
                Some(mojito_types::param_expr::ParamId::new(&owner, slot))
            })
            .collect()
    }

    /// A function's `callee_removed_params` (schema 1.62): each callee whose
    /// removed parameters its `removed_params` discounted a forward to.
    fn callee_params(
        &mut self,
        value: &Value,
    ) -> Vec<(String, Vec<mojito_types::param_expr::ParamId>)> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| {
                let fields = self.record(value, "callee_params").ok()?;
                let callee = self.req(value, fields, "callee", Self::string)?;
                let params = self.param_ids(self.field(fields, "params").ok()?);
                self.unknown(fields, &["callee", "params"]);
                Some((callee, params))
            })
            .collect()
    }

    /// A function's `callee_removed_args` (schema 1.66): each callee whose
    /// removed arguments its `removed_args` relied on dropping a forward to.
    fn callee_args(&mut self, value: &Value) -> Vec<(String, Vec<VarId>)> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| {
                let fields = self.record(value, "callee_args").ok()?;
                let callee = self.req(value, fields, "callee", Self::string)?;
                let args = self
                    .list(self.field(fields, "args").ok()?)
                    .ok()?
                    .iter()
                    .filter_map(|value| self.var(value))
                    .collect();
                self.unknown(fields, &["callee", "args"]);
                Some((callee, args))
            })
            .collect()
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
                Some(self.block_of(instrs, term))
            })
            .collect()
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
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
                constructor: self.lifecycle_symbol(fields, "constructor"),
            }),
            "value.rebind" => Some(MirInstr::Rebind {
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
            "param_list.address" => Some(MirInstr::ParamListAddress {
                dest: self.req(value, fields, "dest", Self::reg)?,
                values: self.req(value, fields, "values", Self::constant)?,
            }),
            "store_to_mem" => Some(MirInstr::StoreToMem {
                dest: self.req(value, fields, "dest", Self::reg)?,
                src: self.req(value, fields, "src", Self::reg)?,
            }),
            "type.name" => Some(MirInstr::TypeName {
                dest: self.req(value, fields, "dest", Self::reg)?,
                ty: self.req(value, fields, "type", Self::ty)?,
            }),
            "type.construct_expr" => Some(MirInstr::ConstructType {
                dest: self.req(value, fields, "dest", Self::reg)?,
                ty: self.req(value, fields, "type", Self::ty)?,
            }),
            "type.construct" => {
                // The writer spells the parameter with `symbol` (a bare atom
                // for identifier-safe names), so accept both spellings here.
                let name = self.req(value, fields, "param", Self::symbol)?;
                let dest = self.req(value, fields, "dest", Self::reg)?;
                let param = match self.register_binders.get(&dest.0) {
                    Some(binder)
                        if self.field(fields, "owner").is_err() && binder.name.as_ref() == name =>
                    {
                        binder.clone()
                    }
                    _ => mojito_types::param_expr::ParamRef {
                        id: self.binder_id(value, fields, &name)?,
                        name: name.into(),
                    },
                };
                // Schema 1.19 records the bound initializer's keyword
                // arguments; an older artifact constructs with none.
                let kwargs = self
                    .field(fields, "kwargs")
                    .ok()
                    .map(|found| self.kwargs(found))
                    .unwrap_or_default();
                let kwarg_places = self
                    .field(fields, "kwarg_places")
                    .ok()
                    .map(|found| self.places_option(found))
                    .unwrap_or_default();
                // Schema 1.21 records a pack element's index; an older
                // artifact constructs a type parameter alone.
                let element = self
                    .field(fields, "element")
                    .ok()
                    .and_then(|found| self.option_value(Some(found)))
                    .and_then(|found| self.mir_param_arg(found));
                Some(MirInstr::ConstructTypeParam {
                    dest,
                    param,
                    kwargs,
                    kwarg_places,
                    element,
                })
            }
            "literal.materialize" => Some(MirInstr::MaterializeLiteral {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                target: self.req(value, fields, "target", Self::ty)?,
            }),
            "scalar.cast" => Some(MirInstr::ScalarCast {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                target: self.req(value, fields, "target", Self::scalar_conversion)?,
            }),
            "var.use" => Some(MirInstr::UseVar {
                dest: self.req(value, fields, "dest", Self::reg)?,
                var: self.req(value, fields, "var", Self::var)?,
                mode: self.req(value, fields, "mode", Self::use_mode)?,
                constructor: self.lifecycle_symbol(fields, "constructor"),
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
                in_place: self.req(value, fields, "in_place", Self::boolean)?,
            }),
            "unary" => Some(MirInstr::UnOp {
                op: self.req(value, fields, "op", Self::prefix_op)?,
                dest: self.req(value, fields, "dest", Self::reg)?,
                a: self.req(value, fields, "a", Self::reg)?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_raises(v)))?,
            }),
            "binary" => Some(MirInstr::BinOp {
                op: self.req(value, fields, "op", Self::infix_op)?,
                dest: self.req(value, fields, "dest", Self::reg)?,
                a: self.req(value, fields, "a", Self::reg)?,
                b: self.req(value, fields, "b", Self::reg)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_raises(v)))?,
                refined_bound: self.refined_bound(fields).map(MirRefinedBound::boxed),
            }),
            "call" => Some(MirInstr::Call {
                dest: self.req(value, fields, "dest", Self::reg)?,
                func: FuncRef(self.req(value, fields, "func", Self::symbol)?),
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_raises(v)))?,
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
                // Schema 1.8 records the spelled receiver of a static call; an
                // older artifact carries none.
                receiver: (self
                    .field(fields, "receiver")
                    .ok()
                    .and_then(|found| self.option_ty(Some(found)))),
                // Schema 1.13 records the solved arguments of a generic
                // `def` call; an older artifact carries none.
                instantiated_args: self
                    .field(fields, "instantiated_args")
                    .ok()
                    .map(|found| self.ty_args(found))
                    .unwrap_or_default(),
                // Schema 1.17 records the position of a whole pack spread
                // into the callee's collector; an older artifact carries
                // none.
                spread: self
                    .field(fields, "spread")
                    .ok()
                    .and_then(|found| self.option_uint(found)),
            }),
            "call.indirect" => Some(MirInstr::CallIndirect {
                dest: self.req(value, fields, "dest", Self::reg)?,
                callee: self.req(value, fields, "callee", Self::reg)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_raises(v)))?,
                args: self.req(value, fields, "args", |d, v| Some(d.regs(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.kwargs(v)))?,
                callee_place: (self.req(value, fields, "callee_place", |d, v| {
                    Some(d.option_place(v))
                })?)
                .map(Box::new),
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
                instantiated_contract: (self.req(
                    value,
                    fields,
                    "instantiated_contract",
                    |d, v| Some(d.option_ty(Some(v))),
                )?),
                instantiated_args: self.req(value, fields, "instantiated_args", |d, v| {
                    Some(d.ty_args(v))
                })?,
                // Schema 1.40 records the position of a whole pack spread
                // into a nested `def`'s collector; an older artifact carries
                // none.
                spread: self
                    .field(fields, "spread")
                    .ok()
                    .and_then(|found| self.option_uint(found)),
            }),
            "call.method" => Some(MirInstr::MethodCall {
                dest: self.req(value, fields, "dest", Self::reg)?,
                recv: self.req(value, fields, "recv", Self::reg)?,
                method: self.req(value, fields, "method", Self::symbol)?,
                resolved: self.req(value, fields, "resolved", |d, v| Some(d.option_symbol(v)))?,
                raises: self.req(value, fields, "raises", |d, v| Some(d.option_raises(v)))?,
                reference_result: (self.req(value, fields, "reference_result", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.ref_ty(v)))
                })?)
                .map(Box::new),
                result_adapter: self
                    .req(value, fields, "result_adapter", |d, v| {
                        Some(d.option_value(Some(v)).and_then(|v| d.result_adapter(v)))
                    })?
                    .map(|kind| {
                        Box::new(MirResultAdapter {
                            kind,
                            constructor: self.lifecycle_symbol(fields, "adapter_constructor"),
                        })
                    }),
                args: self.req(value, fields, "args", |d, v| Some(d.regs(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.kwargs(v)))?,
                recv_place: (self
                    .req(value, fields, "recv_place", |d, v| Some(d.option_place(v)))?)
                .map(Box::new),
                recv_writes: self.req(value, fields, "recv_writes", Self::boolean)?,
                // Schema 1.100 records the callee's receiver convention;
                // 1.98 and 1.99 record only whether it is `deinit`, and an
                // older artifact neither, which the verifier refuses for a
                // callee whose receiver takes ownership.
                receiver_convention: match self.field(fields, "receiver_convention") {
                    Ok(found) => self
                        .option_value(Some(found))
                        .and_then(|v| self.convention(v)),
                    Err(()) => self
                        .field(fields, "deinit_receiver")
                        .ok()
                        .and_then(|found| self.boolean(found))
                        .and_then(|deinit| deinit.then_some(ArgConvention::Deinit)),
                },
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
                // Schema 1.22 records a method's own solved arguments; an
                // older artifact carries none.
                instantiated_args: self
                    .field(fields, "instantiated_args")
                    .ok()
                    .map(|found| self.ty_args(found))
                    .unwrap_or_default(),
                // Schema 1.24 records the position of a whole pack spread
                // into the method's collector; an older artifact carries
                // none.
                spread: self
                    .field(fields, "spread")
                    .ok()
                    .and_then(|found| self.option_uint(found)),
                // Schema 1.114 records the refined bound a requirement call
                // was selected through; a call recording none omits it.
                refined_bound: self.refined_bound(fields).map(MirRefinedBound::boxed),
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
                        destructor: self.lifecycle_symbol(fields, "destructor"),
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
                        destructor: self.lifecycle_symbol(fields, "destructor"),
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
                base_place: (self
                    .req(value, fields, "base_place", |d, v| Some(d.option_place(v)))?)
                .map(Box::new),
                index_place: (self
                    .req(value, fields, "index_place", |d, v| Some(d.option_place(v)))?)
                .map(Box::new),
                call: (self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.subscript_call(v)))
                })?)
                .map(Box::new),
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
                object_place: (self.req(value, fields, "object_place", |d, v| {
                    Some(d.option_place(v))
                })?)
                .map(Box::new),
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                kwargs: self.req(value, fields, "kwargs", |d, v| Some(d.subscript_kwargs(v)))?,
                kwarg_places: self.req(value, fields, "kwarg_places", |d, v| {
                    Some(d.places_option(v))
                })?,
                call: (self.req(value, fields, "call", |d, v| {
                    Some(d.option_value(Some(v)).and_then(|v| d.subscript_call(v)))
                })?)
                .map(Box::new),
            }),
            "index.multi_set" => Some(MirInstr::MultiSet {
                receiver: self.req(value, fields, "receiver", Self::reg)?,
                receiver_place: (self.req(value, fields, "receiver_place", |d, v| {
                    Some(d.option_place(v))
                })?)
                .map(Box::new),
                args: self.req(value, fields, "args", |d, v| Some(d.subscript_args(v)))?,
                arg_places: self
                    .req(value, fields, "arg_places", |d, v| Some(d.places_option(v)))?,
                value: self.req(value, fields, "value", Self::reg)?,
                value_place: (self
                    .req(value, fields, "value_place", |d, v| Some(d.option_place(v)))?)
                .map(Box::new),
                value_keyword: self.req(
                    value,
                    fields,
                    "value_keyword",
                    Self::setter_value_keyword,
                )?,
                call: Box::new(self.req(value, fields, "call", Self::subscript_call)?),
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
            "struct.create" => Some(MirInstr::MakeStruct {
                dest: self.req(value, fields, "dest", Self::reg)?,
                fields: self.req(value, fields, "fields", |d, v| Some(d.regs(v)))?,
            }),
            "array.create" => Some(MirInstr::MakeArray {
                dest: self.req(value, fields, "dest", Self::reg)?,
                elems: self.req(value, fields, "elems", |d, v| Some(d.regs(v)))?,
            }),
            "variant.make" => Some(MirInstr::MakeVariant {
                dest: self.req(value, fields, "dest", Self::reg)?,
                alternatives: self.req(value, fields, "alternatives", |d, v| Some(d.types(v)))?,
                index: self.req(value, fields, "index", Self::variant_index)?,
                value: self.req(value, fields, "value", Self::reg)?,
                constructor: self.lifecycle_symbol(fields, "constructor"),
            }),
            "variant.tag" => Some(MirInstr::VariantTag {
                dest: self.req(value, fields, "dest", Self::reg)?,
                variant: self.req(value, fields, "variant", Self::reg)?,
            }),
            "variant.is" | "variant.get" => {
                let dest = self.req(value, fields, "dest", Self::reg)?;
                let variant = self.req(value, fields, "variant", Self::reg)?;
                let index = self.req(value, fields, "index", Self::variant_index)?;
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
                index: self.req(value, fields, "index", Self::variant_index)?,
                value: self.req(value, fields, "value", Self::reg)?,
                constructor: self.lifecycle_symbol(fields, "constructor"),
                destructors: self.lifecycle_symbols(fields, "destructors"),
            }),
            "variant.take" => Some(MirInstr::VariantTake {
                dest: self.req(value, fields, "dest", Self::reg)?,
                variant: self.req(value, fields, "variant", Self::reg)?,
                index: self.req(value, fields, "index", Self::variant_index)?,
                checked: self.req(value, fields, "checked", Self::boolean)?,
            }),
            "variant.set_init_with" => Some(MirInstr::VariantSetInitWith {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
                index: self.req(value, fields, "index", Self::variant_index)?,
                factory: self.req(value, fields, "factory", Self::reg)?,
                destructors: self.lifecycle_symbols(fields, "destructors"),
            }),
            "variant.deinit_with" => Some(MirInstr::VariantDeinitWith {
                dest: self.req(value, fields, "dest", Self::reg)?,
                variant: self.req(value, fields, "variant", Self::reg)?,
                handler: self.req(value, fields, "handler", Self::reg)?,
                index: self.req(value, fields, "index", Self::variant_index)?,
            }),
            "variant.replace" => Some(MirInstr::VariantReplace {
                dest: self.req(value, fields, "dest", Self::reg)?,
                place: self.req(value, fields, "place", Self::place)?,
                input_index: self.req(value, fields, "input_index", Self::variant_index)?,
                output_index: self.req(value, fields, "output_index", Self::variant_index)?,
                value: self.req(value, fields, "value", Self::reg)?,
                checked: self.req(value, fields, "checked", Self::boolean)?,
                constructor: self.lifecycle_symbol(fields, "constructor"),
            }),
            "simd.make" => Some(MirInstr::MakeSimd {
                dest: self.req(value, fields, "dest", Self::reg)?,
                dtype: self.req(value, fields, "dtype", Self::simd_dtype_slot)?,
                width: self.req(value, fields, "width", Self::simd_width_slot)?,
                elems: self.req(value, fields, "elems", |d, v| Some(d.regs(v)))?,
            }),
            "simd.cast" => Some(MirInstr::SimdCast {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                dtype: self.req(value, fields, "dtype", Self::simd_dtype_slot)?,
                width: self.req(value, fields, "width", Self::simd_width_slot)?,
            }),
            "simd.bits" => Some(MirInstr::SimdBitcast {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                dtype: self.req(value, fields, "dtype", Self::simd_dtype_slot)?,
                width: self.req(value, fields, "width", Self::simd_width_slot)?,
            }),
            "simd.shuffle" => Some(MirInstr::SimdShuffle {
                dest: self.req(value, fields, "dest", Self::reg)?,
                value: self.req(value, fields, "value", Self::reg)?,
                other: self.req(value, fields, "other", |d, v| Some(d.option_reg(v)))?,
                mask: self.req(value, fields, "mask", Self::lane_mask)?,
            }),
            "try" => {
                let body = self.req(value, fields, "body", |d, v| Some(d.region_blocks(v)))?;
                let mut error = self
                    .field(fields, "error")
                    .ok()
                    .and_then(|error| self.option_var(error));
                let handler = {
                    let field = self.required(value, fields, "handler")?;
                    match self.option_value(Some(field)) {
                        None => None,
                        // MIR older than 1.107 binds the caught error in
                        // the handler, the slot the `try` now names.
                        Some(inner) if matches!(&inner.kind, ValueKind::Record(..)) => {
                            let handler_fields = self.record(inner, "handler").ok()?;
                            error = self.req(inner, handler_fields, "error_var", |d, v| {
                                Some(d.option_var(v))
                            })?;
                            let blocks = self.req(inner, handler_fields, "blocks", |d, v| {
                                Some(d.region_blocks(v))
                            })?;
                            self.unknown(handler_fields, &["error_var", "blocks"]);
                            Some(blocks)
                        }
                        Some(inner) => Some(self.region_blocks(inner)),
                    }
                };
                let orelse = {
                    let field = self.required(value, fields, "orelse")?;
                    self.option_value(Some(field))
                        .map(|v| self.region_blocks(v))
                };
                self.no_finally(fields)?;
                let label = if let Ok(label) = self.field(fields, "label") {
                    self.try_label(label)?
                } else {
                    self.unlabelled_tries = true;
                    TryLabel(0)
                };
                Some(MirInstr::Try {
                    label,
                    error,
                    results: self.added_regs(fields, "results"),
                    body,
                    handler_args: self.added_regs(fields, "handler_arguments"),
                    handler,
                    else_args: self.added_regs(fields, "else_arguments"),
                    orelse,
                })
            }
            "if" => Some(MirInstr::If {
                results: self.req(value, fields, "results", |d, v| Some(d.regs(v)))?,
                cond: self.req(value, fields, "condition", Self::reg)?,
                then: self.req(value, fields, "then", |d, v| Some(d.region_blocks(v)))?,
                elifs: self.req(value, fields, "elifs", |d, v| Some(d.elifs(v)))?,
                else_args: self.added_regs(fields, "else_arguments"),
                orelse: self.req(value, fields, "else", |d, v| Some(d.region_blocks(v)))?,
            }),
            "loop" => Some(MirInstr::Loop {
                label: self.req(value, fields, "label", |d, v| Some(d.option_loop_label(v)))?,
                operands: self.added_regs(fields, "operands"),
                args: self.added_regs(fields, "arguments"),
                results: self.added_regs(fields, "results"),
                body: self.req(value, fields, "body", |d, v| Some(d.region_blocks(v)))?,
            }),
            "comptime.if" => Some(MirInstr::ComptimeIf {
                results: self.req(value, fields, "results", |d, v| Some(d.regs(v)))?,
                conds: self.req(value, fields, "conditions", |d, v| Some(d.constraints(v)))?,
                arms: self.req(value, fields, "arms", |d, v| {
                    let arms = d.list(v).ok()?;
                    Some(arms.iter().map(|arm| d.region_blocks(arm)).collect())
                })?,
                orelse: self.req(value, fields, "else", |d, v| Some(d.region_blocks(v)))?,
            }),
            "comptime.for" => Some(MirInstr::ComptimeFor {
                binder: self.req(value, fields, "binder", Self::binder_ref)?,
                source: self.comptime_sequence(value, fields)?,
                operands: self.added_regs(fields, "operands"),
                args: self.added_regs(fields, "arguments"),
                results: self.added_regs(fields, "results"),
                body: self.req(value, fields, "body", |d, v| Some(d.region_blocks(v)))?,
                orelse: self.req(value, fields, "else", |d, v| Some(d.region_blocks(v)))?,
            }),
            "comptime.assert" => Some(MirInstr::ComptimeAssert {
                cond: Box::new(self.req(value, fields, "condition", Self::constraint)?),
            }),
            "match" => Some(MirInstr::Match {
                results: self.added_regs(fields, "results"),
                cases: self.req(value, fields, "cases", |d, v| Some(d.cases(v)))?,
                else_args: self.added_regs(fields, "else_arguments"),
                orelse: self.req(value, fields, "else", |d, v| Some(d.region_blocks(v)))?,
            }),
            "drop.reg" => Some(MirInstr::Drop {
                reg: self.req(value, fields, "reg", Self::reg)?,
                destructor: self.lifecycle_symbol(fields, "destructor"),
            }),
            "var.declare" => Some(MirInstr::DeclareVar {
                var: self.req(value, fields, "var", Self::var)?,
            }),
            "drop.var" => Some(MirInstr::DropVar {
                var: self.req(value, fields, "var", Self::var)?,
                destructor: self.lifecycle_symbol(fields, "destructor"),
            }),
            "consume.var" => Some(MirInstr::ConsumeVar {
                var: self.req(value, fields, "var", Self::var)?,
            }),
            "drop.place" => Some(MirInstr::DropPlace {
                place: self.req(value, fields, "place", Self::place)?,
                destructor: self.lifecycle_symbol(fields, "destructor"),
            }),
            "ownership.mark_initialized" => Some(MirInstr::MarkInitialized {
                place: self.req(value, fields, "place", Self::place)?,
            }),
            "ownership.mark_destroyed" => Some(MirInstr::MarkDestroyed {
                place: self.req(value, fields, "place", Self::place)?,
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
            }),
            "iter.has_next" => Some(MirInstr::HasNext {
                dest: self.req(value, fields, "dest", Self::reg)?,
                iter: self.req(value, fields, "iter", Self::var)?,
            }),
            "iter.next" => Some(MirInstr::Next {
                dest: self.req(value, fields, "dest", Self::reg)?,
                iter: self.req(value, fields, "iter", Self::var)?,
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
            "return" => Some(MirTerm::Return(self.req(
                value,
                fields,
                "value",
                |d, v| Some(d.option_reg(v)),
            )?)),
            "return.cleanup" => {
                self.error(
                    value.span,
                    "`return.cleanup` is gone since MIR 1.92: MIR lowering copies each \
                     `finally` before the return that crosses it (R884)",
                );
                None
            }
            "try.yield" => Some(MirTerm::TryYield(self.added_regs(fields, "values"))),
            "yield" => Some(MirTerm::Yield(self.req(
                value,
                fields,
                "values",
                |d, v| Some(d.regs(v)),
            )?)),
            "if.elifcond.yield" => Some(MirTerm::ElifCond {
                cond: self.req(value, fields, "condition", Self::reg)?,
                values: self.added_regs(fields, "values"),
            }),
            "break" => Some(MirTerm::Break {
                label: self.req(value, fields, "label", |d, v| Some(d.option_loop_label(v)))?,
                values: self.added_regs(fields, "values"),
            }),
            "continue" => Some(MirTerm::Continue {
                label: self.req(value, fields, "label", |d, v| Some(d.option_loop_label(v)))?,
                values: self.added_regs(fields, "values"),
            }),
            "comptime.yield" => Some(MirTerm::ComptimeYield(self.req(
                value,
                fields,
                "values",
                |d, v| Some(d.regs(v)),
            )?)),
            "comptime.for.break" => Some(MirTerm::ComptimeBreak(self.added_regs(fields, "values"))),
            "comptime.for.continue" => {
                Some(MirTerm::ComptimeContinue(self.added_regs(fields, "values")))
            }
            "match.next" => Some(MirTerm::MatchNext(self.added_regs(fields, "values"))),
            "match.complete" => Some(MirTerm::MatchComplete(self.added_regs(fields, "values"))),
            "try.raise" => Some(MirTerm::TryRaise {
                label: self.req(value, fields, "label", Self::try_label)?,
                values: self.added_regs(fields, "values"),
            }),
            // MIR older than 1.108 raises its operand out of the function,
            // through the function's error slot.
            "error_return" => {
                if let Ok(src) = self.field(fields, "src") {
                    let src = self.reg(src)?;
                    self.legacy_raise = Some((LEGACY_FUNCTION_ERROR_SLOT, src));
                    self.legacy_raises = true;
                }
                Some(MirTerm::ErrorReturn)
            }
            // MIR older than 1.107 raises its operand: to the `try` it
            // names, through that `try`'s slot, or out of the function,
            // through the function's.
            "raise" => {
                let src = self.req(value, fields, "src", Self::reg)?;
                let label = if let Ok(label) = self.field(fields, "label") {
                    self.option_value(Some(label))
                        .map(|label| self.try_label(label))
                        .map_or(Some(None), |label| label.map(Some))?
                } else {
                    self.unlabelled_tries = true;
                    None
                };
                let (slot, term) = label.map_or(
                    (LEGACY_FUNCTION_ERROR_SLOT, MirTerm::ErrorReturn),
                    |label| {
                        let values = self.added_regs(fields, "values");
                        (LEGACY_ERROR_SLOT, MirTerm::TryRaise { label, values })
                    },
                );
                self.legacy_raise = Some((slot, src));
                self.legacy_raises = true;
                Some(term)
            }
            "unreachable" => Some(MirTerm::Unreachable),
            other => {
                self.error(value.span, format!("unknown terminator `{other}`"));
                None
            }
        }
    }

    /// The lifecycle member a drop (`destructor`, schema 1.67) or a copy or
    /// move (`constructor`, schema 1.68) names, absent when it names none.
    /// The `refined_bound` a call records (schema 1.114), absent when it
    /// records none.
    pub(super) fn refined_bound(&mut self, fields: &[Field]) -> Option<Vec<String>> {
        self.field(fields, "refined_bound")
            .ok()
            .map(|value| self.strings(value))
    }

    fn lifecycle_symbol(&mut self, fields: &[Field], key: &str) -> Option<String> {
        self.field(fields, key)
            .ok()
            .and_then(|value| self.symbol(value))
    }

    /// The per-alternative destructors a variant store names
    /// (`destructors`, schema 1.73), each present or absent; empty when the
    /// field is absent, as before 1.73.
    fn lifecycle_symbols(&mut self, fields: &[Field], key: &str) -> Vec<Option<String>> {
        let Ok(value) = self.field(fields, key) else {
            return Vec::new();
        };
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .map(|value| self.option_symbol(value))
            .collect()
    }

    /// The `elif` arms of an `if`: each a condition region and the arm it
    /// selects, with their arguments.
    fn elifs(&mut self, value: &Value) -> Vec<MirElif> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| {
                let fields = self.record(value, "elif").ok()?;
                let cond_args = self.added_regs(fields, "condition_arguments");
                let cond = self.req(value, fields, "condition", |d, v| Some(d.region_blocks(v)))?;
                let then_args = self.added_regs(fields, "then_arguments");
                let then = self.req(value, fields, "then", |d, v| Some(d.region_blocks(v)))?;
                self.unknown(
                    fields,
                    &["condition_arguments", "condition", "then_arguments", "then"],
                );
                Some(MirElif {
                    cond_args,
                    cond,
                    then_args,
                    then,
                })
            })
            .collect()
    }

    /// The case regions of a `match`: each a `case` record with its
    /// arguments, or a bare region carrying none before 1.84.
    fn cases(&mut self, value: &Value) -> Vec<MirCase> {
        let Ok(values) = self.list(value) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| {
                if matches!(value.kind, ValueKind::List(_)) {
                    return Some(MirCase {
                        args: Vec::new(),
                        body: self.region_blocks(value),
                    });
                }
                let fields = self.record(value, "case").ok()?;
                let args = self.added_regs(fields, "arguments");
                let body = self.req(value, fields, "body", |d, v| Some(d.region_blocks(v)))?;
                self.unknown(fields, &["arguments", "body"]);
                Some(MirCase { args, body })
            })
            .collect()
    }

    /// A `comptime for` header's source: its `initial`/`has_next`/
    /// `next_iter`/`next_value` fields.
    fn comptime_sequence(
        &mut self,
        value: &Value,
        fields: &[Field],
    ) -> Option<mojito_checked::checked::ComptimeSequence> {
        Some(mojito_checked::checked::ComptimeSequence {
            initial: self.req(value, fields, "initial", Self::param_expr)?,
            has_next: self.req(value, fields, "has_next", Self::param_expr)?,
            next_iter: self.req(value, fields, "next_iter", Self::param_expr)?,
            next_value: self.req(value, fields, "next_value", Self::param_expr)?,
        })
    }

    /// The block `instrs` ended by `term`, a raise read from text older
    /// than MIR 1.108 first storing its error into the slot
    /// [`upgrade_error_slots`] names.
    fn block_of(&mut self, mut instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
        if let Some((slot, src)) = self.legacy_raise.take() {
            instrs.push(legacy_error_store(slot, src));
        }
        MirBlock { instrs, term }
    }

    fn try_label(&mut self, value: &Value) -> Option<TryLabel> {
        let id = self.identity(Some(value), "try")?;
        u32::try_from(id).ok().map(TryLabel)
    }

    /// Reject the `finally` region and raise-edge cleanup list a `try`
    /// older than MIR 1.92 carries, unless absent and empty: MIR lowering
    /// now copies each `finally` before the exits that cross it (R884).
    fn no_finally(&mut self, fields: &[Field]) -> Option<()> {
        if let Ok(finalbody) = self.field(fields, "finalbody")
            && self.option_value(Some(finalbody)).is_some()
        {
            self.error(
                finalbody.span,
                "a `try` holds no `finally` since MIR 1.92: MIR lowering copies it before \
                 each exit that crosses it (R884)",
            );
            return None;
        }
        if let Ok(cleanup) = self.field(fields, "cleanup")
            && !self.list(cleanup).is_ok_and(<[Value]>::is_empty)
        {
            self.error(
                cleanup.span,
                "a `try` lists no raise-edge cleanup since MIR 1.92: each raise destroys \
                 what it holds before it leaves (R884)",
            );
            return None;
        }
        Some(())
    }

    fn loop_label(&mut self, value: &Value) -> Option<LoopLabel> {
        let id = self.identity(Some(value), "loop")?;
        u32::try_from(id).ok().map(LoopLabel)
    }

    fn option_loop_label(&mut self, value: &Value) -> Option<LoopLabel> {
        self.option_value(Some(value))
            .and_then(|value| self.loop_label(value))
    }

    /// Blocks of one region. Each region carries its own dense
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
                Some(self.block_of(instrs, term))
            })
            .collect()
    }
}

/// Give the `try`s and `raise`s of `blocks`, read from text older than MIR
/// 1.92, the labels their old meaning implies: each `try` the next number
/// in preorder from `next`, and each `raise` the label of the nearest `try`
/// whose body holds it and which has a handler, `target` here, as the old
/// raise reached the nearest such handler.
fn infer_try_labels(blocks: &mut [MirBlock], next: &mut u32, target: Option<TryLabel>) {
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::Try {
                label,
                body,
                handler,
                orelse,
                ..
            } = instruction
            {
                *label = TryLabel(*next);
                *next += 1;
                let inner = if handler.is_some() {
                    Some(*label)
                } else {
                    target
                };
                infer_try_labels(body, next, inner);
                for region in handler.iter_mut().chain(orelse) {
                    infer_try_labels(region, next, target);
                }
                continue;
            }
            for region in crate::mir::instruction_regions_mut(instruction) {
                infer_try_labels(region, next, target);
            }
        }
        if let (MirTerm::ErrorReturn, Some(label)) = (&block.term, target)
            && let Some(MirInstr::DefVar { var, .. }) = block.instrs.last_mut()
            && *var == LEGACY_FUNCTION_ERROR_SLOT
        {
            *var = LEGACY_ERROR_SLOT;
            block.term = MirTerm::TryRaise {
                label,
                values: Vec::new(),
            };
        }
    }
}

/// The slot a store [`legacy_error_store`] makes before a raise naming a
/// `try` names until [`upgrade_error_slots`] names the `try`'s.
const LEGACY_ERROR_SLOT: VarId = VarId::MAX;

/// The slot a store [`legacy_error_store`] makes before a raise out of the
/// function names until [`upgrade_function_error_slot`] names the
/// function's.
const LEGACY_FUNCTION_ERROR_SLOT: VarId = VarId::MAX - 1;

/// The store of the error `src` of a raise older than MIR 1.108 into the
/// placeholder `slot`, which [`upgrade_error_slots`] and
/// [`upgrade_function_error_slot`] fill in.
const fn legacy_error_store(slot: VarId, src: crate::mir::Reg) -> MirInstr {
    MirInstr::DefVar {
        var: slot,
        src,
        binding_ty: None,
        in_place: false,
    }
}

/// Name each `try`'s error slot in `function`, read from text older than
/// MIR 1.107, where a raise carried its error to the handler as an
/// operand: the stores [`legacy_error_store`] made before each raise
/// naming a `try` store into that `try`'s slot, its handler's old binding
/// or a fresh `$try_error<N>` typed as the first raise's error, declared
/// before the `try`.
pub(super) fn upgrade_error_slots(function: &mut MirFunction) {
    let mut blocks = std::mem::take(&mut function.blocks);
    upgrade_blocks(&mut blocks, function);
    function.blocks = blocks;
}

/// Give `function`, read from text older than MIR 1.108, the error slot it
/// raises out through when it raises or holds a raise out of it, a fresh
/// `$error` typed as its error: the stores [`legacy_error_store`] made
/// before each raise out of it store into the slot.
pub(super) fn upgrade_function_error_slot(function: &mut MirFunction) {
    let slot = VarId::try_from(function.n_vars).unwrap_or(VarId::MAX);
    let ty = function.error_ty.clone().unwrap_or_else(Ty::error);
    if !name_function_error_slot(&mut function.blocks, slot, &ty) && !function.raises {
        return;
    }
    function.n_vars += 1;
    function.var_names.push("$error".to_string());
    function.var_tys.insert(slot, ty);
    function.error_slot = Some(slot);
}

/// Name `slot` in each store [`legacy_error_store`] made before a raise
/// out of the function in `blocks`, typing it `ty`; whether there was one.
fn name_function_error_slot(blocks: &mut [MirBlock], slot: VarId, ty: &Ty) -> bool {
    let mut named = false;
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::DefVar {
                var, binding_ty, ..
            } = instruction
                && *var == LEGACY_FUNCTION_ERROR_SLOT
            {
                *var = slot;
                *binding_ty = Some(ty.clone());
                named = true;
            }
            for region in crate::mir::instruction_regions_mut(instruction) {
                named |= name_function_error_slot(region, slot, ty);
            }
        }
    }
    named
}

fn upgrade_blocks(blocks: &mut [MirBlock], function: &mut MirFunction) {
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            let mut declare = None;
            if let MirInstr::Try {
                label, error, body, ..
            } = &mut block.instrs[index]
            {
                let mut slot = *error;
                let mut fresh = false;
                legacy_stores(body, *label, &mut |var, binding_ty, src| {
                    let slot = *slot.get_or_insert_with(|| {
                        fresh = true;
                        let var = VarId::try_from(function.n_vars).unwrap_or(VarId::MAX);
                        function.n_vars += 1;
                        function.var_names.push(format!("$try_error{}", label.0));
                        if let Some(ty) = function.reg_types.get(&src.0).cloned() {
                            function.var_tys.insert(var, ty);
                        }
                        var
                    });
                    *var = slot;
                    *binding_ty = function.var_tys.get(&slot).cloned();
                });
                if fresh {
                    *error = slot;
                    declare = slot;
                }
            }
            for region in crate::mir::instruction_regions_mut(&mut block.instrs[index]) {
                upgrade_blocks(region, function);
            }
            if let Some(var) = declare {
                block.instrs.insert(index, MirInstr::DeclareVar { var });
                index += 1;
            }
            index += 1;
        }
    }
}

/// Visit each store [`legacy_error_store`] made before a raise in `body`
/// naming the `try` labelled `label`, resolved as
/// [`crate::mir::try_raises_mut`] resolves the raise.
fn legacy_stores(
    body: &mut [MirBlock],
    label: TryLabel,
    visit: &mut impl FnMut(&mut VarId, &mut Option<Ty>, crate::mir::Reg),
) {
    for block in body {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::Try {
                    label: inner,
                    handler,
                    orelse,
                    ..
                } if *inner == label => {
                    for region in handler.iter_mut().chain(orelse) {
                        legacy_stores(region, label, visit);
                    }
                }
                instruction => {
                    for region in crate::mir::instruction_regions_mut(instruction) {
                        legacy_stores(region, label, visit);
                    }
                }
            }
        }
        if matches!(block.term, MirTerm::TryRaise { label: found, .. } if found == label)
            && let Some(MirInstr::DefVar {
                var,
                src,
                binding_ty,
                ..
            }) = block.instrs.last_mut()
            && *var == LEGACY_ERROR_SLOT
        {
            visit(var, binding_ty, *src);
        }
    }
}
