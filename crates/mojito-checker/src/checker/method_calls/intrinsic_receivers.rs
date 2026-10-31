//! Method calls a receiver's type answers without a declared signature.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl Checker {
    /// Type a call the receiver's type answers without a declared signature:
    /// the compiler-known collections, `SIMD` and `DType`, the inverted
    /// `write_to`, string literals, tuples, packs, pointers, and uninit
    /// storage. `None` leaves the call to
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
        // A string literal's declared member is its struct's body.
        if matches!(obj_ty.kind(), TyKind::StringLiteral(Some(_)))
            && self.declared_receiver_member(obj_ty, method).is_some()
        {
            return Ok(None);
        }
        if let TyKind::Struct(name, _) = obj_ty.kind()
            && !self.structs.contains_key(name)
            && let Some(ty) = self.infer_unregistered_collection_method(site)?
        {
            return Ok(Some(ty));
        }
        if *obj_ty == TyKind::Dtype && mojito_ast::ast::DTYPE_PREDICATES.contains(&method) {
            return self.infer_dtype_predicate(site).map(Some);
        }
        // A name the compiler-known SIMD methods lack falls through to the
        // members every builtin scalar shares (`__hash__`, `write_to`,
        // `__abs__`, …).
        if let TyKind::Simd { dtype, .. } = obj_ty.kind() {
            match self.infer_simd_method(site, dtype) {
                Err(TypeError::NoSuchMethod { .. }) => {}
                result => return result.map(Some),
            }
        }
        // Raw-seam compatibility only: with the linked stdlib present the
        // nominal prelude `Optional` owns its full method surface below.
        if matches!(obj_ty.kind(), TyKind::Struct(name, args) if name == "Optional" && matches!(args.as_slice(), [TyArg::Ty(ty)] if *ty == *TyKind::INT))
            && !self.structs.contains_key("Optional")
        {
            return self.infer_raw_optional_method(site).map(Some);
        }
        // `x.write_to(writer)` where `x` has no `write_to` body of its own — a
        // `Writable`-bounded type parameter (the erased generic body) or a
        // built-in Writable value (`Bool`, a literal, a `StringSpan`) — is
        // `writer.write(x)`: the checker records the operand swap and MIR
        // lowers the `Writer.write` shape, whose formatting dispatches a
        // struct argument through its own `write_to` (instance-clone aware)
        // and a builtin directly. The nominal String takes the same shape
        // (its `write_to` body is the literal bridge, which `Writer.write`
        // already spells on both backends); every other struct receiver,
        // and a `SIMD` value, whose members are declared, keeps the ordinary
        // method path to its own `write_to`.
        let receiver = match obj_ty.kind() {
            TyKind::Ref(reference) => &reference.referent,
            _ => obj_ty,
        };
        if matches!(method, "write_to" | "write_repr_to")
            && args.len() == 1
            && kwargs.is_empty()
            && param_args.is_empty()
            && !matches!(
                receiver.kind(),
                TyKind::Struct(name, targs)
                    if !(targs.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name))
            )
            && !(matches!(receiver.kind(), TyKind::Simd { .. })
                && self.declared_receiver_member(receiver, method).is_some())
            && self.conforms_to(obj_ty, "Writable")
        {
            return self.infer_inverted_write(site).map(Some);
        }
        if method == STRING_FORMAT
            && obj_ty.is_string_literal()
            && self
                .structs
                .contains_key(mojito_symbol::symbol::STDLIB_FORMAT_UTILS_STRUCT)
        {
            return self.infer_literal_format(site).map(Some);
        }
        if obj_ty.is_string_literal()
            && args.is_empty()
            && let Some(ty) = string_literal_primitive(method)
        {
            reject_kwargs(kwargs)?;
            return Ok(Some(ty));
        }
        // A heterogeneous pack that is still a parameter answers its length;
        // any other member waits for the specialization's concrete storage.
        if let TyKind::VariadicPack(element) = obj_ty.kind()
            && mojito_types::types::pack_spread(std::slice::from_ref(element)).is_some()
        {
            reject_kwargs(kwargs)?;
            if method == "__len__" && args.is_empty() && param_args.is_empty() {
                // Upstream's `__len__` forwards to the static
                // `Self.Ts.length`: the call reads no runtime value.
                if let Some(pack) = self.collector_pack(object) {
                    self.record_pack_query_value(
                        span.clone(),
                        &pack,
                        mojito_types::param_expr::PackQuery::Length,
                    );
                }
                return Ok(Some(Ty::int()));
            }
            return Err(TypeError::SymbolicBoundary(format!(
                "method '{method}' of an unbound pack"
            )));
        }
        // Built-in `Pointer` methods: the public unsafe_* operation
        // vocabulary. `unsafe_write(copy=v)` is the one keyword shape; every
        // other pointer method rejects kwargs inside.
        if let TyKind::Pointer {
            element: elem,
            origin,
        } = obj_ty.kind()
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

    /// A `List` or `Set` receiver no linked declaration registers.
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
                Ok(Ty::none())
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            }),
        }
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
                if coerces(&types[0], &Ty::int()) {
                    Ok(Ty::int())
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
        self.record_at(object.source_span())
            .borrowed_read_call_place = true;
        self.infer_print(std::slice::from_ref(object), &[])?;
        self.record_at(span.clone()).adjustment = Some(Box::new(if method == "write_repr_to" {
            mojito_checked::checked::SemanticAdjustment::InvertedReprWrite {
                repr: self.builtin_string_result(),
            }
        } else {
            mojito_checked::checked::SemanticAdjustment::InvertedWrite
        }));
        Ok(Ty::none())
    }

    /// `"…".format(args)`: a string literal declares no methods yet, so the
    /// call types as the static call of the bundled stand-in for upstream's
    /// `StringLiteral.format`, `_FormatUtils.format_literal(literal, args…)`,
    /// and records the adjustment MIR lowers it by.
    fn infer_literal_format(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span, object, call, ..
        } = site;
        let arguments: Vec<Expr> = std::iter::once(object.clone())
            .chain(call.args.iter().cloned())
            .collect();
        let ty = self.infer_struct_static_method(
            span.clone(),
            mojito_symbol::symbol::STDLIB_FORMAT_UTILS_STRUCT,
            (&[], None),
            mojito_symbol::symbol::FORMAT_LITERAL_METHOD,
            MethodCallArguments {
                args: &arguments,
                parameterized_syntax: false,
                ..call
            },
        )?;
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::LiteralFormat,
        ));
        Ok(ty)
    }
}

/// Upstream's `StringLiteral` byte primitives (`pop.string.size` /
/// `pop.string.address`): the literal's UTF-8 bytes live for the whole
/// program, so the pointer carries the static origin.
fn string_literal_primitive(method: &str) -> Option<Ty> {
    match method {
        "byte_length" => Some(Ty::int()),
        "ptr" | "unsafe_ptr" => Some(mojito_types::types::static_string_address_ty()),
        _ => None,
    }
}

const STRING_FORMAT: &str = "format";
