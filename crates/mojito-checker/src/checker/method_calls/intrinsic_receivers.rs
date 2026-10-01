//! Method calls a receiver's type answers without a declared signature.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Type a call the receiver's type answers without a declared signature:
    /// the compiler-known collections, `SIMD` and `DType`, slice descriptors,
    /// the inverted `write_to`, `Writer`/`Hasher` bounds, string literals,
    /// tuples, packs, pointers, and uninit storage. `None` leaves the call to
    /// signature resolution.
    pub(super) fn infer_intrinsic_receiver_call(
        &self,
        site: MethodCallSite<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        if let Ty::Struct(name, _) = obj_ty
            && !self.structs.contains_key(name)
            && let Some(ty) = self.infer_unregistered_collection_method(site)?
        {
            return Ok(Some(ty));
        }
        // `v.to_bits[DType.target]()` — upstream's lane-wise bit
        // reinterpretation of a SIMD value (the native scalars are width-1
        // vectors; `Bool` has no `to_bits` upstream, `Scalar[DType.bool]`
        // does). The target must be unsigned and at least as wide as the
        // source lane; it defaults to the unsigned dtype of the source width.
        if method == "to_bits"
            && args.is_empty()
            && param_args.len() <= 1
            && !matches!(obj_ty, Ty::Bool | Ty::IntLiteral | Ty::FloatLiteral)
            && let Some((source, width)) = simd_slots(obj_ty)
        {
            return self.infer_simd_to_bits(site, &source, width).map(Some);
        }
        if *obj_ty == Ty::Dtype && mojito_ast::ast::DTYPE_PREDICATES.contains(&method) {
            return self.infer_dtype_predicate(site).map(Some);
        }
        // `__hash__` is the builtin hashable-leaf arm's, shared with the
        // other builtin scalars.
        if let Ty::Simd { dtype, width } = obj_ty
            && method != "__hash__"
        {
            return self.infer_simd_method(site, dtype, width).map(Some);
        }
        if let Ty::Struct(kind, targs) = obj_ty
            && matches!(kind.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
            && targs.is_empty()
        {
            return self.infer_slice_descriptor_method(site, kind).map(Some);
        }
        // Raw-seam compatibility only: with the linked stdlib present the
        // nominal prelude `Optional` owns its full method surface below.
        if matches!(obj_ty, Ty::Struct(name, args) if name == "Optional" && matches!(args.as_slice(), [TyArg::Ty(Ty::Int)]))
            && !self.structs.contains_key("Optional")
        {
            return self.infer_raw_optional_method(site).map(Some);
        }
        // `x.write_to(writer)` where `x` has no `write_to` body of its own — a
        // `Writable`-bounded type parameter (the erased generic body) or a
        // built-in Writable value (`Int`, `Float64`, `Bool`, a literal, a
        // SIMD vector, a `StringSpan`) — is `writer.write(x)`: the checker
        // records the operand swap and MIR lowers the `Writer.write` shape,
        // whose formatting dispatches a struct argument through its own
        // `write_to` (instance-clone aware) and a builtin directly. The
        // nominal String takes the same shape (its `write_to` body is the
        // literal bridge, which `Writer.write` already spells on both
        // backends); every other struct receiver keeps the ordinary method
        // path to its own `write_to`.
        if matches!(method, "write_to" | "write_repr_to")
            && args.len() == 1
            && kwargs.is_empty()
            && param_args.is_empty()
            && !matches!(
                match obj_ty {
                    Ty::Ref(reference) => &*reference.referent,
                    other => other,
                },
                Ty::Struct(name, targs)
                    if !(targs.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name))
            )
            && (matches!(obj_ty, Ty::Struct(name, args)
                    if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                        && args.is_empty())
                || self.conforms_to(obj_ty, "Writable"))
        {
            return self.infer_inverted_write(site).map(Some);
        }
        if self.conforms_to(obj_ty, "Writer") && method == "write" {
            return self.infer_writer_write(site).map(Some);
        }
        if matches!(obj_ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Hasher"))
            && let Some(ty) = self.infer_hasher_bound_method(site)?
        {
            return Ok(Some(ty));
        }
        if method == "format"
            && (*obj_ty == Ty::StringLiteral
                || matches!(obj_ty, Ty::Struct(name, args)
                    if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name)))
        {
            // A template receiver may be the compile-time literal or the
            // nominal String; the formatted result materializes nominally.
            reject_kwargs(kwargs)?;
            self.infer_print(args, &[])?;
            return self.nominal_string_wrap(span.clone()).map(Some);
        }
        if *obj_ty == Ty::StringLiteral
            && args.is_empty()
            && let Some(ty) = string_literal_primitive(method)
        {
            reject_kwargs(kwargs)?;
            return Ok(Some(ty));
        }
        if let Ty::Tuple(elements) = obj_ty {
            reject_kwargs(kwargs)?;
            return self
                .infer_tuple_method(span, object, method, elements, call)
                .map(Some);
        }
        // A heterogeneous pack that is still a parameter answers its length;
        // any other member waits for the specialization's concrete storage.
        if let Ty::VariadicPack(element) = obj_ty
            && mojito_types::types::pack_spread(std::slice::from_ref(element)).is_some()
        {
            reject_kwargs(kwargs)?;
            if method == "__len__" && args.is_empty() && param_args.is_empty() {
                return Ok(Some(Ty::Int));
            }
            return Err(TypeError::SymbolicBoundary(format!(
                "method '{method}' of an unbound pack"
            )));
        }
        // Built-in `Pointer` methods: the public unsafe_* operation
        // vocabulary. `unsafe_write(copy=v)` is the one keyword shape; every
        // other pointer method rejects kwargs inside.
        if let Ty::Pointer {
            element: elem,
            origin,
        } = obj_ty
        {
            return self
                .infer_pointer_method(span, object, method, elem, origin, param_args, args, kwargs)
                .map(Some);
        }
        // Compiler-private inline uninit storage (`MaybeUninit`'s field):
        // the write/take/destroy crossing vocabulary.
        if let Some(element) = mojito_types::types::uninit_storage_element(obj_ty) {
            let element = element.clone();
            reject_kwargs(kwargs)?;
            return self
                .infer_uninit_storage_method(span, object, method, &element, args)
                .map(Some);
        }
        Ok(None)
    }

    /// A `List`, `Set`, or `Tuple` receiver no linked declaration registers.
    fn infer_unregistered_collection_method(
        &self,
        site: MethodCallSite<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        if let Some(element) = list_element(obj_ty) {
            reject_kwargs(kwargs)?;
            let result = self.infer_list_method(object, method, element, args)?;
            if matches!(
                method,
                "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
            ) {
                self.record_interior_invalidation(span.clone(), object);
            }
            return Ok(Some(result));
        }
        if let Some(element) = set_element(obj_ty) {
            return self.infer_set_method(site, element).map(Some);
        }
        if let Some(elements) = tuple_elements(obj_ty) {
            reject_kwargs(kwargs)?;
            let elements = elements.into_iter().cloned().collect::<Vec<_>>();
            return self
                .infer_tuple_method(span, object, method, &elements, call)
                .map(Some);
        }
        Ok(None)
    }

    /// The compiler-known `Set` surface.
    fn infer_set_method(&self, site: MethodCallSite<'_>, element: &Ty) -> Result<Ty, TypeError> {
        let MethodCallSite {
            object,
            method,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        reject_kwargs(kwargs)?;
        match method {
            "add" => {
                self.check_place(object)?;
                let values = self.builtin_args("Set.add", 1, args)?;
                if !coerces(&values[0], element) {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: values[0].to_string(),
                        context: "Set.add value".to_string(),
                    });
                }
                self.check_consuming(&args[0], &values[0], "Set.add value")?;
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// A `Slice`/`ContiguousSlice`/`StridedSlice` descriptor's methods.
    fn infer_slice_descriptor_method(
        &self,
        site: MethodCallSite<'_>,
        kind: &str,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        reject_kwargs(kwargs)?;
        // `Slice.__eq__`/`__ne__` (only `Slice` is Equatable upstream).
        if matches!(method, "__eq__" | "__ne__") && kind == "Slice" {
            let types = self.builtin_args(&format!("Slice.{method}"), 1, args)?;
            if !matches!(&types[0], Ty::Struct(name, args) if name == "Slice" && args.is_empty()) {
                return Err(TypeError::TypeMismatch {
                    expected: "Slice".to_string(),
                    found: types[0].to_string(),
                    context: format!("Slice.{method} operand"),
                });
            }
            return Ok(Ty::Bool);
        }
        if matches!(method, "write_to" | "write_repr_to")
            && args.len() == 1
            && param_args.is_empty()
        {
            let writer_ty = self.infer(&args[0])?;
            if !self.conforms_to(&writer_ty, "Writer") {
                return Err(TypeError::TypeMismatch {
                    expected: "Writer".to_string(),
                    found: writer_ty.to_string(),
                    context: format!("argument 1 to '{method}'"),
                });
            }
            self.check_place(&args[0])?;
            self.borrowed_read_call_places
                .borrow_mut()
                .insert(object.source_span());
            self.infer_print(std::slice::from_ref(object), &[])?;
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                if method == "write_repr_to" {
                    mojito_checked::checked::SemanticAdjustment::InvertedReprWrite
                } else {
                    mojito_checked::checked::SemanticAdjustment::InvertedWrite
                },
            );
            return Ok(Ty::None);
        }
        if method != "indices" {
            return Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            });
        }
        let types = self.builtin_args("Slice.indices", 1, args)?;
        if !coerces(&types[0], &Ty::Int) {
            return Err(TypeError::TypeMismatch {
                expected: "Int".to_string(),
                found: types[0].to_string(),
                context: "Slice.indices length".to_string(),
            });
        }
        // Upstream `ContiguousSlice.indices` yields `(start, end)`; the
        // strided family keeps the three-element normalization.
        if kind == "ContiguousSlice" {
            return Ok(self.public_tuple_type(vec![Ty::Int, Ty::Int]));
        }
        Ok(self.public_tuple_type(vec![Ty::Int, Ty::Int, Ty::Int]))
    }

    /// The raw-seam `Optional[Int]`, present only without the linked stdlib.
    fn infer_raw_optional_method(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite {
            method,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        reject_kwargs(kwargs)?;
        match method {
            "or_else" => {
                let types = self.builtin_args("Optional.or_else", 1, args)?;
                if coerces(&types[0], &Ty::Int) {
                    Ok(Ty::Int)
                } else {
                    Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: types[0].to_string(),
                        context: "Optional.or_else default".to_string(),
                    })
                }
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `x.write_to(writer)` on a receiver with no `write_to` body of its own
    /// is `writer.write(x)`: record the operand swap.
    fn infer_inverted_write(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            ..
        } = site;
        let MethodCallArguments { args, .. } = call;
        let writer_ty = self.infer(&args[0])?;
        if !self.conforms_to(&writer_ty, "Writer") {
            return Err(TypeError::TypeMismatch {
                expected: "Writer".to_string(),
                found: writer_ty.to_string(),
                context: "argument 1 to 'write_to'".to_string(),
            });
        }
        self.check_place(&args[0])?;
        self.borrowed_read_call_places
            .borrow_mut()
            .insert(object.source_span());
        self.infer_print(std::slice::from_ref(object), &[])?;
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            if method == "write_repr_to" {
                mojito_checked::checked::SemanticAdjustment::InvertedReprWrite
            } else {
                mojito_checked::checked::SemanticAdjustment::InvertedWrite
            },
        );
        Ok(Ty::None)
    }

    /// `writer.write(values…)` through a `Writer` conformance.
    fn infer_writer_write(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite { object, call, .. } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        reject_kwargs(kwargs)?;
        self.check_place(object)?;
        self.borrowed_read_call_places
            .borrow_mut()
            .extend(args.iter().map(Expr::source_span));
        self.infer_print(args, &[])?;
        Ok(Ty::None)
    }

    /// The `Hasher` requirements a `Hasher`-bounded parameter answers
    /// directly; any other method resolves through the bound.
    fn infer_hasher_bound_method(&self, site: MethodCallSite<'_>) -> Result<Option<Ty>, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        reject_kwargs(kwargs)?;
        match method {
            "update" => {
                self.check_place(object)?;
                let tys = self.builtin_args("Hasher.update", 1, args)?;
                if !self.keyed_hash_leaves(span.clone(), || self.conforms_to(&tys[0], "Hashable")) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: tys[0].to_string(),
                        trait_name: "Hashable".to_string(),
                        reason: self.trait_failure_reason(&tys[0], "Hashable"),
                    });
                }
                // `update(mut self, value: Some[Hashable])` reads its
                // value: a named one is lent where it lies, not copied.
                if matches!(args[0].kind, ExprKind::Identifier(_)) {
                    self.borrowed_read_call_places
                        .borrow_mut()
                        .insert(args[0].source_span());
                }
                return Ok(Some(Ty::None));
            }
            "_update_with_bytes" => {
                self.check_place(object)?;
                let tys = self.builtin_args("Hasher._update_with_bytes", 1, args)?;
                if !matches!(&tys[0], Ty::Struct(name, _) if name.ends_with("Span")) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Span[Byte, _]".to_string(),
                        found: tys[0].to_string(),
                        context: "Hasher._update_with_bytes".to_string(),
                    });
                }
                return Ok(Some(Ty::None));
            }
            "_update_with_simd" => {
                self.check_place(object)?;
                let tys = self.builtin_args("Hasher._update_with_simd", 1, args)?;
                // The argument's own vector type keys the hasher's clone
                // (`SIMD[_, _]` infers per call); the runtime dispatch
                // computes the same clone name from the value.
                if !simd_valued_ty(&tys[0]) {
                    return Err(TypeError::TypeMismatch {
                        expected: "SIMD[_, _]".to_string(),
                        found: tys[0].to_string(),
                        context: "Hasher._update_with_simd".to_string(),
                    });
                }
                self.keyed_hash_leaves(span.clone(), || self.record_hash_leaf(&tys[0]));
                return Ok(Some(Ty::None));
            }
            "finish" if args.is_empty() => {
                return Ok(Some(canonical_simd_ty(Dtype::UInt64, 1)));
            }
            _ => {}
        }
        Ok(None)
    }
}

/// Upstream's `StringLiteral` byte primitives (`pop.string.size` /
/// `pop.string.address`): the literal's UTF-8 bytes live for the whole
/// program, so the pointer carries the static origin.
fn string_literal_primitive(method: &str) -> Option<Ty> {
    match method {
        "byte_length" => Some(Ty::Int),
        "ptr" | "unsafe_ptr" => Some(Ty::Pointer {
            element: Box::new(canonical_simd_ty(Dtype::UInt8, 1)),
            origin: mojito_types::origin::PointerOrigin::Static,
        }),
        _ => None,
    }
}
