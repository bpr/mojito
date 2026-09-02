//! Artifact header plus struct/function declaration decoding.

use super::*;

impl Decoder {
    pub(super) fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            source_map: ArtifactSourceMap::default(),
            files: BTreeMap::new(),
        }
    }

    pub(super) fn program(
        mut self,
        value: &Value,
    ) -> Result<(MirProgram, ArtifactSourceMap), Vec<ArtifactDiagnostic>> {
        self.mark("artifact", value.span);
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
        self.unknown(
            fields,
            &["features", "files", "structs", "decls", "functions"],
        );
        if self.diagnostics.is_empty() {
            Ok((
                MirProgram {
                    functions,
                    declarations: MirDeclarations {
                        structs,
                        functions: function_declarations,
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
            ],
        );
        Some(MirStructDeclaration {
            name,
            fields: struct_fields,
            mut_self_methods,
            fieldwise_init,
            param_decls,
            explicit_destroy_message,
            explicit_destructors,
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
        for (label, length) in [
            ("param_types", param_types.len()),
            ("defaults", defaults.len()),
            ("required", required.len()),
            ("param_conventions", param_conventions.len()),
            ("ref_params", ref_params.len()),
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
                other => {
                    self.error(value.span, format!("unknown checked constant `{other}`"));
                    None
                }
            },
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
}
