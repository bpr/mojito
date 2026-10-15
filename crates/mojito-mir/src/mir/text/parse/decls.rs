//! Artifact header plus struct/function declaration decoding.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Decoder {
    pub(super) fn new(legacy: bool) -> Self {
        Self {
            diagnostics: Vec::new(),
            source_map: ArtifactSourceMap::default(),
            files: BTreeMap::new(),
            context: ParamContext::new(),
            legacy_binders: legacy.then(HashMap::new),
            register_binders: HashMap::new(),
        }
    }

    pub(super) fn program(
        mut self,
        value: &Value,
    ) -> Result<(MirProgram, ArtifactSourceMap), Vec<ArtifactDiagnostic>> {
        self.mark("artifact", value.span);
        if self.legacy_binders.is_some() {
            self.legacy_binders = Some(Self::legacy_value_binders(value));
        }
        let Ok(fields) = self.record(value, "artifact") else {
            return Err(self.diagnostics);
        };
        let Ok(features) = self.field(fields, "features") else {
            self.error(value.span, "missing required field `features`");
            return Err(self.diagnostics);
        };
        if self.list(features).is_ok_and(|v| !v.is_empty()) {
            self.error(features.span, "features decoding is not available");
        }
        let Ok(files) = self.field(fields, "files") else {
            self.error(value.span, "missing required field `files`");
            return Err(self.diagnostics);
        };
        self.decode_files(files);
        let Ok(structs) = self.field(fields, "structs") else {
            self.error(value.span, "missing required field `structs`");
            return Err(self.diagnostics);
        };
        let structs = self.struct_declarations(structs);
        let Ok(decls) = self.field(fields, "decls") else {
            self.error(value.span, "missing required field `decls`");
            return Err(self.diagnostics);
        };
        let function_declarations = self.function_declarations(decls);
        let mut functions = Vec::new();
        let Ok(function_values) = self.field(fields, "functions") else {
            self.error(value.span, "missing required field `functions`");
            return Err(self.diagnostics);
        };
        let Ok(function_values) = self.list(function_values) else {
            return Err(self.diagnostics);
        };
        for function in function_values {
            if let Some(function) = self.function(function) {
                functions.push(function);
            }
        }
        // Schema 1.9 lists the declared traits, which schema 1.42 records
        // with their parents; an older artifact names none.
        let traits = self
            .field(fields, "traits")
            .ok()
            .and_then(|value| self.list(value).ok())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| self.trait_declaration(value))
                    .collect()
            })
            .unwrap_or_default();
        self.unknown(
            fields,
            &[
                "features",
                "files",
                "traits",
                "structs",
                "decls",
                "functions",
            ],
        );
        if self.diagnostics.is_empty() {
            Ok((
                MirProgram {
                    functions,
                    declarations: MirDeclarations {
                        structs,
                        functions: function_declarations,
                        traits,
                        imported: None,
                    },
                    invariant_errors: Vec::new(),
                },
                self.source_map,
            ))
        } else {
            Err(self.diagnostics)
        }
    }

    pub(super) fn struct_declarations(&mut self, value: &Value) -> Vec<MirStructDeclaration> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.struct_declaration(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn struct_declaration(&mut self, value: &Value) -> Option<MirStructDeclaration> {
        let fields = self.record(value, "struct").ok()?;
        let name = self.req(value, fields, "name", Self::symbol)?;
        let struct_fields = {
            let field = self.required(value, fields, "fields")?;
            let Ok(values) = self.list(field) else {
                return None;
            };
            let mut entries = Vec::new();
            for entry in values {
                let Ok(entry_fields) = self.record(entry, "field") else {
                    continue;
                };
                let field_name = self.req(entry, entry_fields, "name", Self::symbol);
                let field_ty = self.req(entry, entry_fields, "type", Self::ty);
                self.unknown(entry_fields, &["name", "type"]);
                if let (Some(field_name), Some(field_ty)) = (field_name, field_ty) {
                    entries.push((field_name, field_ty));
                }
            }
            entries
        };
        let mut_self_methods = {
            let field = self.required(value, fields, "mut_self_methods")?;
            let names = self.strings(field);
            let mut methods = HashSet::new();
            for method in names {
                if !methods.insert(method.clone()) {
                    self.error(field.span, format!("duplicate mut_self method `{method}`"));
                }
            }
            methods
        };
        let fieldwise_init = self.req(value, fields, "fieldwise_init", Self::boolean)?;
        let param_decls = self.req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?;
        let explicit_destroy_message =
            self.req(value, fields, "explicit_destroy_message", |d, v| {
                Some(d.option_string(Some(v)))
            })?;
        let explicit_destructors = {
            let field = self.required(value, fields, "explicit_destructors")?;
            let mut destructors = HashMap::new();
            if let Ok(values) = self.list(field) {
                for entry in values {
                    let Ok(entry_fields) = self.record(entry, "destructor") else {
                        continue;
                    };
                    let destructor_name = self.req(entry, entry_fields, "name", Self::symbol);
                    let raises = self.req(entry, entry_fields, "raises", Self::boolean);
                    self.unknown(entry_fields, &["name", "raises"]);
                    if let (Some(destructor_name), Some(raises)) = (destructor_name, raises)
                        && destructors
                            .insert(destructor_name.clone(), raises)
                            .is_some()
                    {
                        self.error(
                            entry.span,
                            format!("duplicate destructor `{destructor_name}`"),
                        );
                    }
                }
            }
            destructors
        };
        // Schema 1.41 says whether the message was declared and which fields
        // carry their own obligation; an older artifact says neither.
        let declared = self
            .field(fields, "explicit_destroy_declared")
            .ok()
            .and_then(|field| self.boolean(field))
            .unwrap_or(false);
        let obligated_fields = self
            .field(fields, "explicit_destroy_fields")
            .ok()
            .and_then(|entries| self.list(entries).ok())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let entry_fields = self.record(entry, "field").ok()?;
                        let name = self.req(entry, entry_fields, "name", Self::symbol);
                        let owner = self.req(entry, entry_fields, "struct", Self::symbol);
                        self.unknown(entry_fields, &["name", "struct"]);
                        Some((name?, owner?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let explicit_destroy = explicit_destroy_message.map(|message| {
            mojito_checked::declaration::ExplicitDestroyObligation {
                message,
                declared,
                fields: obligated_fields,
            }
        });
        // Schema 1.9 carries the conformance rows, and schema 1.42 the
        // declared conformances; an older artifact has none.
        let conformances = self.conformance_rows(fields, "conformances");
        let declared = self.conformance_rows(fields, "declared");
        // Schema 1.12 carries the associated types; an older artifact has none.
        let associated_types = self
            .field(fields, "associated_types")
            .ok()
            .and_then(|members| self.list(members).ok())
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| {
                        let member_fields = self.record(member, "associated_type").ok()?;
                        let name = self.req(member, member_fields, "name", Self::symbol);
                        let ty = self.req(member, member_fields, "type", Self::ty);
                        self.unknown(member_fields, &["name", "type"]);
                        Some((name?, ty?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Schema 1.46 carries the parameterized members; an older artifact
        // has none.
        let parameterized_associated_types = self
            .field(fields, "parameterized_associated_types")
            .ok()
            .and_then(|members| self.list(members).ok())
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| self.parameterized_associated_type(member))
                    .collect()
            })
            .unwrap_or_default();
        // Schema 1.36 carries the value members; an older artifact has none.
        let associated_values = self
            .field(fields, "associated_values")
            .ok()
            .and_then(|members| self.list(members).ok())
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| {
                        let member_fields = self.record(member, "associated_value").ok()?;
                        let name = self.req(member, member_fields, "name", Self::symbol);
                        let value = self.req(member, member_fields, "value", Self::param_expr);
                        self.unknown(member_fields, &["name", "value"]);
                        Some((name?, value?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.unknown(
            fields,
            &[
                "name",
                "fields",
                "mut_self_methods",
                "fieldwise_init",
                "param_decls",
                "explicit_destroy_message",
                "explicit_destructors",
                "explicit_destroy_declared",
                "explicit_destroy_fields",
                "conformances",
                "declared",
                "associated_types",
                "parameterized_associated_types",
                "associated_values",
            ],
        );
        Some(MirStructDeclaration {
            name,
            fields: struct_fields,
            mut_self_methods,
            fieldwise_init,
            param_decls,
            explicit_destructors,
            explicit_destroy,
            conformances,
            declared,
            associated_types,
            parameterized_associated_types,
            associated_values,
        })
    }

    fn parameterized_associated_type(
        &mut self,
        value: &Value,
    ) -> Option<mojito_checked::declaration::ParameterizedAssociatedType> {
        use mojito_checked::declaration::AssociatedParameter;
        let fields = self.record(value, "parameterized_associated_type").ok()?;
        let name = self.req(value, fields, "name", Self::symbol);
        let params = self.req(value, fields, "params", |d, params| {
            let params = d.list(params).ok()?;
            params
                .iter()
                .map(|param| {
                    let ValueKind::Positional(tag, inner) = &param.kind else {
                        d.error(param.span, "expected associated parameter");
                        return None;
                    };
                    match tag.as_str() {
                        "type_param" => d.binder_ref(inner).map(AssociatedParameter::Type),
                        "value_param" => d.symbol(inner).map(AssociatedParameter::Value),
                        "origin_param" => d
                            .uint32(inner)
                            .map(|id| AssociatedParameter::Origin(OriginParamId(id))),
                        other => {
                            d.error(
                                param.span,
                                format!("unknown associated parameter `{other}`"),
                            );
                            None
                        }
                    }
                })
                .collect::<Option<Vec<_>>>()
        });
        let template = self.req(value, fields, "type", Self::ty);
        self.unknown(fields, &["name", "params", "type"]);
        Some(mojito_checked::declaration::ParameterizedAssociatedType {
            name: name?,
            params: params?,
            template: template?,
        })
    }

    pub(super) fn function_declarations(&mut self, value: &Value) -> Vec<MirFunctionDeclaration> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.function_declaration(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn function_declaration(&mut self, value: &Value) -> Option<MirFunctionDeclaration> {
        let fields = self.record(value, "decl").ok()?;
        let lowered_name = self.req(value, fields, "lowered_name", Self::symbol)?;
        let param_names = self.req(value, fields, "param_names", |d, v| Some(d.strings(v)))?;
        let param_types = self.req(value, fields, "param_types", |d, v| Some(d.types(v)))?;
        let defaults = self.req(value, fields, "defaults", |d, v| {
            Some(
                d.list(v)
                    .map(|values| {
                        values
                            .iter()
                            .map(|v| d.option_value(Some(v)).and_then(|v| d.checked_const(v)))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
        })?;
        let required = self.req(value, fields, "required", |d, v| Some(d.bools(v)))?;
        let variadic = self.req(value, fields, "variadic", |d, v| Some(d.option_ty(Some(v))))?;
        let variadic_convention = self.req(value, fields, "variadic_convention", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.convention(v)))
        })?;
        let variadic_index = self.req(value, fields, "variadic_index", |d, v| {
            Some(d.option_uint(v))
        })?;
        let kw_variadic = self.req(value, fields, "kw_variadic", |d, v| {
            Some(d.option_ty(Some(v)))
        })?;
        let kw_variadic_convention =
            self.req(value, fields, "kw_variadic_convention", |d, v| {
                Some(d.option_value(Some(v)).and_then(|v| d.convention(v)))
            })?;
        let kw_variadic_index = self.req(value, fields, "kw_variadic_index", |d, v| {
            Some(d.option_uint(v))
        })?;
        let positional_only = self.req(value, fields, "positional_only", |d, v| {
            Some(d.option_uint(v))
        })?;
        let keyword_only =
            self.req(value, fields, "keyword_only", |d, v| Some(d.option_uint(v)))?;
        let param_decls = self.req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?;
        let has_receiver = self.req(value, fields, "has_receiver", Self::boolean)?;
        let receiver_convention = self.req(value, fields, "receiver_convention", |d, v| {
            Some(d.option_value(Some(v)).and_then(|v| d.convention(v)))
        })?;
        let param_conventions = self.req(value, fields, "param_conventions", |d, v| {
            Some(d.conventions(v))
        })?;
        let ret_ty = self.req(value, fields, "return_type", Self::ty)?;
        let returns_reference = self.req(value, fields, "returns_reference", Self::boolean)?;
        let raises = self.req(value, fields, "raises", Self::boolean)?;
        let error_ty = self.req(value, fields, "error_type", |d, v| {
            Some(d.option_ty(Some(v)))
        })?;
        let ref_params = self.req(value, fields, "ref_params", |d, v| Some(d.bools(v)))?;
        let param_writes = self.req(value, fields, "param_writes", |d, v| Some(d.bools(v)))?;
        // Schema 1.9 carries a member's `where` clauses; an older artifact
        // has none.
        let availability = self
            .field(fields, "availability")
            .map(|value| self.constraints(value))
            .unwrap_or_default();
        // Schema 1.32 names a nested function's enclosing declaration.
        let enclosing = self
            .field(fields, "enclosing")
            .ok()
            .and_then(|value| self.option_symbol(value));
        // Schema 1.35 declares the binders of the regions a nested function
        // is declared in.
        let region_binders = self
            .field(fields, "region_binders")
            .map(|value| self.param_decls(value))
            .unwrap_or_default();
        // Schema 1.61 records an inline level other than `automatic`.
        let inline = self
            .field(fields, "inline")
            .ok()
            .and_then(|value| self.inline_level(value))
            .unwrap_or_default();
        for (label, length) in [
            ("param_types", param_types.len()),
            ("defaults", defaults.len()),
            ("required", required.len()),
            ("param_conventions", param_conventions.len()),
            ("ref_params", ref_params.len()),
            ("param_writes", param_writes.len()),
        ] {
            if length != param_names.len() {
                self.error(
                    value.span,
                    format!(
                        "declaration `{lowered_name}` has {} parameter names but `{label}` has \
                         {length} entries",
                        param_names.len()
                    ),
                );
            }
        }
        self.unknown(
            fields,
            &[
                "lowered_name",
                "param_names",
                "param_types",
                "defaults",
                "required",
                "variadic",
                "variadic_convention",
                "variadic_index",
                "kw_variadic",
                "kw_variadic_convention",
                "kw_variadic_index",
                "positional_only",
                "keyword_only",
                "param_decls",
                "has_receiver",
                "receiver_convention",
                "param_conventions",
                "return_type",
                "returns_reference",
                "raises",
                "error_type",
                "ref_params",
                "param_writes",
                "availability",
                "enclosing",
                "region_binders",
                "inline",
            ],
        );
        Some(MirFunctionDeclaration {
            lowered_name,
            param_names,
            param_types,
            defaults,
            required,
            variadic,
            variadic_convention,
            variadic_index,
            kw_variadic,
            kw_variadic_convention,
            kw_variadic_index,
            positional_only,
            keyword_only,
            param_decls,
            has_receiver,
            receiver_convention,
            param_conventions,
            ret_ty,
            returns_reference,
            raises,
            error_ty,
            ref_params,
            param_writes,
            availability,
            enclosing,
            region_binders,
            inline,
        })
    }

    pub(super) fn checked_const(&mut self, value: &Value) -> Option<CheckedConst> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "checked_none" => Some(CheckedConst::None),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "checked_int" => self.int_literal(inner).map(CheckedConst::Int),
                "checked_float" => self.float_literal(inner).map(CheckedConst::Float),
                "checked_bool" => self.boolean(inner).map(CheckedConst::Bool),
                "checked_string" => self.string(inner).map(CheckedConst::String),
                "checked_dtype" => self.dtype(inner).map(CheckedConst::Dtype),
                "checked_evaluate" => self.string(inner).map(|function| CheckedConst::Evaluate {
                    function,
                    arguments: Vec::new(),
                }),
                other => {
                    self.error(value.span, format!("unknown checked constant `{other}`"));
                    None
                }
            },
            // Schema 1.43 applies an evaluated default's function to the
            // arguments of its own binders.
            ValueKind::Record(tag, fields) if tag == "checked_evaluate" => {
                let function = self.string(self.field(fields, "function").ok()?)?;
                let arguments = self.ty_args(self.field(fields, "arguments").ok()?);
                Some(CheckedConst::Evaluate {
                    function,
                    arguments,
                })
            }
            ValueKind::Record(tag, fields) if tag == "checked_construct" => {
                let target = self.string(self.field(fields, "target").ok()?)?;
                let arg = self.checked_const(self.field(fields, "arg").ok()?)?;
                Some(CheckedConst::Construct {
                    target,
                    arg: Box::new(arg),
                })
            }
            _ => {
                self.error(value.span, "expected checked constant");
                None
            }
        }
    }

    pub(super) fn decode_files(&mut self, value: &Value) {
        let Ok(values) = self.list(value) else {
            return;
        };
        for (expected, value) in values.iter().enumerate() {
            let Ok(fields) = self.record(value, "file") else {
                continue;
            };
            let id = self.identity(self.field(fields, "id").ok(), "file");
            if id != Some(expected) {
                self.error(
                    value.span,
                    format!("file identities must be dense; expected file{expected}"),
                );
            }
            let path = self.option_string(self.field(fields, "path").ok());
            let _module = self.option_string(self.field(fields, "module").ok());
            if let Some(id) = id {
                self.files.insert(id, path);
            }
            self.unknown(fields, &["id", "path", "module"]);
        }
    }

    /// A trait of the artifact's list: a schema 1.42 `trait` record, or an
    /// older artifact's bare name, which records no parents. A record
    /// before schema 1.44 records no requirements, and one before schema
    /// 1.48 no requirement's signature: its empty dispatch symbol names no
    /// call, so a call through a bound passes its arguments by address.
    fn trait_declaration(&mut self, value: &Value) -> Option<TraitDeclaration> {
        if !matches!(value.kind, ValueKind::Record(..)) {
            return Some(TraitDeclaration {
                name: self.symbol(value)?,
                refines: Vec::new(),
                requirements: Vec::new(),
            });
        }
        let fields = self.record(value, "trait").ok()?;
        let name = self.req(value, fields, "name", Self::symbol);
        let refines = self.req(value, fields, "refines", |d, v| Some(d.strings(v)));
        let requirements = self
            .field(fields, "requirements")
            .ok()
            .and_then(|rows| self.list(rows).ok())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let row_fields = self.record(row, "requirement").ok()?;
                        let name = self.req(row, row_fields, "name", Self::symbol);
                        let arity = self.req(row, row_fields, "arity", Self::uint);
                        let dispatch = self
                            .field(row_fields, "dispatch")
                            .ok()
                            .and_then(|value| self.symbol(value))
                            .unwrap_or_default();
                        let param_names = self
                            .field(row_fields, "param_names")
                            .map(|value| self.strings(value))
                            .unwrap_or_default();
                        let param_types = self
                            .field(row_fields, "param_types")
                            .map(|value| self.types(value))
                            .unwrap_or_default();
                        let param_conventions = self
                            .field(row_fields, "param_conventions")
                            .map(|value| self.conventions(value))
                            .unwrap_or_default();
                        let variadic_index = self
                            .field(row_fields, "variadic_index")
                            .ok()
                            .and_then(|value| self.option_uint(value));
                        self.unknown(
                            row_fields,
                            &[
                                "name",
                                "arity",
                                "dispatch",
                                "param_names",
                                "param_types",
                                "param_conventions",
                                "variadic_index",
                            ],
                        );
                        Some(TraitRequirement {
                            name: name?,
                            arity: arity?,
                            dispatch,
                            param_names,
                            param_types,
                            param_conventions,
                            variadic_index,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.unknown(fields, &["name", "refines", "requirements"]);
        Some(TraitDeclaration {
            name: name?,
            refines: refines?,
            requirements,
        })
    }

    /// A struct's `conformance` records under `field`, none when it is
    /// absent.
    fn conformance_rows(
        &mut self,
        fields: &[Field],
        field: &str,
    ) -> Vec<mojito_checked::checked::StructConformance> {
        self.field(fields, field)
            .ok()
            .and_then(|rows| self.list(rows).ok())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let row_fields = self.record(row, "conformance").ok()?;
                        let trait_name = self.req(row, row_fields, "trait", Self::symbol);
                        let conditions =
                            self.req(row, row_fields, "conditions", |d, v| Some(d.constraints(v)));
                        self.unknown(row_fields, &["trait", "conditions"]);
                        Some(mojito_checked::checked::StructConformance {
                            trait_name: trait_name?,
                            conditions: conditions?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn inline_level(&mut self, value: &Value) -> Option<mojito_checked::declaration::InlineLevel> {
        let spelling = self.atom(value)?;
        let level = mojito_checked::declaration::InlineLevel::from_spelling(spelling);
        if level.is_none() {
            let message = format!("unknown inline level `{spelling}`");
            self.error(value.span, message);
        }
        level
    }
}
