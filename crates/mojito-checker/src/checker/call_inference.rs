//! Free-function and callable-value call inference: `infer_call` dispatch,
//! callable-type application, generic-call instantiation, and forwarded-kwargs
//! element typing. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::constraints::LACKING_EVIDENCE;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_symbol::symbol::TypeKey;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};

/// A call typed against one callable: its result, rank, error type, the
/// erased-origin bindings, and how many arguments with a lane it binds to a
/// bare type parameter (`CallableOverloadMatch::simd_erasures`).
/// A typed call: its result, overload rank, raised error, bool origin
/// bindings, SIMD erasures, and what each of its bracket arguments bound to.
type InferredCall = (
    Ty,
    usize,
    Option<Ty>,
    HashMap<usize, bool>,
    usize,
    BracketAttributes,
);

impl Checker {
    /// Reject a reference to the module function `name` as a value when
    /// it is `@unavailable`, as a call selecting it is.
    pub(super) fn reject_unavailable_reference(&self, name: &str) -> Result<(), TypeError> {
        if self.binding_scope(name) != Some(0) {
            return Ok(());
        }
        match self.lookup_callable_origins(name).as_deref() {
            Some([signature]) => reject_unavailable(Some(signature)),
            _ => Ok(()),
        }
    }

    fn erased_origin_constraint_environment(
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Vec<(String, TyArg)> {
        bindings
            .iter()
            .filter_map(|(index, value)| {
                signature
                    .source
                    .get(*index)
                    .map(|parameter| (parameter.name.clone(), TyArg::Val(CtValue::Bool(*value))))
            })
            .collect()
    }

    fn erased_origin_constraint_applies(
        &self,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> bool {
        if signature.availability.is_empty() {
            return true;
        }
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = ConstraintEnvironment::named(
            &[],
            owned.iter().map(|(name, value)| (name.as_str(), value)),
        );
        signature
            .availability
            .iter()
            .all(|constraint| self.eval_generic_constraint(constraint, &environment))
    }

    /// The retained diagnostic for a failed erased-origin availability check:
    /// the first failing clause's message, when it declared one.
    fn first_failing_erased_origin_message(
        &self,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Option<String> {
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = ConstraintEnvironment::named(
            &[],
            owned.iter().map(|(name, value)| (name.as_str(), value)),
        );
        signature
            .availability
            .iter()
            .find(|constraint| !self.eval_generic_constraint(constraint, &environment))
            .and_then(|constraint| match constraint {
                GenericConstraint::WithMessage(_, message) => {
                    Some(format!("constraint failed: {message}"))
                }
                _ => None,
            })
    }

    /// A call of a built-in or a struct construction: a name no binding
    /// shadows.
    fn infer_unbound_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        match name {
            _ if let Some(constructed) =
                self.declared_simd_spelling(&span, name, param_args, args, kwargs) =>
            {
                constructed
            }
            _ if let Some(constructed) =
                self.string_literal_construction(&span, name, param_args, args, kwargs) =>
            {
                constructed
            }
            _ if self.structs.contains_key(name) && name != mojito_types::types::SIMD_TYPE_NAME => {
                // `String(x)` on a non-literal argument stringifies through
                // the builtin conversion, which builds the nominal struct
                // itself: the call routes to the `"String"` builtin (a
                // `ResolveCallable` adjustment) and yields an owned
                // `String`, as upstream's `String.__init__[T: Writable]`
                // does. A single string-literal argument, a keyword
                // construction such as `String(copy: s)`, and a sequence of
                // `Writable` arguments are the ordinary constructor path.
                if mojito_symbol::symbol::is_stdlib_string_struct(name)
                    && args.len() == 1
                    && kwargs.is_empty()
                    && param_args.is_empty()
                    && !(args.len() == 1
                        && matches!((self.infer(&args[0])?).kind(), TyKind::StringLiteral(_)))
                {
                    self.infer_stringify(args)?;
                    self.record_at(span).overload_target = Some("String".to_string());
                    return Ok(Ty::new(TyKind::Struct(name.to_string(), Vec::new())));
                }
                let constructed = self.infer_construction(&span, name, param_args, args, kwargs)?;
                self.type_runtime_param_args(param_args)?;
                Ok(constructed)
            }
            "Pointer" | "UnsafePointer" if !kwargs.is_empty() => {
                self.infer_pointer_to(span, param_args, args, kwargs)
            }
            "__get_mvalue_as_litref" => self.infer_mvalue_as_litref(span, param_args, args, kwargs),
            // Compiler-private tagged-union storage:
            // `__VariantStorage[T0, ...](value^)` selects the alternative
            // by the payload's type; `(init_with=factory)` by the
            // factory's result type.
            // An `enum`'s storage: `__EnumStorage[T0, ...](payload,
            // index=i)` builds case `i` from its payload, by index, since
            // cases may share a payload type.
            _ if name == mojito_types::types::ENUM_STORAGE_TYPE_NAME => {
                if !self.enum_storage_allowed() {
                    return Err(TypeError::Unsupported(format!(
                        "'{name}' is the storage of an 'enum'; declare one instead"
                    )));
                }
                self.infer_enum_storage_construction(span, param_args, args, kwargs)
            }
            _ if name == mojito_types::types::VARIANT_STORAGE_TYPE_NAME => {
                if !self.bundled_stdlib_declaration {
                    return Err(TypeError::Unsupported(format!(
                        "'{name}' is compiler-private storage; use Variant from std.utils"
                    )));
                }
                self.infer_variant_construction(span, param_args, args, kwargs)
            }
            // Compiler-private inline uninit storage: `__UninitStorage[T]()`
            // (uninitialized) or `__UninitStorage[T](value^)` (initialized),
            // reachable only from the bundled crossing module.
            _ if name == mojito_types::types::UNINIT_STORAGE_TYPE_NAME => {
                reject_kwargs(kwargs)?;
                self.infer_uninit_storage_construction(span, param_args, args)
            }
            "print" => self.infer_print(args, kwargs),
            // A well-formed `rebind` was erased before checking; one
            // that reaches here is malformed.
            "rebind" => Err(Self::rebind_shape_error(param_args, args)),
            // One the elaborator left is over a binder of a template
            // body (`materialize[names[i]]()`): the runtime value of its
            // compile-time operand, which MIR lowers in its place.
            "materialize" if args.is_empty() && kwargs.is_empty() => {
                self.infer_template_materialize(&span, param_args)
            }
            // One the elaborator left is over a binder of a template
            // body (`comptime(len(L))`): the runtime value of its
            // compile-time operand, as `materialize[...]()` is.
            "comptime" if args.len() == 1 && param_args.is_empty() && kwargs.is_empty() => {
                self.infer_template_comptime(&args[0])
            }
            // `conforms_to(T, Tr)`: a compile-time `Bool` read as a
            // runtime value, the parameter constant of its query.
            "conforms_to" if param_args.is_empty() && kwargs.is_empty() => {
                let [subject, traits] = args else {
                    return Err(TypeError::ArityMismatch {
                        name: "conforms_to".to_string(),
                        expected: 2,
                        got: args.len(),
                    });
                };
                let value = self.conformance_query(subject, traits)?;
                self.record_member_value(span, value);
                Ok(Ty::bool())
            }
            // `Int(copy=x)`: a built-in value type's `Copyable`
            // initializer.
            _ if args.is_empty()
                && matches!(kwargs, [keyword] if keyword.name == "copy")
                && let Some((ty, attributes)) = self.builtin_copy_constructed(name, param_args) =>
            {
                self.record_bracket_attributes(span.clone(), attributes);
                self.infer_builtin_copy_construction(span, name, &ty, &kwargs[0])
            }
            // `SIMD[dt, n](mlir_value=v)`: upstream's storage initializer.
            _ if args.is_empty()
                && matches!(kwargs, [keyword] if keyword.name == "mlir_value")
                && let Some((ty, attributes)) = self.builtin_copy_constructed(name, param_args)
                && simd_slots(&ty).is_some() =>
            {
                self.record_bracket_attributes(span.clone(), attributes);
                self.infer_storage_construction(span, name, &ty, &kwargs[0])
            }
            // `SIMD(mlir_value=v)`, both parameters inferred from `v`.
            "SIMD" if let Some(storage) = lone_storage_keyword(param_args, args, kwargs) => {
                self.infer_inferred_storage_construction(span, name, storage)
            }
            // `Bool(value=i)`: upstream's initializer from an `i1`, or its
            // `Boolable` initializer at a `Bool`, the overload the keyword
            // also names.
            "Bool"
                if args.is_empty()
                    && param_args.is_empty()
                    && matches!(kwargs, [keyword] if keyword.name == "value") =>
            {
                let source = if self.infer(&kwargs[0].value)? == Ty::i1() {
                    Ty::i1()
                } else {
                    Ty::bool()
                };
                self.infer_storage_construction(span, name, &source, &kwargs[0])
                    .map(|_| Ty::bool())
            }
            // `Bool(mlir_value=s)`: upstream's initializer from the
            // `!kgen.scalar<bool>` a `pop.simd.reduce_and` gives, which
            // Mojito holds as the one-lane `bool` vector the `Bool` casts.
            "Bool"
                if args.is_empty()
                    && param_args.is_empty()
                    && matches!(kwargs, [keyword] if keyword.name == "mlir_value") =>
            {
                self.infer_storage_construction(
                    span.clone(),
                    name,
                    &simd_ty(Dtype::Bool, 1),
                    &kwargs[0],
                )?;
                self.record_at(span).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ScalarCast(
                        mojito_types::ct::ScalarConversion::Bool,
                    ),
                ));
                Ok(Ty::bool())
            }
            _ if !kwargs.is_empty() => Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "keyword arguments are not supported here".to_string(),
            }),
            // Upstream's libc FFI spelling over Mojito's closed callee
            // table (`mojito_types::ffi`); the backends read the callee
            // from the call's instantiated arguments and the result type
            // from the destination register.
            "external_call" => self.infer_external_call(&span, param_args, args),
            // The static address of a parameter list's elements, spelled
            // only by the bundled `ParameterList` (upstream's
            // `global_constant` over `#pop.variadic_to_array`).
            PARAM_LIST_ADDRESS
                if super::overload_support::is_bundled_variadics_source(span.source.as_deref()) =>
            {
                self.infer_param_list_address(span, param_args, args)
            }
            // `_unqualified_type_name[T]()` (upstream `std.reflection`):
            // the unqualified spelling of one checked type, a compile-time
            // string. Recorded for every resolution — a still-generic
            // spelling names the parameter, which only the specialized
            // re-check's constant ever reaches a backend.
            "_unqualified_type_name" => {
                if param_args.len() != 1 {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "_unqualified_type_name".to_string(),
                        expected: 1,
                        got: param_args.len(),
                    });
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "_unqualified_type_name".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let ty = self.type_param_argument(&param_args[0], "_unqualified_type_name")?;
                self.record_bracket_attributes(
                    span.clone(),
                    types_attributes(param_args, std::slice::from_ref(&ty)),
                );
                self.record_at(span).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::TypeName {
                        text: mojito_symbol::symbol::unqualified_instance_name(&ty),
                        ty,
                    },
                ));
                Ok(Ty::string_literal())
            }
            // Compiler-private crossing for `std.os.abort`: an uncatchable
            // VM trap carrying a nominal String message. Typed as returning
            // None — the call never returns at runtime, and stdlib call
            // sites keep a normal control-flow path after it rather than
            // relying on bottom-type flow analysis.
            // The `with` desugar's liveness anchor: a named local kept
            // alive to this statement without a copy or a move (HIR
            // lowers the statement to `KeepAlive`).
            name if name == with_stmt::KEEP_ALIVE_BUILTIN => {
                let [argument] = args else {
                    return Err(TypeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                };
                let ExprKind::Identifier(local) = &argument.kind else {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "the argument must name a local variable".to_string(),
                    });
                };
                if self.lookup(local).is_none() {
                    return Err(TypeError::UndefinedVariable(local.clone()));
                }
                self.infer(argument)?;
                Ok(Ty::none())
            }
            "_mojito_float_text" => self.infer_float_text(args),
            "_mojito_abort" => {
                let tys = self.builtin_args("_mojito_abort", 1, args)?;
                match (tys[0]).kind() {
                    // A literal message avoids an os-module import cycle
                    // for stdlib-internal trap sites; the VM reads either
                    // spelling.
                    TyKind::StringLiteral(_) => Ok(Ty::none()),
                    TyKind::Struct(name, targs)
                        if targs.is_empty()
                            && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
                    {
                        Ok(Ty::none())
                    }
                    other => Err(TypeError::TypeMismatch {
                        expected: "String".to_string(),
                        found: other.to_string(),
                        context: "argument to '_mojito_abort'".to_string(),
                    }),
                }
            }
            // Positional `String(x)` is the stringify intrinsic; the
            // zero-argument form is the nominal empty constructor
            // (2026-08 stabilization) and falls through to it.
            "String" if !args.is_empty() => self.infer_stringify(args),
            "repr" => {
                let tys = self.builtin_args("repr", 1, args)?;
                if self.conforms_to(&tys[0], "Writable") {
                    self.record_at(args[0].source_span()).call_place_use = true;
                    return Ok(self.builtin_string_result());
                }
                Err(TypeError::TypeMismatch {
                    expected: "Writable".to_string(),
                    found: tys[0].to_string(),
                    context: "argument to 'repr'".to_string(),
                })
            }
            "abs" => self.infer_abs(args),
            "min" | "max" => self.infer_min_max(name, args),
            "round" => self.infer_round(args),
            "input" => self.infer_input(span, args),
            "len" => self.infer_len(args),
            "range" => self.infer_range(args),
            // An `IntLiteral` or `FloatLiteral` argument (R1507); every
            // other argument binds `SIMD`'s declared overloads above.
            "Int" => self.infer_conversion(&span, Ty::int(), args),
            "UInt" => self.infer_conversion(&span, Ty::uint(), args),
            "Float64" => self.infer_conversion(&span, Ty::float64(), args),
            "Bool" => self.infer_conversion(&span, Ty::bool(), args),
            "divmod" => self.infer_divmod(args),
            "SIMD" => self.infer_simd_construction(&span, param_args, args),
            // `Self(...)` in `SIMD`'s own body constructs by the rule too.
            "Self"
                if param_args.is_empty()
                    && let Some(TyKind::Simd { dtype, width }) =
                        self.self_ty.as_ref().map(Ty::kind) =>
            {
                self.construct_simd(&span, dtype, width, args)
            }
            _ if param_args.is_empty() && self.vector_alias(name).is_some() => {
                self.infer_vector_alias_construction(name, args)
            }
            "Scalar" => self.infer_scalar_construction(&span, param_args, args),
            "List" => self.infer_list_construction(&span, param_args, args),
            "Set" => {
                let (collection, attributes) = self.set_type(param_args)?;
                self.record_bracket_attributes(span, attributes);
                let element = set_element(&collection)
                    .expect("Set type helper returns a nominal Set")
                    .clone();
                for argument in args {
                    let actual = self.infer(argument)?;
                    if !coerces(&actual, &element) {
                        return Err(TypeError::TypeMismatch {
                            expected: element.to_string(),
                            found: actual.to_string(),
                            context: "Set construction element".to_string(),
                        });
                    }
                    self.record_literal_materializations(argument, &actual, &element)?;
                    self.check_consuming(argument, &actual, "Set construction element")?;
                }
                Ok(self.nominal_set(element))
            }
            "Dict" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "Dict".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let (dict, attributes) = self.dict_type(param_args)?;
                self.record_bracket_attributes(span, attributes);
                Ok(dict)
            }
            "Tuple" => {
                let tuple = self.infer_tuple_construction(&span, param_args, args)?;
                if let TyKind::Struct(target, _) = tuple.kind()
                    && target != mojito_types::types::TUPLE_TYPE_NAME
                {
                    self.record_at(span).overload_target = Some(target.clone());
                }
                Ok(tuple)
            }
            "Error" => self.infer_error_construction(args),
            _ if Dtype::from_scalar_alias(name).is_some() => {
                let dtype =
                    Dtype::from_scalar_alias(name).expect("match guard established a scalar alias");
                self.infer_simd_alias_construction(dtype, param_args, args)
            }
            _ => Err(TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// A `SIMD` construction through the `__init__` overloads `std.simd`
    /// declares, or `None` when it is the checker's rule's: its arguments
    /// select an overload not declared yet
    /// ([`Self::binds_undeclared_overload`]), or no declared one matches
    /// and a rule serves its keywords (`mlir_value=`, `copy=`) or its one
    /// positional argument into a width not known here (`SIMD[dt, _]`,
    /// R1544); any other miss is the declared selection's error, so
    /// `Int8(x)` over a struct whose `__int__` raises but which does not
    /// declare `IntableRaising` is rejected as at the pin. `constructed` is
    /// the instance an alias (`Int`, `Int32`, `Scalar[dt]`, a vector alias,
    /// `Self` in `SIMD`'s body) names without brackets.
    fn declared_simd_construction(
        &self,
        span: &SourceSpan,
        constructed: Option<&Ty>,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let name = mojito_types::types::SIMD_TYPE_NAME;
        if !self
            .structs
            .get(name)
            .is_some_and(|info| info.methods.contains_key("__init__"))
            || self.binds_undeclared_overload(args)
        {
            return None;
        }
        let construct =
            || self.infer_construction_of(span, name, constructed, param_args, args, kwargs);
        let rule_served = || {
            if kwargs.is_empty() {
                if args.len() > 1 {
                    return false;
                }
                let instance = match constructed {
                    Some(constructed) => Some(constructed.clone()),
                    None => self.simd_type(param_args).ok().map(|(ty, _)| ty),
                };
                return instance
                    .and_then(|instance| simd_slots(&instance))
                    .is_none_or(|(_, width)| width.known().is_none());
            }
            kwargs
                .iter()
                .all(|keyword| matches!(keyword.name.as_str(), "mlir_value" | "copy"))
        };
        // An explicit initializer list binds only a declared initializer.
        let explicit_own = self.spelled_initializer_args.borrow().contains_key(span);
        if self.probe(construct).is_err() && !explicit_own && rule_served() {
            return None;
        }
        Some(construct().and_then(|ty| {
            self.type_runtime_param_args(param_args)?;
            Ok(ty)
        }))
    }

    /// A construction of a `StringLiteral[value]` instance (`Self` in its
    /// own members), or `None` for any other call. Upstream's struct has no
    /// fields, so its default initializer and its copy each build the one
    /// value the type names: the elaborator writes the closed type's
    /// default construction, the literal of its text, while the
    /// [`Ty::new`] bridge holds the instance as the compiler's literal
    /// type (R1660).
    fn string_literal_construction(
        &self,
        span: &SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let copy = matches!(kwargs, [keyword] if keyword.name == "copy");
        if !args.is_empty() || !(kwargs.is_empty() || copy) {
            return None;
        }
        let spelled = name == mojito_types::types::STRING_LITERAL_TYPE_NAME
            && !param_args.is_empty()
            && self.structs.contains_key(name);
        let instance = match name {
            "Self" if param_args.is_empty() && self.lookup(name).is_none() => self.self_ty.clone(),
            _ if spelled => self
                .probe(|| self.infer_construction(span, name, param_args, &[], &[]))
                .ok(),
            _ => None,
        }
        .filter(|ty| matches!(ty.kind(), TyKind::StringLiteral(Some(_))))?;
        let construct = || {
            // The application's arguments are typed as the instance's.
            if spelled {
                self.infer_construction(span, name, param_args, &[], &[])?;
            }
            if copy {
                self.infer_builtin_copy_construction(span.clone(), name, &instance, &kwargs[0])?;
            } else {
                self.record_at(span.clone()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ConstructType {
                        ty: instance.clone(),
                    },
                ));
            }
            self.type_runtime_param_args(param_args)?;
            Ok(instance.clone())
        };
        Some(construct())
    }

    /// A construction the checker spelled for a type with no source
    /// spelling, bound to `instance`: a struct's `__init__` overloads solve
    /// its parameters against the instance, and a `SIMD` takes its declared
    /// initializers before the checker's rule, as an alias's does.
    fn infer_instance_construction(
        &self,
        span: &SourceSpan,
        instance: &Ty,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        match instance.kind() {
            // `copy=` and an undeclared `mlir_value=` are the checker's
            // rules for the instance, as for a spelled `SIMD[dt, n]`.
            TyKind::Simd { .. }
                if args.is_empty()
                    && let [keyword] = kwargs
                    && matches!(keyword.name.as_str(), "mlir_value" | "copy") =>
            {
                let name = mojito_types::types::SIMD_TYPE_NAME;
                if keyword.name == "copy" {
                    self.infer_builtin_copy_construction(span.clone(), name, instance, keyword)
                } else {
                    self.declared_simd_construction(span, Some(instance), &[], args, kwargs)
                        .unwrap_or_else(|| {
                            self.infer_storage_construction(span.clone(), name, instance, keyword)
                        })
                }
            }
            TyKind::Simd { dtype, width } => self
                .declared_simd_construction(span, Some(instance), &[], args, kwargs)
                .unwrap_or_else(|| {
                    super::calls::reject_kwargs(kwargs)?;
                    self.construct_simd(span, dtype, width, args)
                }),
            // An origin slot the arguments leave open is the instance's.
            TyKind::Struct(name, expected) => self
                .infer_construction_of(span, name, Some(instance), &[], args, kwargs)
                .map(|constructed| match constructed.kind() {
                    TyKind::Struct(name, arguments) if arguments.len() == expected.len() => {
                        let arguments = arguments
                            .iter()
                            .zip(expected)
                            .map(|(argument, expected)| match (argument, expected) {
                                (
                                    TyArg::Origin(mojito_types::origin::Origin::Unbound),
                                    TyArg::Origin(_),
                                ) => expected.clone(),
                                _ => argument.clone(),
                            })
                            .collect();
                        TyKind::Struct(name.clone(), arguments).into()
                    }
                    _ => constructed,
                }),
            // A literal's default construction is the literal of its text,
            // symbolic until the instance is elaborated
            // (`StringLiteral.__add__`'s `{}`).
            TyKind::StringLiteral(Some(_)) if args.is_empty() && kwargs.is_empty() => {
                self.record_at(span.clone()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ConstructType {
                        ty: instance.clone(),
                    },
                ));
                Ok(instance.clone())
            }
            // A reflected field type's default construction.
            TyKind::Dependent(_) if args.is_empty() && kwargs.is_empty() => {
                self.construct_dependent(span.clone(), instance.clone())
            }
            _ => Err(TypeError::Unsupported(format!(
                "cannot emit initializer list for '{instance}': the type has no construction \
                 to spell"
            ))),
        }
    }

    /// `Scalar[dt](...)` by the checker's rule: a one-lane `SIMD`
    /// construction.
    fn infer_scalar_construction(
        &self,
        span: &SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let [dtype_arg] = param_args else {
            return Err(TypeError::WrongTypeArgCount {
                name: "Scalar".to_string(),
                expected: 1,
                got: param_args.len(),
            });
        };
        let dtype = self.dtype_from_arg(dtype_arg)?;
        self.record_bracket_attributes(span.clone(), vec![dtype_attribute(dtype_arg, &dtype)]);
        let ty = simd_of(dtype.clone(), SimdWidth::Known(1))?;
        if let [arg] = args
            && self.simd_length_construction(span, &ty, arg, &self.infer(arg)?)?
        {
            return Ok(ty);
        }
        self.check_simd_args(&dtype, &SimdWidth::Known(1), args)?;
        Ok(ty)
    }

    /// A `SIMD` construction however it is spelled (`SIMD[...]`,
    /// `Scalar[dt]`, a sized or vector alias, `Self` in `SIMD`'s body)
    /// through its declared initializers, or `None` for any other call or
    /// one the checker's rule serves ([`Self::declared_simd_construction`]).
    fn declared_simd_spelling(
        &self,
        span: &SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        match (name, param_args) {
            (mojito_types::types::SIMD_TYPE_NAME, _) => {
                self.declared_simd_construction(span, None, param_args, args, kwargs)
            }
            (_, []) => {
                let alias = self.simd_alias_instance(name)?;
                self.declared_simd_construction(span, Some(&alias), &[], args, kwargs)
            }
            ("Scalar", [dtype_arg]) => {
                let dtype = self.dtype_from_arg(dtype_arg).ok()?;
                let alias = simd_of(dtype.clone(), SimdWidth::Known(1)).ok()?;
                let constructed =
                    self.declared_simd_construction(span, Some(&alias), &[], args, kwargs)?;
                Some(constructed.and_then(|constructed| {
                    self.type_runtime_param_args(param_args)?;
                    self.record_bracket_attributes(
                        span.clone(),
                        vec![dtype_attribute(dtype_arg, &dtype)],
                    );
                    Ok(constructed)
                }))
            }
            _ => None,
        }
    }

    /// The `SIMD` instance a bracketless alias names: a scalar alias
    /// (`Int`, `Int32`), a vector alias, or `Self` in `SIMD`'s own body.
    fn simd_alias_instance(&self, name: &str) -> Option<Ty> {
        match name {
            "Int" | "UInt" | "Float64" => scalar_type_name(name),
            "Self" if self.lookup(name).is_none() => self
                .self_ty
                .clone()
                .filter(|ty| matches!(ty.kind(), TyKind::Simd { .. })),
            _ => Dtype::from_scalar_alias(name)
                .map(|dtype| simd_ty(dtype, 1))
                .or_else(|| {
                    self.vector_alias(name)
                        .map(|(dtype, width)| simd_ty(dtype, width))
                }),
        }
    }

    /// Whether a `SIMD` construction's positional arguments select, at the
    /// pin, an overload `std.simd` does not declare yet, which the checker's
    /// rule serves instead: one `IntLiteral` or `FloatLiteral` (R1507),
    /// which would otherwise reach a declared overload through an implicit
    /// conversion the pin never takes.
    fn binds_undeclared_overload(&self, args: &[Expr]) -> bool {
        let [argument] = args else {
            return false;
        };
        self.probe(|| self.infer(argument))
            .is_ok_and(|ty| matches!(ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral))
    }

    /// Check every member of a free-function overload set against the
    /// call, each inside a probe: whatever a candidate records is undone,
    /// and only the selected candidate's re-run leaves facts.
    fn probe_overload_candidates(
        &self,
        call: &OverloadCall<'_>,
        candidates: &[Ty],
        origin_signatures: &[CallableOriginSignature],
    ) -> OverloadProbes {
        let mut probes = OverloadProbes {
            matches: Vec::new(),
            availability_failures: Vec::new(),
            bound_failures: Vec::new(),
            argument_failure: None,
        };
        for (index, candidate) in candidates.iter().enumerate() {
            match self.probe(|| {
                self.check_overload_candidate(call, index, candidate, origin_signatures.get(index))
            }) {
                CandidateOutcome::Match(found) => probes.matches.push(found),
                CandidateOutcome::Unavailable(reason) => probes.availability_failures.push(reason),
                CandidateOutcome::Bound(failure) => probes.bound_failures.push(failure),
                CandidateOutcome::Argument(failure) => {
                    probes.argument_failure.get_or_insert(failure);
                }
                CandidateOutcome::Rejected => {}
            }
        }
        probes
    }

    /// Check one member of a free-function overload set against the call.
    fn check_overload_candidate(
        &self,
        call: &OverloadCall<'_>,
        index: usize,
        candidate: &Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> CandidateOutcome {
        let Ok((prepared, ordinary_param_args, _)) = self.prepare_callable_specialization(
            call.name,
            call.param_args,
            candidate.clone(),
            signature,
        ) else {
            return CandidateOutcome::Rejected;
        };
        match self.infer_callable_ty(
            call.span,
            call.name,
            prepared.clone(),
            &ordinary_param_args,
            call.args,
            call.kwargs,
        ) {
            Ok((ret, score, error, bool_bindings, simd_erasures, _)) => {
                let Some(target) = callable_lowered_name(call.name, candidate) else {
                    return CandidateOutcome::Rejected;
                };
                match signature {
                    Some(signature)
                        if !self.erased_origin_constraint_applies(signature, &bool_bindings) =>
                    {
                        CandidateOutcome::Unavailable(
                            self.first_failing_erased_origin_message(signature, &bool_bindings),
                        )
                    }
                    _ => CandidateOutcome::Match(CallableOverloadMatch {
                        ret,
                        score,
                        target,
                        index,
                        error,
                        owned: owned_parameters(candidate),
                        owned_arguments: owned_arguments(candidate, call.args, call.kwargs),
                        simd_erasures,
                    }),
                }
            }
            Err(TypeError::BadCall { reason, .. })
                if reason.starts_with("constraint failed: ")
                    || reason.starts_with("violated constraint; ")
                    || reason.starts_with(LACKING_EVIDENCE) =>
            {
                // Generic constraints are validated while solving type
                // arguments, before the final coercion and alias checks.
                // Probe the same candidate without those availability
                // predicates so only an otherwise call-compatible shape
                // can contribute a retained diagnostic.
                let mut unconstrained = prepared;
                unconstrained.update(|kind| {
                    if let Some(GenericFuncTy { decls, .. }) = kind.as_generic_func_mut() {
                        for decl in decls {
                            match decl {
                                ParamDecl::Type { constraints, .. }
                                | ParamDecl::Value { constraints, .. } => {
                                    constraints.clear();
                                }
                            }
                        }
                    }
                });
                if self
                    .infer_callable_ty(
                        call.span,
                        call.name,
                        unconstrained,
                        &ordinary_param_args,
                        call.args,
                        call.kwargs,
                    )
                    .is_ok()
                {
                    // A sole candidate's own failed assertion or unproven
                    // constraint is the call's diagnostic, as upstream
                    // reports it.
                    CandidateOutcome::Unavailable(
                        (reason.starts_with("constraint failed: ")
                            || reason.starts_with(LACKING_EVIDENCE))
                        .then_some(reason),
                    )
                } else {
                    CandidateOutcome::Rejected
                }
            }
            // A candidate whose only failure is a trait bound on a solved
            // type argument (`hash(x)` over a non-Hashable `x` against
            // `hash[T: Hashable, //, ...]`) keeps its conformance
            // diagnostic, like a single callable's.
            Err(failure @ TypeError::TraitNotSatisfied { .. }) => CandidateOutcome::Bound(failure),
            // An argument with no runtime form fails every candidate alike.
            Err(failure @ TypeError::ComptimeCrossing(_)) => CandidateOutcome::Argument(failure),
            Err(_) => CandidateOutcome::Rejected,
        }
    }

    fn validate_erased_origin_constraint(
        &self,
        name: &str,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Result<(), TypeError> {
        if signature.availability.is_empty() {
            return Ok(());
        }
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = ConstraintEnvironment::named(
            &[],
            owned.iter().map(|(name, value)| (name.as_str(), value)),
        );
        for constraint in &signature.availability {
            self.validate_constraint_in_environment(name, constraint, &environment)?;
        }
        Ok(())
    }

    /// The result of a builtin that builds the nominal `String` itself
    /// (`repr`, `input`), as upstream's functions return one: the stdlib
    /// struct, or the compile-time string on an unlinked seam without it.
    pub(super) fn builtin_string_result(&self) -> Ty {
        if self
            .structs
            .contains_key(mojito_symbol::symbol::STDLIB_STRING_STRUCT)
        {
            Ty::new(TyKind::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                Vec::new(),
            ))
        } else {
            Ty::string_literal()
        }
    }

    /// The nominal-`String` wrap of a compile-time string that materializes
    /// as a runtime value (an inferred binding, a collection element):
    /// record the literal-constructor wrap at `span` and report the nominal
    /// `String`, falling back to the compile-time string when the stdlib
    /// constructor is unavailable (an unlinked seam).
    pub(super) fn nominal_string_wrap(&self, span: SourceSpan) -> Result<Ty, TypeError> {
        let nominal = Ty::new(TyKind::Struct(
            mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
            Vec::new(),
        ));
        let Some((target, _)) = self.implicit_conversion_target(&Ty::string_literal(), &nominal)?
        else {
            return Ok(Ty::string_literal());
        };
        // A view conversion already recorded at this span supersedes the
        // wrap; re-inferring the expression must not revert it.
        self.record_at(span)
            .implicit_conversion
            .get_or_insert(target);
        Ok(nominal)
    }

    /// `T(copy=x)` on a type parameter: construction through the bound's
    /// `Copyable` initializer, `__init__(out self, *, copy: Self)`, which
    /// borrows a value of the parameter's own type. Recorded as the
    /// `ConstructTypeParam` of the binder; its argument stays the call's own.
    pub(super) fn infer_type_param_copy_construction(
        &self,
        span: SourceSpan,
        name: &str,
        ty: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let TyKind::Param { binder, bounds, .. } = ty.kind() else {
            return Err(TypeError::InvariantViolation(format!(
                "constructing '{name}' names no type parameter"
            )));
        };
        // A nullary requirement (`Defaultable`'s) is `T()`'s, never this
        // call's.
        if let Some(bound) = bounds.iter().find(|bound| {
            self.traits
                .get(bound.as_str())
                .and_then(|info| info.methods.get("__init__"))
                .is_some_and(|inits| inits.iter().any(|init| !init.params.is_empty()))
        }) {
            return Err(TypeError::Unsupported(format!(
                "constructing type parameter '{name}' through the `__init__` requirement \
                 of trait '{bound}'"
            )));
        }
        let ([], [], [source]) = (param_args, args, kwargs) else {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "a type parameter is constructed only through its bound's \
                         initializers, `()` or `(copy=value)`"
                    .to_string(),
            });
        };
        if source.name != "copy" {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: format!(
                    "no initializer of the parameter's bound takes keyword '{}'",
                    source.name
                ),
            });
        }
        if !self.is_copyable(ty) {
            return Err(TypeError::TraitNotSatisfied {
                param: name.to_string(),
                ty: ty.to_string(),
                trait_name: "Copyable".to_string(),
                reason: self.trait_failure_reason(ty, "Copyable"),
            });
        }
        let found = self.infer(&source.value)?;
        if !mojito_types::types::coerces(&found, ty) {
            return Err(TypeError::TypeMismatch {
                expected: ty.to_string(),
                found: found.to_string(),
                context: format!("argument 'copy' of '{name}'"),
            });
        }
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                param: binder.clone(),
            },
        ));
        Ok(ty.clone())
    }

    /// `Name(copy=x)` on a built-in value type: its `Copyable` initializer,
    /// which borrows a value of the type and copies it by the value read.
    fn infer_builtin_copy_construction(
        &self,
        span: SourceSpan,
        name: &str,
        ty: &Ty,
        source: &mojito_ast::ast::KwArg,
    ) -> Result<Ty, TypeError> {
        let found = self.infer_with_expected(&source.value, ty, true)?;
        if !self.record_implicit_conversion(&source.value, &found, ty)? {
            return Err(TypeError::TypeMismatch {
                expected: ty.to_string(),
                found: found.to_string(),
                context: format!("argument 'copy' of '{name}'"),
            });
        }
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::BuiltinCopyConstruction,
        ));
        Ok(ty.clone())
    }

    /// `SIMD(mlir_value=v)`: the storage construction whose type is the
    /// vector of `v`'s `!kgen.simd` type.
    fn infer_inferred_storage_construction(
        &self,
        span: SourceSpan,
        name: &str,
        storage: &mojito_ast::ast::KwArg,
    ) -> Result<Ty, TypeError> {
        let found = self.infer(&storage.value)?;
        let ty =
            mojito_types::types::storage_simd(&found).ok_or_else(|| TypeError::TypeMismatch {
                expected: "`!kgen.simd` storage".to_string(),
                found: found.to_string(),
                context: format!("argument 'mlir_value' of '{name}'"),
            })?;
        self.infer_storage_construction(span, name, &ty, storage)
    }

    /// `Name(mlir_value=v)` on a SIMD type, or `Bool(value=i)` at `ty` the
    /// `i1` or `Bool` it takes: upstream's initializer from the
    /// `!kgen.simd` storage of the type, or from the `i1` both backends hold
    /// as a `Bool`, so the argument must already be that storage, never a
    /// literal, a value of the type, or another width or dtype.
    fn infer_storage_construction(
        &self,
        span: SourceSpan,
        name: &str,
        ty: &Ty,
        storage: &mojito_ast::ast::KwArg,
    ) -> Result<Ty, TypeError> {
        let found = self.infer(&storage.value)?;
        let expected = match storage.name.as_str() {
            "mlir_value" => mojito_types::types::simd_storage(ty).unwrap_or_else(|| ty.clone()),
            _ => ty.clone(),
        };
        if found != expected {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: if mojito_types::types::default_literal(&found) == found {
                    found.to_string()
                } else {
                    "a literal".to_string()
                },
                context: format!("argument '{}' of '{name}'", storage.name),
            });
        }
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::BuiltinCopyConstruction,
        ));
        Ok(ty.clone())
    }

    /// The built-in value type a constructor head names (`Int`,
    /// `SIMD[DType.float32, 4]`, `Scalar[DType.bool]`, `Float32`), whose
    /// copy is the value read.
    fn builtin_copy_constructed(
        &self,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Option<(Ty, BracketAttributes)> {
        match name {
            "SIMD" => self.simd_type(param_args).ok(),
            "Scalar" if param_args.len() == 1 => {
                let dtype = self.dtype_from_arg(&param_args[0]).ok()?;
                let attribute = dtype_attribute(&param_args[0], &dtype);
                Some((simd_of(dtype, SimdWidth::Known(1)).ok()?, vec![attribute]))
            }
            "Int" | "UInt" | "Bool" | "Float64" if param_args.is_empty() => {
                scalar_type_name(name).map(|ty| (ty, Vec::new()))
            }
            // `Self(mlir_value=v)` inside `SIMD`'s own members.
            "Self" if param_args.is_empty() && self.lookup("Self").is_none() => self
                .self_ty
                .clone()
                .filter(|ty| simd_slots(ty).is_some())
                .map(|ty| (ty, Vec::new())),
            _ if param_args.is_empty() => {
                Dtype::from_scalar_alias(name).map(|dtype| (simd_ty(dtype, 1), Vec::new()))
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn infer_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let spelled = self.spelled_instances.borrow().get(&span).cloned();
        if let Some(instance) = spelled {
            return self.infer_instance_construction(&span, &instance, args, kwargs);
        }
        if name == "__RuntimeTuple"
            && (self.bundled_stdlib_declaration
                || super::overload_support::is_bundled_module_source(span.source.as_deref()))
            && param_args.is_empty()
            && kwargs.is_empty()
        {
            return args
                .iter()
                .map(|argument| self.infer(argument))
                .collect::<Result<Vec<_>, _>>()
                .map(|elements| TyKind::Tuple(elements).into());
        }
        if name == "__mojito_fieldwise_copy" {
            if !param_args.is_empty() || args.len() != 1 || !kwargs.is_empty() {
                return Err(TypeError::InvariantViolation(
                    "malformed synthesized fieldwise copy".to_string(),
                ));
            }
            let ty = self.infer(&args[0])?;
            if !self.is_copyable(&ty) {
                return Err(TypeError::TraitNotSatisfied {
                    param: "value".to_string(),
                    ty: ty.to_string(),
                    trait_name: "Copyable".to_string(),
                    reason: self.trait_failure_reason(&ty, "Copyable"),
                });
            }
            self.record_at(args[0].source_span()).copy_place_value_use = true;
            return Ok(ty);
        }
        if let Some(element) = self.infer_pack_element_construction(
            span.clone(),
            name,
            false,
            param_args,
            args,
            kwargs,
        ) {
            return element;
        }
        if param_args.is_empty() {
            let type_parameter = self
                .tparams
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).cloned())
                .or_else(|| {
                    self.self_decls
                        .iter()
                        .filter(|declaration| declaration.name() == name)
                        .find_map(type_parameter)
                });
            if let Some(ty) = type_parameter.as_ref()
                && let TyKind::Param { binder, bounds, .. } = ty.kind()
                && (!args.is_empty()
                    || !kwargs.is_empty()
                    || bounds
                        .iter()
                        .any(|bound| self.bound_default_constructs(bound)))
            {
                if !(args.is_empty() && kwargs.is_empty()) {
                    return self.infer_type_param_copy_construction(
                        span, name, ty, param_args, args, kwargs,
                    );
                }
                self.record_at(span).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                        param: binder.clone(),
                    },
                ));
                return Ok(ty.clone());
            }
        }
        self.reject_nested_self_reference(name)?;
        let Some(ty) = self.lookup(name).cloned() else {
            return self.infer_unbound_call(span, name, param_args, args, kwargs);
        };
        self.record_permitted_call_capture(name);
        if let Some(owner) = self.lookup_owner(name) {
            self.record_at(span.clone()).binding = Some(owner);
        }
        let origin_signatures = self.lookup_callable_origins(name).unwrap_or_default();
        if let TyKind::Overload(candidates) = ty.kind() {
            let OverloadProbes {
                matches,
                availability_failures,
                bound_failures,
                argument_failure,
            } = self.probe_overload_candidates(
                &OverloadCall {
                    span: &span,
                    name,
                    param_args,
                    args,
                    kwargs,
                },
                candidates,
                &origin_signatures,
            );
            return match select_callable_overload(matches) {
                Ok((ret, target, selected_index, error)) => {
                    reject_unavailable(origin_signatures.get(selected_index))?;
                    self.record_at(span.clone()).overload_target = Some(target);
                    if let Some((index, selected)) = candidates
                        .get(selected_index)
                        .map(|selected| (selected_index, selected))
                    {
                        let (prepared, ordinary_param_args, forwarded) = self
                            .prepare_callable_specialization(
                                name,
                                param_args,
                                selected.clone(),
                                origin_signatures.get(index),
                            )?;
                        let (.., attributes) = self.infer_callable_ty(
                            &span,
                            name,
                            prepared.clone(),
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                        self.record_bracket_attributes(span.clone(), forwarded.scatter(attributes));
                        self.record_call_environment_effects(
                            span.clone(),
                            &prepared,
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                        // Overloads share the bare-name effect entry (a
                        // conservative union); replay it and any call-through
                        // residues exactly like the single-callable path.
                        let bindings = self.call_bindings.borrow_mut().remove(&span);
                        self.apply_transfer_effects(
                            name,
                            None,
                            args,
                            &span,
                            &bindings.unwrap_or_default(),
                        )?;
                        self.record_call_through(name, &prepared, args);
                        self.record_call_parameter_names(&span, &prepared);
                        let callee_decls = match prepared.kind() {
                            TyKind::GenericFunc(generic) => generic.decls.as_slice(),
                            _ => &[],
                        };
                        self.apply_call_through_effects(
                            name,
                            callee_decls,
                            None,
                            param_args,
                            args,
                            &span,
                        )?;
                        self.record_free_call_result_origins(
                            &span,
                            &prepared,
                            origin_signatures.get(index),
                            args,
                            kwargs,
                        );
                    }
                    if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
                        self.record_call_effect(span.clone(), error.clone());
                        self.require_error(format!("call to raising function '{name}'"), error)?;
                    }
                    self.record_view_result_borrow(&span, &ret);
                    Ok(self.bind_call_result_tail(&span, ret))
                }
                Err(OverloadSelect::NoMatch) => {
                    // The stdlib scalar `range` family: upstream's overloads
                    // are dtype-inferred with no explicit spelling, so a
                    // scalar argument can never match the Int defs. The
                    // stdlib-set guard (a candidate returning a range-family
                    // struct) keeps a user's shadowing `range` overloads on
                    // the ordinary diagnostic path.
                    if name == "range"
                        && param_args.is_empty()
                        && kwargs.is_empty()
                        && candidates.iter().filter_map(Ty::as_func).any(|func| {
                            matches!(
                                func.ret.kind(),
                                TyKind::Struct(target, _)
                                    if mojito_types::types::SCALAR_RANGE_FAMILY
                                        .iter()
                                        .any(|family| target.contains(family))
                            )
                        })
                        && let Some(ty) = self.infer_scalar_range(&span, args)?
                    {
                        return Ok(ty);
                    }
                    if let Some(failure) = argument_failure {
                        return Err(failure);
                    }
                    if availability_failures.is_empty()
                        && let [failure] = bound_failures.as_slice()
                    {
                        return Err(failure.clone());
                    }
                    Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: match availability_failures.as_slice() {
                            [Some(reason)] => reason.clone(),
                            _ => "no overload matches the supplied arguments".to_string(),
                        },
                    })
                }
                Err(OverloadSelect::Ambiguous) => Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "ambiguous overloaded call".to_string(),
                }),
            };
        }
        // `objs[0](3)` / `grid[i, j](x)`: value brackets over an indexable
        // runtime binding are a subscript, not compile-time parameter
        // application — current Mojo dispatches the element call. A struct
        // carrying its own callable contract keeps parameter application
        // (specialized callable values), and non-value bracket forms keep the
        // parenthesization hint in `infer_callable_ty`.
        if let TyKind::Struct(struct_name, _) = ty.kind()
            && !param_args.is_empty()
            && param_args
                .iter()
                .all(|argument| matches!(argument, mojito_ast::ast::ParamArg::Value(_)))
            && self.declared_callable_contract(&ty).is_none()
            && self
                .structs
                .get(struct_name)
                .is_some_and(|info| info.methods.contains_key("__getitem__"))
        {
            let indices: Vec<Expr> = param_args
                .iter()
                .filter_map(|argument| match argument {
                    mojito_ast::ast::ParamArg::Value(value) => Some(value.clone()),
                    _ => None,
                })
                .collect();
            let receiver = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            };
            return self.infer_element_call(span, &receiver, &ty, &indices, args, kwargs);
        }
        reject_unavailable(origin_signatures.first())?;
        let (ty, ordinary_param_args, forwarded) =
            self.prepare_callable_specialization(name, param_args, ty, origin_signatures.first())?;
        let indirect_target = match ty.kind() {
            TyKind::Struct(..) => self.indirect_callable_target(&ty),
            _ if callable_contract_ty(&ty).is_some()
                && self.binding_scope(name).is_some_and(|scope| scope > 0) =>
            {
                self.indirect_callable_target(&ty)
            }
            _ => None,
        };
        if indirect_target.is_some() && matches!(ty.kind(), TyKind::GenericFunc(_)) {
            let (contract, arguments) =
                self.instantiate_generic_callable_value(name, ty.clone(), &ordinary_param_args)?;
            self.record_at(span.clone()).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                },
            ));
        }
        let (ret, _, error, bool_bindings, _, attributes) =
            self.infer_callable_ty(&span, name, ty.clone(), &ordinary_param_args, args, kwargs)?;
        self.record_bracket_attributes(span.clone(), forwarded.scatter(attributes));
        if let Some(signature) = origin_signatures.first() {
            self.validate_erased_origin_constraint(name, signature, &bool_bindings)?;
        }
        // Replay the callee's loan-transfer effects against the actuals.
        let bindings = self.call_bindings.borrow_mut().remove(&span);
        self.apply_transfer_effects(name, None, args, &span, &bindings.unwrap_or_default())?;
        // A call through one of the current body's own callable parameters
        // records a higher-order residue for this body's callers to resolve;
        // a call to a callee WITH such residues resolves them against the
        // concrete callables this call supplies.
        self.record_call_through(name, &ty, args);
        let callee_decls = match ty.kind() {
            TyKind::GenericFunc(generic) => generic.decls.as_slice(),
            _ => &[],
        };
        self.apply_call_through_effects(name, callee_decls, None, param_args, args, &span)?;
        // A function-typed VALUE carries its origin def's effects on the
        // type; replay those too (the name-keyed entry above covers direct
        // and nested calls by declaration name).
        let carried = contract_transfer_effects(&ty);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span, &TySubst::new())?;
        }
        // A callable struct value dispatches `Struct.__call__`; replay its
        // effects with the callee binding as the receiver actual.
        if let TyKind::Struct(struct_name, _) = ty.kind() {
            let callee_value = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            };
            let method_key = format!("{struct_name}.__call__");
            let effect_key = indirect_target.as_deref().unwrap_or(&method_key);
            self.apply_transfer_effects(
                effect_key,
                Some(&callee_value),
                args,
                &span,
                &TySubst::new(),
            )?;
        }
        self.record_call_environment_effects(
            span.clone(),
            &ty,
            &ordinary_param_args,
            args,
            kwargs,
        )?;
        if let Some(target) = indirect_target {
            self.record_at(span.clone()).overload_target = Some(target);
        }
        self.record_view_result_borrow(&span, &ret);
        if let TyKind::GenericFunc(generic) = ty.kind() {
            self.record_at(span.clone()).declared_result = Some(generic.ret.clone());
        }
        self.record_free_call_result_origins(&span, &ty, origin_signatures.first(), args, kwargs);
        self.record_call_parameter_names(&span, &ty);
        let ret = self.bind_call_result_tail(&span, ret);
        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
            self.record_call_effect(span, error.clone());
            self.require_error(format!("call to raising function '{name}'"), error)?;
        }
        Ok(ret)
    }

    /// Record a direct callable's runtime parameters for the call at `span`;
    /// a callable struct value records none.
    pub(super) fn record_call_parameter_names(&self, span: &SourceSpan, callee: &Ty) {
        if let Some(FuncTy {
            names,
            params,
            conventions,
            ..
        }) = callee.callable()
        {
            self.call_parameters.borrow_mut().insert(
                span.clone(),
                names
                    .iter()
                    .zip(params)
                    .zip(conventions)
                    .map(|((name, ty), convention)| super::CallParameter {
                        name: name.clone(),
                        convention: *convention,
                        ty: ty.clone(),
                    })
                    .collect(),
            );
        }
    }

    /// A non-method call whose result is a ref-field struct (a borrowing
    /// view/iterator) lends its borrowed sources to the result, exactly as the
    /// method-receiver rule in `infer_method_call` does: record the
    /// view-result adjustment so lowering anchors the result and establishes
    /// caller-side loans. Reference results carry their own loan channel, and
    /// an already-recorded adjustment (captures, instantiated contracts) wins.
    fn record_view_result_borrow(&self, span: &SourceSpan, ret: &Ty) {
        // Reference fields and origin-bearing pointer fields (upstream's
        // `Pointer[T, Self.o]` iterator storage) both borrow.
        if matches!(ret.kind(), TyKind::Struct(..)) && self.type_carries_loans(ret) {
            self.record_at(span.clone())
                .adjustment
                .get_or_insert_with(|| {
                    Box::new(
                        mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                            materialized: None,
                            interior: Vec::new(),
                        },
                    )
                });
        }
    }

    /// Resolve a free call's view-return contract (the selected overload's
    /// `CallableOriginSignature::view_return`) against its arguments and
    /// record the result's origin slots; see `record_call_result_origins`.
    /// The slots are matched exactly as the call typing matched them.
    fn record_free_call_result_origins(
        &self,
        span: &SourceSpan,
        callee: &Ty,
        signature: Option<&super::CallableOriginSignature>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) {
        let Some(signature) = signature.filter(|signature| !signature.view_return.is_empty())
        else {
            return;
        };
        let Some(FuncTy {
            names,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            ..
        }) = callee.callable()
        else {
            return;
        };
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let Ok(matched) = match_call_slots(
            names,
            required,
            *positional_only,
            *keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        ) else {
            return;
        };
        self.record_call_result_origins(
            span,
            &signature.view_return,
            &matched.slots,
            args,
            kwargs,
            None,
        );
    }

    /// Type the bare element-call spelling — `objs[0](3)`, `a.b[i](x)`,
    /// `grid[i, j](x)` — as subscript-then-indirect-call, matching current
    /// Mojo. The brackets are a runtime subscript of `receiver`, an indexable
    /// value with no callable contract of its own. The selected `__getitem__`
    /// moves out of the call's `selected_call` fact into the
    /// `ElementInvocation` adjustment so node-level consumers
    /// (reference-result diversion, resolved-callable projection, raise
    /// attribution) see the element call, not the subscript read.
    pub(super) fn infer_element_call(
        &self,
        span: SourceSpan,
        receiver: &Expr,
        receiver_ty: &Ty,
        indices: &[Expr],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let label = match &receiver.kind {
            ExprKind::Identifier(name) => name.clone(),
            ExprKind::Member { field, .. } => field.clone(),
            _ => "<element>".to_string(),
        };
        for (position, index) in indices.iter().enumerate() {
            self.prepare_index_argument(receiver_ty, index, "__getitem__", position)?;
        }
        let result =
            self.infer_struct_getitem_call(span.clone(), receiver, indices, receiver_ty)?;
        let element_ty = match result.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => result,
        };
        let getter = self
            .clear_fact(&span, |facts| facts.selected_call.take().map(|fact| *fact))
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "element-call subscript selection recorded no contract".to_string(),
                )
            })?;
        let Some(callable) = self.declared_callable_contract(&element_ty) else {
            return Err(TypeError::NotCallable {
                name: format!("{label}[…]"),
                ty: element_ty.to_string(),
            });
        };
        // The getter's dispatch entry must not survive as this node's resolved
        // callable: `CallIndirect` consults the element's `__call__` target.
        let target = self.indirect_callable_target(&element_ty);
        match &target {
            Some(target) => {
                self.record_at(span.clone()).overload_target = Some(target.clone());
            }
            None => {
                self.clear_fact(&span, |facts| facts.overload_target.take());
            }
        }
        let (ret, _, error, ..) =
            self.infer_callable_ty(&span, "<element>", callable.clone(), &[], args, kwargs)?;
        let carried = contract_transfer_effects(&callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span, &TySubst::new())?;
        }
        if let TyKind::Struct(struct_name, _) = element_ty.kind() {
            let method_key = format!("{struct_name}.__call__");
            let effect_key = target.as_deref().unwrap_or(&method_key);
            self.apply_transfer_effects(effect_key, Some(receiver), args, &span, &TySubst::new())?;
        }
        self.record_call_environment_effects(span.clone(), &callable, &[], args, kwargs)?;
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ElementInvocation(Box::new(
                mojito_checked::checked::CheckedElementInvocation {
                    getter,
                    callable,
                    target,
                    raises: error.clone(),
                },
            )),
        ));
        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
            self.record_call_effect(span, error.clone());
            self.require_error("call through a raising callable element", error)?;
        }
        Ok(ret)
    }

    pub(super) fn infer_callable_ty(
        &self,
        span: &SourceSpan,
        name: &str,
        ty: Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        // Bind ordinary arguments first, then infer or apply the generic
        // function's compile-time parameters from the occupied slots.
        if matches!(ty.kind(), TyKind::GenericFunc(_)) {
            return self.infer_generic_call(span, name, &ty, param_args, args, kwargs);
        }
        let (
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            _raises,
            error,
            conventions,
            ref_params,
            ref_return,
        ) = match ty.into_kind() {
            TyKind::Param {
                callable_bound: Some(bound),
                ..
            } => {
                return self.infer_callable_ty(span, name, bound, param_args, args, kwargs);
            }
            TyKind::Struct(struct_name, arguments) => {
                let actual = Ty::new(TyKind::Struct(struct_name.clone(), arguments));
                let callable = self.declared_callable_contract(&actual).ok_or_else(|| {
                    // Value brackets over an indexable runtime value were
                    // re-dispatched as an element call before reaching here,
                    // so this shape carries type, named, or empty bracket
                    // arguments — neither a runtime subscript nor parameter
                    // application on a callable.
                    if !param_args.is_empty()
                        && self
                            .structs
                            .get(&struct_name)
                            .is_some_and(|info| info.methods.contains_key("__getitem__"))
                    {
                        return TypeError::Unsupported(format!(
                            "'{name}' is a runtime value of type '{struct_name}': \
                             '{name}[…](…)' dispatches a subscripted element call only \
                             for runtime index arguments, and a value takes no \
                             compile-time parameters"
                        ));
                    }
                    TypeError::NotCallable {
                        name: name.to_string(),
                        ty: struct_name.clone(),
                    }
                })?;
                return self.infer_callable_ty(span, name, callable, param_args, args, kwargs);
            }
            // A non-generic function takes no compile-time parameters.
            TyKind::Func(func) => {
                let FuncTy {
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                    ..
                } = *func;

                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: name.to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                (
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                )
            }
            other => {
                return Err(TypeError::NotCallable {
                    name: name.to_string(),
                    ty: other.to_string(),
                });
            }
        };

        // Match positional then keyword arguments to the regular parameter slots
        // (extra positional args overflow into a `*args` parameter), then check
        // each supplied argument coerces to its parameter's type (an unfilled slot
        // uses the default, already type-checked at the definition site).
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let has_kw_collector = kw_variadic.is_some();
        let kw_collector = kw_variadic;
        if forwarded_element.is_some() && kw_collector.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        self.reject_forwarded_pack(name, args, variadic.as_ref())?;
        let matched = match_call_slots(
            &names,
            &required,
            positional_only,
            keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_collector.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        // The callee's own origin binders bind from the arguments before each
        // argument coerces, and the result carries what they bound;
        // `declared` keeps the type parameters abstract.
        let origin_arguments = CalleeOriginArguments {
            slots: &slots,
            overflow: &overflow,
            args,
            kwargs,
        };
        let (params, declared, bound_variadic, bindings) = self.substitute_callee_pointer_origins(
            name,
            &names,
            params,
            variadic.as_ref(),
            &origin_arguments,
        )?;
        let ret = match bindings {
            Some(bindings) => bindings.substitute(&ret),
            None => ret,
        };
        let mut score = 0;
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            let arg_ty = self.infer_with_expected(arg, &params[i], true)?;
            reject_generic_function_value(name, &names[i], arg, &arg_ty, &params[i])?;
            if !self.record_implicit_conversion(arg, &arg_ty, &params[i])? {
                if super::builtins::callable_mismatch_is_environment_only(&arg_ty, &params[i]) {
                    return Err(TypeError::Unsupported(format!(
                        "a capturing closure cannot bind to the unqualified 'def(...)' \
                         parameter '{}' of '{}' in current Mojo; the contract must spell \
                         'capturing[...]'",
                        names[i], name
                    )));
                }
                return Err(TypeError::TypeMismatch {
                    expected: params[i].to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument '{}' to '{}'", names[i], name),
                });
            }
            score += conversion_count(&arg_ty, &params[i]);
            // Only a `var`/`deinit` parameter *consumes* its argument (moving the
            // value in). `read` (the default), `mut`, and `ref` all **borrow** — no
            // copy — so passing a non-Copyable value to them is fine.
            if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                conventions.get(i)
            {
                let kind = if *convention == ArgConvention::Deinit {
                    super::traits::ConsumeKind::Deinit
                } else {
                    super::traits::ConsumeKind::Move
                };
                self.check_consuming_as(
                    arg,
                    &arg_ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    kind,
                )?;
            }
        }
        let collected = match &variadic {
            Some(element) => {
                let (collected_score, collected) = self.check_collected_arguments(
                    name,
                    element,
                    bound_variadic.as_ref(),
                    &overflow,
                    args,
                )?;
                score += collected_score;
                collected
            }
            None => Vec::new(),
        };
        self.check_owned_collector(name, &overflow, args)?;
        if let Some(elem) = kw_collector {
            for index in kw_overflow {
                let expression = &kwargs[index].value;
                let found = self.infer_with_expected(expression, &elem, true)?;
                if !self.record_implicit_conversion(expression, &found, &elem)? {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: found.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &found,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                score += conversion_count(&found, &elem);
            }
            if let Some(found) = forwarded_element
                && found != elem
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{elem}]"),
                    found: format!("StringDict[{found}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }

        // Borrow check (mutable-XOR-shared), root-sensitive: within one call a
        // variable borrowed exclusively (`mut`/`ref`) or moved (`^`) may not be
        // borrowed again — mutably, shared, or moved.
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                &conventions,
                &ref_params,
                ref_return.as_deref(),
                args,
                kwargs,
                true,
            )
            .map_err(|error| name_ref_origin_mismatch(error, name, &names))?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                let ty = self.infer_with_expected(expression, &params[index], true)?;
                Ok(self.argument_is_independent_copy(convention, expression, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        let collected = self.collected_arguments(name, collected);
        self.check_free_call_aliasing(
            &self.free_callee(name, &names, &declared, &params, Some(&collected)),
            &effective_conventions,
            &copied_reads,
            &slots,
            args,
            kwargs,
        )?;
        self.record_argument_borrows(
            &slots,
            &effective_conventions,
            self.gathered_arguments(name, &overflow),
            args,
            kwargs,
            None,
        );

        let result = match return_ref {
            Some(mut reference) => {
                reference.referent = ret;
                TyKind::Ref(reference).into()
            }
            None => ret,
        };
        Ok((
            result,
            concrete_overload_rank(score, variadic.is_some() || has_kw_collector, &declared)
                + self
                    .argument_binding(
                        variadic.as_ref().map(|_| overflow.len()),
                        &slots,
                        &conventions,
                        &params,
                        args,
                        kwargs,
                    )
                    .rank(),
            error,
            bool_bindings,
            0,
            Vec::new(),
        ))
    }

    /// Type a call to a generic function: solve its type parameters from the
    /// argument types, then check each argument coerces to the substituted
    /// parameter type and return the substituted result type.
    pub(super) fn infer_generic_call(
        &self,
        span: &SourceSpan,
        name: &str,
        generic: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        let Some(GenericFuncTy {
            decls,
            func:
                FuncTy {
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises: _,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                    ..
                },
        }) = generic.as_generic_func()
        else {
            return Err(TypeError::InvariantViolation(format!(
                "generic call inference received non-generic callee '{name}'"
            )));
        };
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let forwarded_pack = self.forwarded_pack_argument(name, args, variadic.is_some())?;
        let matched = match_call_slots(
            names,
            required,
            *positional_only,
            *keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error(name))?;
        if let Some((position, _)) = &forwarded_pack {
            mojito_ast::call::bind_spread(&matched, *position)
                .map_err(|error| error.into_type_error(name))?;
        }
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let explicit = self.explicit_type_subst(name, decls, param_args)?;
        let mut use_params = Vec::new();
        let mut arg_tys = Vec::new();
        let mut arg_exprs = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            use_params.push(params[i].clone());
            arg_tys.push(self.solving_argument_ty(arg, &params[i], decls, &explicit)?);
            arg_exprs.push(arg);
        }
        if let Some(elem) = variadic.as_ref() {
            for &p in &overflow {
                use_params.push(elem.clone());
                let actual = match &forwarded_pack {
                    Some((position, forwarded)) if *position == p => self.bind_forwarded_pack(
                        name,
                        &args[p],
                        forwarded,
                        elem,
                        self.owned_collectors.get(name).copied(),
                    )?,
                    _ => self.solving_argument_ty(&args[p], elem, decls, &explicit)?,
                };
                arg_tys.push(actual);
                arg_exprs.push(&args[p]);
            }
        }
        let mut keyword_actuals = Vec::new();
        if let Some(element) = kw_variadic.as_ref() {
            for &index in &kw_overflow {
                let actual = self.infer(&kwargs[index].value)?;
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
                keyword_actuals.push((index, actual));
            }
            if let Some(actual) = &forwarded_element {
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
            }
        }
        let BoundParameters {
            subst,
            arguments: tyargs,
            attributes,
        } = self.resolve_use_params(name, decls, param_args, &use_params, &arg_tys)?;
        let values = Self::value_argument_environment(decls, &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = super::generics::expand_solved_packs(ty, decls, &tyargs);
            let substituted = self.resolve_assoc_ty(&substitute(&expanded, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        self.record_pack_element_conversions(decls, &tyargs, &use_params, &arg_tys, &arg_exprs)?;
        let (mut conversions, mut simd_erasures) = (0, 0);
        for ((aty, pty), expression) in arg_tys.iter().zip(&use_params).zip(arg_exprs) {
            if matches!(pty.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')) {
                // Each pack element was checked independently against the pack's
                // bounds during inference; there is intentionally no single
                // substituted element type to coerce every argument into.
                conversions += pack_element_conversion_count(aty);
                continue;
            }
            simd_erasures += usize::from(
                matches!(pty.kind(), TyKind::Param { .. })
                    && mojito_types::types::simd_shape(aty).is_some(),
            );
            let expected = resolve(pty)?;
            // Re-inferred against the solved parameter: a display argument
            // constructs that parameter's collection, and a parameter that
            // resolves to a reference-bearing type only once explicit value
            // arguments are substituted (`Ts[index]` over a struct with a
            // `ref` field) takes the stored handle, not its referent.
            let actual = self.infer_with_expected(expression, &expected, true)?;
            if !self.record_implicit_conversion(expression, &actual, &expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("argument to '{name}'"),
                });
            }
            conversions += conversion_count(&actual, &expected);
        }
        if let Some(element) = kw_variadic.as_ref() {
            let expected = resolve(element)?;
            for (index, actual) in keyword_actuals {
                let expression = &kwargs[index].value;
                if !self.record_implicit_conversion(expression, &actual, &expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                conversions += conversion_count(&actual, &expected);
            }
            if let Some(actual) = forwarded_element
                && actual != expected
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{expected}]"),
                    found: format!("StringDict[{actual}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }
        for (i, slot) in slots.iter().enumerate() {
            if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                conventions.get(i)
            {
                let arg = match slot {
                    ArgSlot::Positional(p) => &args[*p],
                    ArgSlot::Keyword(k) => &kwargs[*k].value,
                    ArgSlot::Default => continue,
                };
                let kind = if *convention == ArgConvention::Deinit {
                    super::traits::ConsumeKind::Deinit
                } else {
                    super::traits::ConsumeKind::Move
                };
                let expected = resolve(&params[i])?;
                let ty = self.infer_with_expected(arg, &expected, true)?;
                self.check_consuming_as(
                    arg,
                    &ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    kind,
                )?;
            }
        }
        self.check_owned_collector(name, &overflow, args)?;
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                conventions,
                ref_params,
                ref_return.as_deref(),
                args,
                kwargs,
                true,
            )
            .map_err(|error| name_ref_origin_mismatch(error, name, names))?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                // Contextual, as the argument was just checked: a bare
                // inference would re-record a display as an `Array`.
                let ty = match resolve(&params[index]) {
                    Ok(expected) => self.infer_with_expected(expression, &expected, true)?,
                    Err(_) => self.infer(expression)?,
                };
                Ok(self.argument_is_independent_copy(convention, expression, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        let bound: Vec<Ty> = params
            .iter()
            .map(|parameter| resolve(parameter).unwrap_or_else(|_| parameter.clone()))
            .collect();
        let collected =
            self.generic_collected_arguments(name, variadic.as_ref(), &overflow, args, resolve);
        self.check_free_call_aliasing(
            &self.free_callee(name, names, params, &bound, Some(&collected)),
            &effective_conventions,
            &copied_reads,
            &slots,
            args,
            kwargs,
        )?;
        self.record_argument_borrows(
            &slots,
            &effective_conventions,
            self.gathered_arguments(name, &overflow),
            args,
            kwargs,
            None,
        );
        let mut referent = resolve(ret)?;
        if forwarded_pack.is_some() {
            // A result naming the callee's pack elements (`Us[i]`) closes
            // over the caller's own pack.
            referent = self.close_pack_elements(referent, &[(decls, &tyargs)]);
        }
        let result = return_ref
            .map(|mut reference| {
                reference.referent = referent.clone();
                Ty::new(TyKind::Ref(reference))
            })
            .unwrap_or(referent);
        let error = error.as_ref().map(resolve).transpose()?;
        self.call_bindings
            .borrow_mut()
            .insert(span.clone(), subst.clone());
        // An inferred application whose clone already exists (an earlier
        // discovery round minted it for another occurrence) retargets to it
        // by exact name instead of requesting a round of its own; otherwise
        // retain the resolved application for instantiation discovery.
        // An overload candidate's write here is undone with its probe; the
        // selected candidate's re-run is the only one kept.
        if param_args.is_empty()
            && let Some(clone) = self.existing_def_clone(name, decls, &tyargs)
        {
            self.clear_fact(span, |facts| facts.generic_instantiation.take());
            self.record_at(span.clone()).overload_target = Some(clone);
        } else {
            self.record_at(span.clone()).generic_instantiation =
                Some(mojito_checked::checked::GenericInstantiation {
                    callee: name.to_string(),
                    parameter_names: names.clone(),
                    parameter_types: params
                        .iter()
                        .map(|ty| TypeKey::from_ty(ty).as_str().to_string())
                        .collect(),
                    variadic: mojito_symbol::symbol::VariadicKey::from_callable(generic),
                    folded_arguments: folded_parameter_arguments(decls, param_args, &tyargs),
                    arguments: tyargs,
                });
        }
        Ok((
            result,
            overload_rank(
                conversions,
                variadic.is_some() || kw_variadic.is_some(),
                decls.len(),
                true,
            ) + self
                .argument_binding(
                    variadic.as_ref().map(|_| overflow.len()),
                    &slots,
                    conventions,
                    params,
                    args,
                    kwargs,
                )
                .rank(),
            error,
            bool_bindings,
            simd_erasures,
            attributes,
        ))
    }

    /// The already-minted clone of bound-generic `name` for a closed inferred
    /// application, when the program declares it: the specializer names a
    /// clone `mangle(name, values)` over the declaration-order type and value
    /// arguments (`def_request_values`), so an occurrence whose clone exists
    /// needs no discovery round. Anything that does not resolve to a declared
    /// concrete function keeps the request path.
    pub(super) fn existing_def_clone(
        &self,
        name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Option<String> {
        if decls.is_empty() || decls.len() != arguments.len() {
            return None;
        }
        let mut values = Vec::new();
        for (decl, argument) in decls.iter().zip(arguments) {
            match (decl, argument) {
                (ParamDecl::Type { variadic: true, .. }, _) => return None,
                // A deferred callable-value slot is not part of the key; a
                // residual value names no clone at all (`mangle` refuses it).
                (_, TyArg::Val(CtValue::Deferred(_) | CtValue::Marker(_))) => continue,
                (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                    values.push(CtValue::Type(ty.clone()));
                }
                (ParamDecl::Value { .. }, TyArg::Val(value)) => values.push(value.clone()),
                _ => return None,
            }
        }
        if values.is_empty() {
            return None;
        }
        let clone = mojito_symbol::symbol::mangle(name, &values).ok()?;
        matches!(self.lookup(&clone).map(Ty::kind), Some(TyKind::Func(_))).then_some(clone)
    }

    pub(super) fn forwarded_kwargs_element(
        &self,
        callee: &str,
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Option<Ty>, TypeError> {
        let mut forwarded = kwargs.iter().filter(|argument| argument.is_forwarded());
        let Some(argument) = forwarded.next() else {
            return Ok(None);
        };
        if forwarded.next().is_some() {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "only one keyword dictionary can be forwarded".to_string(),
            });
        }
        if !matches!(&argument.value.kind, ExprKind::Transfer(_)) {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "keyword forwarding requires ownership transfer (`**kwargs^`)".to_string(),
            });
        }
        let found = self.infer(&argument.value)?;
        match found.kind() {
            TyKind::Struct(name, args) if name == "StringDict" => match args.as_slice() {
                [TyArg::Ty(element)] => Ok(Some(element.clone())),
                _ => Err(TypeError::InvariantViolation(
                    "StringDict must carry one value type".to_string(),
                )),
            },
            other => Err(TypeError::TypeMismatch {
                expected: "StringDict[T]".to_string(),
                found: other.to_string(),
                context: format!("forwarded keyword arguments to '{callee}'"),
            }),
        }
    }

    /// How a candidate binds the call's arguments to its regular parameters,
    /// for the terms `ArgumentBinding` owns. `collector` is how many
    /// arguments the candidate's `*args` collector takes, or `None` when it
    /// has none: a candidate without a collector pays only the copies, so its
    /// parameter types are never asked whether they are trivially
    /// register-passable.
    pub(in crate::checker) fn argument_binding(
        &self,
        collector: Option<usize>,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        params: &[Ty],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> ArgumentBinding {
        let mut binding = ArgumentBinding::new(collector);
        for (index, slot) in slots.iter().enumerate() {
            let argument = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            binding.bind(
                conventions.get(index).copied().flatten(),
                collector.is_some()
                    && params
                        .get(index)
                        .is_some_and(|param| self.is_trivial_register_passable(param)),
                argument,
            );
        }
        binding
    }

    /// Record each pack element's conversion to the element the call
    /// solved, when it solved the pack. Each element was checked against the
    /// pack's bounds during inference, and there is no single substituted
    /// element type to coerce every argument into; a literal element
    /// converts to its solved element, which the template's instance takes
    /// it as.
    fn record_pack_element_conversions(
        &self,
        decls: &[ParamDecl],
        tyargs: &[TyArg],
        use_params: &[Ty],
        arg_tys: &[Ty],
        arg_exprs: &[&Expr],
    ) -> Result<(), TypeError> {
        let solved = decls
            .iter()
            .zip(tyargs)
            .find_map(|(decl, argument)| match (decl, argument) {
                (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                    Some(elements)
                }
                _ => None,
            });
        let Some(elements) = solved else {
            return Ok(());
        };
        let pack_arguments = use_params.iter().zip(arg_tys).zip(arg_exprs).filter(
            |((pty, _), _)| matches!(pty.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')),
        );
        for (((_, found), expression), element) in pack_arguments.zip(elements) {
            if let CtValue::Type(element) = element {
                self.record_implicit_conversion(expression, found, element)?;
            }
        }
        Ok(())
    }

    /// Check each argument a `*args` collector gathers at `overflow` against
    /// its element type — `bound`, the element with the callee's origin
    /// binders substituted, where the call bound any — returning the
    /// conversion score and each element's declared and found types for the
    /// exclusivity rule.
    fn check_collected_arguments<'a>(
        &self,
        name: &str,
        element: &Ty,
        bound: Option<&Ty>,
        overflow: &[usize],
        args: &'a [Expr],
    ) -> Result<(usize, CollectedElements<'a>), TypeError> {
        let arity_mismatch = |expected: usize| TypeError::ArityMismatch {
            name: name.to_string(),
            expected,
            got: overflow.len(),
        };
        let mut score = 0;
        let mut collected = Vec::new();
        for (pack_index, &position) in overflow.iter().enumerate() {
            let source = bound.unwrap_or(element);
            let expected = match source.kind() {
                TyKind::RuntimePack(elements) => elements
                    .get(pack_index)
                    .ok_or_else(|| arity_mismatch(elements.len()))?,
                _ => source,
            };
            let argument = &args[position];
            let found = self.infer_with_expected(argument, expected, true)?;
            // A checker-minted pack clone materializes a literal element
            // as its nominal type (`"lit"` → `String`): the element
            // converts exactly as a regular argument does.
            if !self.record_implicit_conversion(argument, &found, expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: found.to_string(),
                    context: format!("variadic argument to '{name}'"),
                });
            }
            score += conversion_count(&found, expected);
            let declared = match element.kind() {
                TyKind::RuntimePack(elements) => &elements[pack_index],
                _ => element,
            };
            collected.push((argument, vec![declared.clone(), found]));
        }
        match element.kind() {
            TyKind::RuntimePack(elements) if elements.len() != overflow.len() => {
                Err(arity_mismatch(elements.len()))
            }
            _ => Ok((score, collected)),
        }
    }

    /// [`Self::check_consuming_collected`] for the free callee `name`.
    fn check_owned_collector(
        &self,
        name: &str,
        overflow: &[usize],
        args: &[Expr],
    ) -> Result<(), TypeError> {
        self.check_consuming_collected(
            self.owned_collectors.get(name) == Some(&true),
            overflow,
            args,
            &format!("'{name}'"),
        )
    }

    /// Bind a callee's own origin binders named by its parameters — a
    /// `Pointer[T, o]`, a `ref[o]` referent, or a struct carrying `o` in its
    /// origin tail (`List[RefBox[o]]`) — from the arguments filling those
    /// slots and those its `*args` collector gathers, and substitute the
    /// bindings into `params` and the collector's element type before each
    /// argument coerces. Returns the bound parameters, the bound declared
    /// spellings (type parameters left abstract, for the exclusivity rule),
    /// the bound element type, and the bindings, which the result carries
    /// too.
    fn substitute_callee_pointer_origins(
        &self,
        name: &str,
        names: &[String],
        params: Vec<Ty>,
        variadic: Option<&Ty>,
        arguments: &CalleeOriginArguments<'_>,
    ) -> Result<CalleeOriginSubstitution, TypeError> {
        let mut origin_bound: Vec<(usize, Ty)> = Vec::new();
        for (index, slot) in arguments.slots.iter().enumerate() {
            if !params
                .get(index)
                .is_some_and(super::origins::names_origin_binder)
            {
                continue;
            }
            let argument = match slot {
                ArgSlot::Positional(position) => &arguments.args[*position],
                ArgSlot::Keyword(position) => &arguments.kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            origin_bound.push((index, self.infer(argument)?));
        }
        // Each collected argument binds against the collector's element
        // (a pack's own element), appended past the regular parameters.
        let mut patterns = params.clone();
        let mut pattern_names = names.to_vec();
        for (pack_index, &position) in arguments.overflow.iter().enumerate() {
            let element = match variadic.map(Ty::kind) {
                Some(TyKind::RuntimePack(elements)) => elements.get(pack_index),
                _ => variadic,
            };
            let Some(element) =
                element.filter(|element| super::origins::names_origin_binder(element))
            else {
                continue;
            };
            origin_bound.push((patterns.len(), self.infer(&arguments.args[position])?));
            patterns.push(element.clone());
            pattern_names.push(
                self.collector_names
                    .get(name)
                    .map_or("args", String::as_str)
                    .to_string(),
            );
        }
        if origin_bound.is_empty() {
            let declared = params.clone();
            return Ok((params, declared, None, None));
        }
        let bound: Vec<(usize, &Ty)> = origin_bound
            .iter()
            .map(|(index, ty)| (*index, ty))
            .collect();
        let bindings = Self::bind_callee_origins(name, &pattern_names, &patterns, &bound)?;
        let bound_variadic = variadic.map(|element| bindings.substitute(element));
        let bound = bindings.substitute_all(&params);
        Ok((bound.clone(), bound, bound_variadic, Some(bindings)))
    }

    /// `__param_list_address[*values]()`: a `Pointer[E, ImmStaticOrigin]` to
    /// the first of the value pack's elements, laid out once per program.
    fn infer_param_list_address(
        &self,
        span: SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (argument, values) = match param_args {
            [argument] if args.is_empty() => self
                .raw_list_argument(argument)
                .map(|values| (argument, values)),
            _ => None,
        }
        .ok_or_else(|| {
            TypeError::Unsupported(format!(
                "'{PARAM_LIST_ADDRESS}' takes one raw parameter list and no arguments"
            ))
        })?;
        self.record_bracket_attributes(
            span.clone(),
            vec![bracket_attribute(
                argument,
                &TyArg::Val(CtValue::Expr(values.clone())),
            )],
        );
        let element = match values.meta() {
            mojito_types::param_expr::MetaTy::ParamList(element) => element.as_value().cloned(),
            _ => None,
        }
        .ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "the value pack `{values}` is not a parameter list of values"
            ))
        })?;
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ParamListAddress { values },
        ));
        Ok(TyKind::Pointer {
            element,
            origin: mojito_types::origin::PointerOrigin::Static,
        }
        .into())
    }

    /// `_mojito_float_text(value, out)`, `std.builtin._format_float`'s
    /// staged leaf: the shortest round-trip text of a `Float64` as bytes at
    /// `out`, returning how many, until R159's Dragonbox body writes it in
    /// Mojo.
    fn infer_float_text(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("_mojito_float_text", 2, args)?;
        if tys[0].kind() != TyKind::FLOAT64 {
            return Err(TypeError::TypeMismatch {
                expected: "Float64".to_string(),
                found: tys[0].to_string(),
                context: "argument to '_mojito_float_text'".to_string(),
            });
        }
        if !matches!(tys[1].kind(), TyKind::Pointer { element, .. }
            if mojito_types::types::simd_slots(element)
                .is_some_and(|(dtype, _)| dtype == mojito_types::types::SimdDtype::Known(mojito_ast::ast::Dtype::UInt8)))
        {
            return Err(TypeError::TypeMismatch {
                expected: "UnsafePointer[Byte]".to_string(),
                found: tys[1].to_string(),
                context: "argument to '_mojito_float_text'".to_string(),
            });
        }
        Ok(Ty::int())
    }
}

/// The positions of the bracket arguments `param_args` supplies to a
/// non-variadic value parameter of `decls` solved to a closed value
/// ([`CtValue::is_parameter_attribute`]). Mojo's call carries such an
/// argument as a parameter attribute alone; the call's instantiated arguments
/// carry it here, so it has no run-time form at the call site
/// ([`mojito_checked::checked::SemanticAdjustment::FoldedParameterArguments`]).
pub(super) fn folded_parameter_arguments(
    decls: &[ParamDecl],
    param_args: &[mojito_ast::ast::ParamArg],
    arguments: &[TyArg],
) -> Vec<usize> {
    param_args
        .iter()
        .enumerate()
        .zip(bracket_argument_slots(decls, param_args))
        .filter_map(|((position, argument), index)| {
            let index = index?;
            let ParamDecl::Value {
                variadic: false, ..
            } = decls.get(index)?
            else {
                return None;
            };
            let Some(TyArg::Val(value)) = arguments.get(index) else {
                return None;
            };
            let is_value = match argument {
                mojito_ast::ast::ParamArg::Value(_) => true,
                mojito_ast::ast::ParamArg::Named { value, .. } => {
                    matches!(&**value, mojito_ast::ast::ParamArg::Value(_))
                }
                mojito_ast::ast::ParamArg::Type(_) => false,
            };
            (is_value && value.is_parameter_attribute()).then_some(position)
        })
        .collect()
}

/// A call's origin-bound parameters, their declared spellings for the
/// exclusivity rule, its bound `*args` element type, and the bindings, when
/// any parameter or collected argument names a binder.
type CalleeOriginSubstitution = (
    Vec<Ty>,
    Vec<Ty>,
    Option<Ty>,
    Option<super::origins::ConstructorOriginBindings>,
);

/// The arguments a `*args` collector gathers, each with its declared and
/// found element types.
type CollectedElements<'a> = Vec<(&'a Expr, Vec<Ty>)>;

/// The arguments of a call whose callee's origin binders bind: the regular
/// slots and the positions its `*args` collector gathers.
struct CalleeOriginArguments<'a> {
    slots: &'a [ArgSlot],
    overflow: &'a [usize],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
}

/// A free call whose callee is an overload set, as each candidate is
/// checked against it.
struct OverloadCall<'a> {
    span: &'a SourceSpan,
    name: &'a str,
    param_args: &'a [mojito_ast::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
}

/// What probing an overload set's candidates found: the call-compatible
/// ones, the failed availability clauses, the failed trait bounds, and an
/// argument that fails every candidate alike.
struct OverloadProbes {
    matches: Vec<CallableOverloadMatch>,
    availability_failures: Vec<Option<String>>,
    bound_failures: Vec<TypeError>,
    argument_failure: Option<TypeError>,
}

/// What checking one overload candidate contributes to the selection.
enum CandidateOutcome {
    Match(CallableOverloadMatch),
    /// Call-compatible, but an availability clause failed, with the
    /// clause's message when it declared one.
    Unavailable(Option<String>),
    /// Rejected only by a trait bound on a solved type argument.
    Bound(TypeError),
    /// Rejected by an argument itself, whatever the candidate.
    Argument(TypeError),
    Rejected,
}

const PARAM_LIST_ADDRESS: &str = "__param_list_address";

/// Reject a use of the declaration `signature` describes when it is
/// `@unavailable`, with its decorator's message.
fn reject_unavailable(signature: Option<&CallableOriginSignature>) -> Result<(), TypeError> {
    signature
        .and_then(|signature| signature.unavailable.clone())
        .map_or(Ok(()), |message| Err(TypeError::Unavailable(message)))
}

/// The declaration each bracket argument binds, one entry per argument, as
/// [`mojito_ast::call::match_parameter_slots`] binds them; every entry is
/// `None` when the list does not bind.
fn bracket_argument_slots(
    decls: &[ParamDecl],
    param_args: &[mojito_ast::ast::ParamArg],
) -> Vec<Option<usize>> {
    let keywords = mojito_ast::call::bracket_keywords(param_args);
    mojito_ast::call::match_parameter_slots(decls, &keywords).map_or_else(
        |_| vec![None; param_args.len()],
        |slots| slots.into_iter().map(Some).collect(),
    )
}

/// Rewrite the solver's slot-indexed exact-origin mismatch into upstream's
/// call diagnostic naming the function and the parameter.
fn name_ref_origin_mismatch(error: TypeError, func: &str, names: &[String]) -> TypeError {
    match error {
        TypeError::RefOriginMismatch {
            slot,
            ty,
            expected,
            actual,
        } => TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{}' cannot be converted from '{ty}' to ref '{ty}'\nnote: operand origin '{actual}' doesn't match expected origin '{expected}'",
                names.get(slot).map_or("argument", String::as_str)
            ),
        },
        other => other,
    }
}

/// Current Mojo does not infer a generic function's specialization from the
/// parameter it is passed to, as it does not from a local annotation: only an
/// explicit specialization (`f[...]`) is a non-generic function value.
fn reject_generic_function_value(
    func: &str,
    parameter: &str,
    argument: &Expr,
    found: &Ty,
    expected: &Ty,
) -> Result<(), TypeError> {
    if matches!(found.kind(), TyKind::GenericFunc(_))
        && matches!(expected.kind(), TyKind::Func(_))
        && matches!(argument.kind, ExprKind::Identifier(_))
    {
        return Err(TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{parameter}' cannot be converted from the generic function \
                 '{found}' to '{expected}'; specialize it explicitly"
            ),
        });
    }
    Ok(())
}

/// The lone `mlir_value=` keyword of a bracketless storage construction
/// (`SIMD(mlir_value=v)`).
fn lone_storage_keyword<'a>(
    param_args: &[mojito_ast::ast::ParamArg],
    args: &[Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
) -> Option<&'a mojito_ast::ast::KwArg> {
    match kwargs {
        [keyword] if keyword.name == "mlir_value" && args.is_empty() && param_args.is_empty() => {
            Some(keyword)
        }
        _ => None,
    }
}
