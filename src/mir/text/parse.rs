use super::{ArtifactDiagnostic, ArtifactReport, ArtifactSourceMap, ParsedArtifact};
use crate::ast::{ArgConvention, Dtype, InfixOp, PrefixOp};
use crate::checked::{
    CheckedCallArgument, CheckedCallArgumentSource, CheckedConst, CheckedIteratorCall,
    CheckedResultAdapter, IterationMode, TransferEffect, TransferSet,
};
use crate::ct::{CtExpr, CtValue};
use crate::literal::{FloatLiteral, IntLiteral};
use crate::mir::{
    Const, FuncRef, MirBlock, MirCaptureAccess, MirCaptureMode, MirClosureCapture, MirDeclarations,
    MirFunction, MirFunctionDeclaration, MirInstr, MirInteriorOrigin, MirIntrinsicSubscript,
    MirLoan, MirParamArg, MirPlace, MirProgram, MirStructDeclaration, MirSubscriptArg,
    MirSubscriptCall, MirTerm, Proj, Reg, SpanTable, UseMode,
};
use crate::origin::{
    CallableEnvironment, CaptureAccess, CaptureOrigin, CaptureOriginSet, CaptureSetParamId,
    Mutability, Origin, OriginParamId, OriginPlace, OriginSeg, OwnerId, PointerOrigin, RefSig,
    RefTy, SigMutability, SigOrigin,
};
use crate::token::SourceSpan;
use crate::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, PackPredicateRef,
    ParamDecl, SliceKind, TrivialLifecycle, Ty, TyArg,
};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_DIAGNOSTICS: usize = 64;

#[derive(Debug, Clone)]
struct Value {
    kind: ValueKind,
    span: (usize, usize),
}

#[derive(Debug, Clone)]
enum ValueKind {
    Atom(String),
    String(String),
    List(Vec<Value>),
    Positional(String, Box<Value>),
    Record(String, Vec<Field>),
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    name_span: (usize, usize),
    value: Value,
}

pub(super) fn artifact(
    input: &[u8],
    source_name: String,
) -> Result<ParsedArtifact, ArtifactReport> {
    let source = match std::str::from_utf8(input) {
        Ok(source) => source,
        Err(error) => {
            let start = error.valid_up_to();
            return Err(report(
                source_name,
                vec![diagnostic(
                    (start, start + 1),
                    "artifact is not valid UTF-8",
                )],
            ));
        }
    };
    if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(report(
            source_name,
            vec![diagnostic((0, 3), "byte-order mark is not permitted")],
        ));
    }
    let mut parser = Parser::new(source);
    parser.header();
    let value = parser.value();
    parser.space();
    if parser.pos < source.len() {
        parser.error((parser.pos, source.len()), "trailing tokens after artifact");
    }
    if !parser.diagnostics.is_empty() {
        return Err(report(source_name, parser.diagnostics));
    }
    let Some(value) = value else {
        return Err(report(
            source_name,
            vec![diagnostic((0, input.len()), "missing artifact record")],
        ));
    };
    match Decoder::new().program(&value) {
        Ok((program, source_map)) => Ok(ParsedArtifact {
            program,
            source_map,
        }),
        Err(diagnostics) => Err(report(source_name, diagnostics)),
    }
}

fn report(source_name: String, diagnostics: Vec<ArtifactDiagnostic>) -> ArtifactReport {
    ArtifactReport {
        source_name,
        diagnostics,
    }
}

fn diagnostic(span: (usize, usize), message: impl Into<String>) -> ArtifactDiagnostic {
    ArtifactDiagnostic {
        span,
        message: message.into(),
        context: Vec::new(),
    }
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    diagnostics: Vec<ArtifactDiagnostic>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    fn header(&mut self) {
        self.space();
        let start = self.pos;
        let magic = self.atom();
        if magic.as_deref() != Some("mojito-mir") {
            self.error(
                (start, self.pos.max(start + 1)),
                "expected `mojito-mir` header",
            );
        }
        self.space();
        let version_start = self.pos;
        let version = self.take_while(|byte| byte.is_ascii_digit() || byte == b'.');
        match version {
            "1.0" => {}
            value if value.starts_with("1.") => self.error(
                (version_start, self.pos),
                "unsupported MIR 1.x minor version",
            ),
            _ => self.error(
                (version_start, self.pos.max(version_start + 1)),
                "unsupported MIR artifact version",
            ),
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.space();
        let start = self.pos;
        match self.peek()? {
            b'[' => {
                self.pos += 1;
                let mut values = Vec::new();
                loop {
                    self.space();
                    if self.eat(b']') {
                        break;
                    }
                    let before = self.pos;
                    if let Some(value) = self.value() {
                        values.push(value);
                    }
                    if self.pos == before {
                        self.pos += 1;
                    }
                    self.space();
                    if self.eat(b']') {
                        break;
                    }
                    if !self.eat(b',') {
                        self.error_here("expected `,` or `]`");
                        self.sync(b']');
                    }
                }
                Some(Value {
                    kind: ValueKind::List(values),
                    span: (start, self.pos),
                })
            }
            b'"' => self.string().map(|value| Value {
                kind: ValueKind::String(value),
                span: (start, self.pos),
            }),
            _ => {
                let tag = self.atom()?;
                self.space();
                if self.eat(b'(') {
                    let value = self.value()?;
                    self.space();
                    if !self.eat(b')') {
                        self.error_here("expected `)`");
                    }
                    Some(Value {
                        kind: ValueKind::Positional(tag, Box::new(value)),
                        span: (start, self.pos),
                    })
                } else if self.eat(b'{') {
                    let mut fields = Vec::new();
                    loop {
                        self.space();
                        if self.eat(b'}') {
                            break;
                        }
                        let name_start = self.pos;
                        let Some(name) = self.atom() else {
                            self.error_here("expected field name");
                            self.sync(b'}');
                            continue;
                        };
                        let name_span = (name_start, self.pos);
                        self.space();
                        if !self.eat(b':') {
                            self.error_here("expected `:` after field name");
                        }
                        if let Some(value) = self.value() {
                            fields.push(Field {
                                name,
                                name_span,
                                value,
                            });
                        }
                        self.space();
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            self.error_here("expected `,` or `}`");
                            self.sync(b'}');
                        }
                    }
                    Some(Value {
                        kind: ValueKind::Record(tag, fields),
                        span: (start, self.pos),
                    })
                } else {
                    Some(Value {
                        kind: ValueKind::Atom(tag),
                        span: (start, self.pos),
                    })
                }
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        let start = self.pos;
        self.pos += 1;
        let mut output = String::new();
        while let Some(character) = self.source[self.pos..].chars().next() {
            self.pos += character.len_utf8();
            match character {
                '"' => return Some(output),
                '\\' => {
                    let escape_start = self.pos - 1;
                    let Some(escape) = self.source[self.pos..].chars().next() else {
                        break;
                    };
                    self.pos += escape.len_utf8();
                    match escape {
                        '"' => output.push('"'),
                        '\\' => output.push('\\'),
                        'n' => output.push('\n'),
                        'r' => output.push('\r'),
                        't' => output.push('\t'),
                        'u' => {
                            if !self.eat(b'{') {
                                self.error(
                                    (escape_start, self.pos),
                                    "expected `{` in Unicode escape",
                                );
                                continue;
                            }
                            let digits_start = self.pos;
                            let digits = self.take_while(|byte| byte.is_ascii_hexdigit());
                            let scalar = u32::from_str_radix(digits, 16)
                                .ok()
                                .and_then(char::from_u32);
                            let closed = self.eat(b'}');
                            if let Some(scalar) = scalar.filter(|_| !digits.is_empty() && closed) {
                                output.push(scalar);
                            } else {
                                self.error(
                                    (escape_start, self.pos),
                                    "invalid Unicode scalar escape",
                                );
                            }
                            if self.pos == digits_start {
                                self.pos = self.pos.max(digits_start + 1);
                            }
                        }
                        _ => self.error((escape_start, self.pos), "unknown string escape"),
                    }
                }
                value if value.is_control() => self.error(
                    (self.pos - value.len_utf8(), self.pos),
                    "unescaped control character in string",
                ),
                value => output.push(value),
            }
        }
        self.error((start, self.pos), "unterminated string");
        None
    }

    fn atom(&mut self) -> Option<String> {
        self.space();
        let value =
            self.take_while(|byte| !byte.is_ascii_whitespace() && !b"[]{}():,#".contains(&byte));
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    }

    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.pos += 1;
            }
            if self.peek() == Some(b'#') {
                self.pos += 1;
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }
    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }
    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn take_while(&mut self, predicate: impl Fn(u8) -> bool) -> &'a str {
        let start = self.pos;
        while self.peek().is_some_and(&predicate) {
            self.pos += 1;
        }
        &self.source[start..self.pos]
    }
    fn error_here(&mut self, message: &str) {
        self.error((self.pos, (self.pos + 1).min(self.source.len())), message);
    }
    fn error(&mut self, span: (usize, usize), message: &str) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(diagnostic(span, message));
        }
    }
    fn sync(&mut self, closer: u8) {
        while let Some(byte) = self.peek() {
            if byte == b',' {
                self.pos += 1;
                break;
            }
            if byte == closer {
                break;
            }
            self.pos += 1;
        }
    }
}

struct Decoder {
    diagnostics: Vec<ArtifactDiagnostic>,
    source_map: ArtifactSourceMap,
    files: BTreeMap<usize, Option<String>>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            source_map: ArtifactSourceMap::default(),
            files: BTreeMap::new(),
        }
    }

    fn program(
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

    fn struct_declarations(&mut self, value: &Value) -> Vec<MirStructDeclaration> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.struct_declaration(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn struct_declaration(&mut self, value: &Value) -> Option<MirStructDeclaration> {
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

    fn function_declarations(&mut self, value: &Value) -> Vec<MirFunctionDeclaration> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.function_declaration(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn function_declaration(&mut self, value: &Value) -> Option<MirFunctionDeclaration> {
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

    fn checked_const(&mut self, value: &Value) -> Option<CheckedConst> {
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

    fn decode_files(&mut self, value: &Value) {
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

    fn function(&mut self, value: &Value) -> Option<(String, MirFunction)> {
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

    fn blocks(&mut self, value: &Value, function: &str) -> Vec<MirBlock> {
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

    fn instruction(&mut self, value: &Value) -> Option<MirInstr> {
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

    fn term(&mut self, value: &Value) -> Option<MirTerm> {
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
    fn region_blocks(&mut self, value: &Value) -> Vec<MirBlock> {
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

    fn use_mode(&mut self, value: &Value) -> Option<UseMode> {
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

    fn prefix_op(&mut self, value: &Value) -> Option<PrefixOp> {
        match self.atom(value)? {
            "neg" => Some(PrefixOp::Neg),
            "not" => Some(PrefixOp::Not),
            other => {
                self.error(value.span, format!("unknown unary operator `{other}`"));
                None
            }
        }
    }

    fn infix_op(&mut self, value: &Value) -> Option<InfixOp> {
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

    fn slice_kind(&mut self, value: &Value) -> Option<SliceKind> {
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

    fn iteration_mode(&mut self, value: &Value) -> Option<IterationMode> {
        match self.atom(value)? {
            "borrowed" => Some(IterationMode::Borrowed),
            "owned" => Some(IterationMode::Owned),
            other => {
                self.error(value.span, format!("unknown iteration mode `{other}`"));
                None
            }
        }
    }

    fn intrinsic_subscript(&mut self, value: &Value) -> Option<MirIntrinsicSubscript> {
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

    fn result_adapter(&mut self, value: &Value) -> Option<CheckedResultAdapter> {
        match self.atom(value)? {
            "copy_iterator_reference" => Some(CheckedResultAdapter::CopyIteratorReference),
            other => {
                self.error(value.span, format!("unknown result adapter `{other}`"));
                None
            }
        }
    }

    fn place(&mut self, value: &Value) -> Option<MirPlace> {
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

    fn projection(&mut self, value: &Value) -> Option<Proj> {
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

    fn option_place(&mut self, value: &Value) -> Option<MirPlace> {
        self.option_value(Some(value)).and_then(|v| self.place(v))
    }

    fn places_option(&mut self, value: &Value) -> Vec<Option<MirPlace>> {
        self.list(value)
            .map(|values| values.iter().map(|v| self.option_place(v)).collect())
            .unwrap_or_default()
    }

    fn loans(&mut self, value: &Value) -> Vec<MirLoan> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.loan(v)).collect())
            .unwrap_or_default()
    }

    fn loan(&mut self, value: &Value) -> Option<MirLoan> {
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

    fn mir_interior_origin(&mut self, value: &Value) -> Option<MirInteriorOrigin> {
        let fields = self.record(value, "interior_origin").ok()?;
        let root = self.req(value, fields, "root", Self::var)?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        self.unknown(fields, &["root", "path"]);
        Some(MirInteriorOrigin { root, path })
    }

    fn mir_capture_accesses(&mut self, value: &Value) -> Vec<MirCaptureAccess> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.mir_capture_access(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn mir_capture_access(&mut self, value: &Value) -> Option<MirCaptureAccess> {
        let fields = self.record(value, "capture_access").ok()?;
        let root = self.req(value, fields, "root", Self::var)?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        let access = self.req(value, fields, "access", Self::capture_access_kind)?;
        self.unknown(fields, &["root", "path", "access"]);
        Some(MirCaptureAccess { root, path, access })
    }

    fn mir_param_args(&mut self, value: &Value) -> Vec<MirParamArg> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.mir_param_arg(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn mir_param_arg(&mut self, value: &Value) -> Option<MirParamArg> {
        let fields = self.record(value, "param_arg").ok()?;
        let name = self.req(value, fields, "name", |d, v| Some(d.option_symbol(v)))?;
        let param_value = self.req(value, fields, "value", |d, v| Some(d.option_reg(v)))?;
        self.unknown(fields, &["name", "value"]);
        Some(MirParamArg {
            name,
            value: param_value,
        })
    }

    fn subscript_args(&mut self, value: &Value) -> Vec<MirSubscriptArg> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.subscript_arg(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn subscript_arg(&mut self, value: &Value) -> Option<MirSubscriptArg> {
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

    fn subscript_kwargs(&mut self, value: &Value) -> Vec<(String, MirSubscriptArg)> {
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

    fn subscript_call(&mut self, value: &Value) -> Option<MirSubscriptCall> {
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

    fn call_arguments(&mut self, value: &Value) -> Vec<CheckedCallArgument> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| self.call_argument(v))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn call_argument(&mut self, value: &Value) -> Option<CheckedCallArgument> {
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

    fn call_argument_source(&mut self, value: &Value) -> Option<CheckedCallArgumentSource> {
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

    fn iterator_call(&mut self, value: &Value) -> Option<CheckedIteratorCall> {
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

    fn closure_captures(&mut self, value: &Value) -> Vec<MirClosureCapture> {
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

    fn capture_mode(&mut self, value: &Value) -> Option<MirCaptureMode> {
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

    fn reg(&mut self, value: &Value) -> Option<Reg> {
        self.identity(Some(value), "%r").map(|v| Reg(v as u32))
    }

    fn var(&mut self, value: &Value) -> Option<u32> {
        self.identity(Some(value), "$v").map(|v| v as u32)
    }

    fn block_id(&mut self, value: &Value) -> Option<usize> {
        self.identity(Some(value), "bb")
    }

    fn regs(&mut self, value: &Value) -> Vec<Reg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.reg(v)).collect())
            .unwrap_or_default()
    }

    fn vars(&mut self, value: &Value) -> Vec<u32> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.var(v)).collect())
            .unwrap_or_default()
    }

    fn option_reg(&mut self, value: &Value) -> Option<Reg> {
        self.option_value(Some(value)).and_then(|v| self.reg(v))
    }

    fn option_var(&mut self, value: &Value) -> Option<u32> {
        self.option_value(Some(value)).and_then(|v| self.var(v))
    }

    fn option_symbol(&mut self, value: &Value) -> Option<String> {
        self.option_value(Some(value)).and_then(|v| self.symbol(v))
    }

    fn kwargs(&mut self, value: &Value) -> Vec<(String, Reg)> {
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

    fn constant(&mut self, value: &Value) -> Option<Const> {
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

    fn ty(&mut self, value: &Value) -> Option<Ty> {
        match &value.kind {
            ValueKind::Atom(tag) => Some(match tag.as_str() {
                "Int" => Ty::Int,
                "UInt" => Ty::UInt,
                "Bool" => Ty::Bool,
                "StringLiteral" => Ty::StringLiteral,
                "Float64" => Ty::Float64,
                "None" => Ty::None,
                "Never" => Ty::Never,
                "IntLiteral" => Ty::IntLiteral,
                "FloatLiteral" => Ty::FloatLiteral,
                "Infer" => Ty::Infer,
                "DType" => Ty::Dtype,
                "Self" => Ty::SelfType,
                "Error" => Ty::Error,
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            ValueKind::Positional(tag, inner) => Some(match tag.as_str() {
                "overload" => Ty::Overload(self.types(inner)),
                "comptime_list" => Ty::ComptimeList(Box::new(self.ty(inner)?)),
                "tuple" => Ty::Tuple(self.types(inner)),
                "runtime_pack" => Ty::RuntimePack(self.types(inner)),
                "variadic_pack" => Ty::VariadicPack(Box::new(self.ty(inner)?)),
                "variant" => Ty::Variant(self.types(inner)),
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            ValueKind::Record(tag, fields) => Some(match tag.as_str() {
                "func" => self.callable_ty(value, fields, false)?,
                "generic_func" => self.callable_ty(value, fields, true)?,
                "param" => {
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let bounds = self.req(value, fields, "bounds", |d, v| Some(d.strings(v)))?;
                    let callable_bound = self
                        .req(value, fields, "callable_bound", |d, v| {
                            Some(d.option_ty(Some(v)))
                        })?
                        .map(Box::new);
                    self.unknown(fields, &["name", "bounds", "callable_bound"]);
                    Ty::Param {
                        name,
                        bounds,
                        callable_bound,
                    }
                }
                "assoc" => {
                    let base = Box::new(self.req(value, fields, "base", Self::ty)?);
                    let name = self.req(value, fields, "member", Self::symbol)?;
                    let args = self.req(value, fields, "arguments", |d, v| Some(d.ty_args(v)))?;
                    self.unknown(fields, &["base", "member", "arguments"]);
                    Ty::Assoc { base, name, args }
                }
                "dependent_index" => {
                    let elements = self.req(value, fields, "elements", |d, v| Some(d.types(v)))?;
                    let index = self.req(value, fields, "index", Self::ct_expr)?;
                    self.unknown(fields, &["elements", "index"]);
                    Ty::Dependent(DependentType::Indexed { elements, index })
                }
                "struct_type" => {
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let args = self.req(value, fields, "arguments", |d, v| Some(d.ty_args(v)))?;
                    self.unknown(fields, &["name", "arguments"]);
                    Ty::Struct(name, args)
                }
                "simd" => {
                    let dtype = self.req(value, fields, "dtype", Self::dtype)?;
                    let width = self.req(value, fields, "width", Self::int64)?;
                    self.unknown(fields, &["dtype", "width"]);
                    Ty::Simd { dtype, width }
                }
                "pointer" => {
                    let element = Box::new(self.req(value, fields, "element", Self::ty)?);
                    let origin = self.req(value, fields, "origin", Self::pointer_origin)?;
                    self.unknown(fields, &["element", "origin"]);
                    Ty::Pointer { element, origin }
                }
                "ref" => Ty::Ref(self.ref_ty(value)?),
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            _ => {
                self.error(value.span, "expected type");
                None
            }
        }
    }

    fn callable_ty(&mut self, value: &Value, fields: &[Field], generic: bool) -> Option<Ty> {
        let environment = self.req(value, fields, "environment", Self::environment)?;
        let decls = self.req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?;
        let params = self.req(value, fields, "params", |d, v| Some(d.types(v)))?;
        let names = self.req(value, fields, "names", |d, v| Some(d.strings(v)))?;
        let ret = Box::new(self.req(value, fields, "return_type", Self::ty)?);
        let required = self.req(value, fields, "required", |d, v| Some(d.bools(v)))?;
        let variadic = self
            .req(value, fields, "variadic", |d, v| Some(d.option_ty(Some(v))))?
            .map(Box::new);
        let kw_variadic = self
            .req(value, fields, "kw_variadic", |d, v| {
                Some(d.option_ty(Some(v)))
            })?
            .map(Box::new);
        let positional_only = self.req(value, fields, "positional_only", |d, v| {
            Some(d.option_uint(v))
        })?;
        let keyword_only =
            self.req(value, fields, "keyword_only", |d, v| Some(d.option_uint(v)))?;
        let raises = self.req(value, fields, "raises", Self::boolean)?;
        let error = self
            .req(value, fields, "error_type", |d, v| {
                Some(d.option_ty(Some(v)))
            })?
            .map(Box::new);
        let conventions = self.req(value, fields, "conventions", |d, v| Some(d.conventions(v)))?;
        let ref_params =
            Box::new(self.req(value, fields, "ref_params", |d, v| Some(d.ref_sigs(v)))?);
        let ref_return = {
            let field = self.required(value, fields, "ref_return")?;
            self.option_value(Some(field))
                .and_then(|v| self.ref_sig(v))
                .map(Box::new)
        };
        let transfers = self.req(value, fields, "transfers", |d, v| Some(d.transfer_set(v)))?;
        self.unknown(
            fields,
            &[
                "environment",
                "param_decls",
                "params",
                "names",
                "return_type",
                "required",
                "variadic",
                "kw_variadic",
                "positional_only",
                "keyword_only",
                "raises",
                "error_type",
                "conventions",
                "ref_params",
                "ref_return",
                "transfers",
            ],
        );
        if generic {
            return Some(Ty::GenericFunc {
                environment,
                decls,
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
                transfers,
            });
        }
        if !decls.is_empty() {
            self.error(value.span, "`func` types take no param_decls");
            return None;
        }
        Some(Ty::Func {
            environment,
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
            transfers,
        })
    }

    fn transfer_set(&mut self, value: &Value) -> TransferSet {
        TransferSet(
            self.list(value)
                .map(|values| values.iter().filter_map(|v| self.transfer(v)).collect())
                .unwrap_or_default(),
        )
    }

    fn transfer(&mut self, value: &Value) -> Option<TransferEffect> {
        let fields = self.record(value, "transfer").ok()?;
        let dest = self.req(value, fields, "dest", Self::sig_origin)?;
        let src = self.req(value, fields, "src", Self::sig_origin)?;
        let src_is_place = self.req(value, fields, "src_is_place", Self::boolean)?;
        let mutable = self.req(value, fields, "mutable", Self::boolean)?;
        self.unknown(fields, &["dest", "src", "src_is_place", "mutable"]);
        Some(TransferEffect {
            dest,
            src,
            src_is_place,
            mutable,
        })
    }

    fn ty_args(&mut self, value: &Value) -> Vec<TyArg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.ty_arg(v)).collect())
            .unwrap_or_default()
    }

    fn ty_arg(&mut self, value: &Value) -> Option<TyArg> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "type_arg" => self.ty(inner).map(TyArg::Ty),
            "value_arg" => self.ct_value(inner).map(TyArg::Val),
            "origin_arg" => self.origin(inner).map(TyArg::Origin),
            other => {
                self.error(value.span, format!("unknown type argument `{other}`"));
                None
            }
        }
    }

    fn param_decls(&mut self, value: &Value) -> Vec<ParamDecl> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.param_decl(v)).collect())
            .unwrap_or_default()
    }

    fn param_decl(&mut self, value: &Value) -> Option<ParamDecl> {
        let (tag, fields) = self.any_record(value)?;
        match tag {
            "type_param" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let bounds = self.req(value, fields, "bounds", |d, v| Some(d.strings(v)))?;
                let callable_bound = self
                    .req(value, fields, "callable_bound", |d, v| {
                        Some(d.option_ty(Some(v)))
                    })?
                    .map(Box::new);
                let default = self
                    .req(value, fields, "default", |d, v| Some(d.option_ty(Some(v))))?
                    .map(Box::new);
                let infer_only = self.req(value, fields, "infer_only", Self::boolean)?;
                let variadic = self.req(value, fields, "variadic", Self::boolean)?;
                let constraints =
                    self.req(value, fields, "constraints", |d, v| Some(d.constraints(v)))?;
                self.unknown(
                    fields,
                    &[
                        "name",
                        "bounds",
                        "callable_bound",
                        "default",
                        "infer_only",
                        "variadic",
                        "constraints",
                    ],
                );
                Some(ParamDecl::Type {
                    name,
                    bounds,
                    callable_bound,
                    default,
                    infer_only,
                    variadic,
                    constraints,
                })
            }
            "value_param" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let ty = Box::new(self.req(value, fields, "type", Self::ty)?);
                let default = {
                    let field = self.required(value, fields, "default")?;
                    self.option_value(Some(field)).and_then(|v| self.ct_expr(v))
                };
                let callable_default = {
                    let field = self.required(value, fields, "callable_default")?;
                    self.option_value(Some(field))
                        .and_then(|v| self.callable_default(v))
                };
                let infer_only = self.req(value, fields, "infer_only", Self::boolean)?;
                let variadic = self.req(value, fields, "variadic", Self::boolean)?;
                let constraints =
                    self.req(value, fields, "constraints", |d, v| Some(d.constraints(v)))?;
                self.unknown(
                    fields,
                    &[
                        "name",
                        "type",
                        "default",
                        "callable_default",
                        "infer_only",
                        "variadic",
                        "constraints",
                    ],
                );
                Some(ParamDecl::Value {
                    name,
                    ty,
                    default,
                    callable_default,
                    infer_only,
                    variadic,
                    constraints,
                })
            }
            other => {
                self.error(
                    value.span,
                    format!("unknown parameter declaration `{other}`"),
                );
                None
            }
        }
    }

    fn constraints(&mut self, value: &Value) -> Vec<GenericConstraint> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.constraint(v)).collect())
            .unwrap_or_default()
    }

    fn constraint(&mut self, value: &Value) -> Option<GenericConstraint> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "not" => Some(GenericConstraint::Not(Box::new(self.constraint(inner)?))),
                "constraint_bool" => self.boolean(inner).map(GenericConstraint::Bool),
                other => {
                    self.error(value.span, format!("unknown constraint `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "with_message" => {
                    let condition =
                        Box::new(self.req(value, fields, "condition", Self::constraint)?);
                    let message = self.req(value, fields, "message", Self::string)?;
                    self.unknown(fields, &["condition", "message"]);
                    Some(GenericConstraint::WithMessage(condition, message))
                }
                "conforms" | "conforms_pack" => {
                    let param = self.req(value, fields, "param", Self::symbol)?;
                    let trait_name = self.req(value, fields, "trait", Self::symbol)?;
                    self.unknown(fields, &["param", "trait"]);
                    Some(if tag == "conforms" {
                        GenericConstraint::Conforms { param, trait_name }
                    } else {
                        GenericConstraint::ConformsPack { param, trait_name }
                    })
                }
                "pack_predicate" => {
                    let param = self.req(value, fields, "param", Self::symbol)?;
                    let predicate = self.req(value, fields, "predicate", Self::pack_predicate)?;
                    let all = self.req(value, fields, "all", Self::boolean)?;
                    self.unknown(fields, &["param", "predicate", "all"]);
                    Some(GenericConstraint::PackPredicate {
                        param,
                        predicate,
                        all,
                    })
                }
                "pack_contains" => {
                    let param = self.req(value, fields, "param", Self::symbol)?;
                    let element = self.req(value, fields, "element", Self::constraint_operand)?;
                    self.unknown(fields, &["param", "element"]);
                    Some(GenericConstraint::PackContains { param, element })
                }
                "trivial" => {
                    let kind = self.req(value, fields, "lifecycle", Self::lifecycle)?;
                    let operand = self.req(value, fields, "operand", Self::constraint_operand)?;
                    self.unknown(fields, &["lifecycle", "operand"]);
                    Some(GenericConstraint::Trivial(kind, operand))
                }
                "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                    let left = self.req(value, fields, "left", Self::constraint_operand)?;
                    let right = self.req(value, fields, "right", Self::constraint_operand)?;
                    self.unknown(fields, &["left", "right"]);
                    Some(match tag.as_str() {
                        "eq" => GenericConstraint::Eq(left, right),
                        "ne" => GenericConstraint::Ne(left, right),
                        "lt" => GenericConstraint::Lt(left, right),
                        "le" => GenericConstraint::Le(left, right),
                        "gt" => GenericConstraint::Gt(left, right),
                        _ => GenericConstraint::Ge(left, right),
                    })
                }
                "and" | "or" => {
                    let left = Box::new(self.req(value, fields, "left", Self::constraint)?);
                    let right = Box::new(self.req(value, fields, "right", Self::constraint)?);
                    self.unknown(fields, &["left", "right"]);
                    Some(if tag == "and" {
                        GenericConstraint::And(left, right)
                    } else {
                        GenericConstraint::Or(left, right)
                    })
                }
                other => {
                    self.error(value.span, format!("unknown constraint `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected constraint");
                None
            }
        }
    }

    fn constraint_operand(&mut self, value: &Value) -> Option<ConstraintOperand> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "operand_param" => self.symbol(inner).map(ConstraintOperand::Param),
            "operand_value" => self.ct_value(inner).map(ConstraintOperand::Value),
            "operand_type" => self.ty(inner).map(ConstraintOperand::Type),
            "operand_pack_length" => self.symbol(inner).map(ConstraintOperand::PackLength),
            other => {
                self.error(value.span, format!("unknown constraint operand `{other}`"));
                None
            }
        }
    }

    fn pack_predicate(&mut self, value: &Value) -> Option<PackPredicateRef> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "predicate_trivial" => self.lifecycle(inner).map(PackPredicateRef::Trivial),
            "predicate_alias" => self.symbol(inner).map(PackPredicateRef::Alias),
            other => {
                self.error(value.span, format!("unknown pack predicate `{other}`"));
                None
            }
        }
    }

    fn lifecycle(&mut self, value: &Value) -> Option<TrivialLifecycle> {
        match self.atom(value)? {
            "movable" => Some(TrivialLifecycle::Movable),
            "copyable" => Some(TrivialLifecycle::Copyable),
            "deinitable" => Some(TrivialLifecycle::Deinitable),
            other => {
                self.error(value.span, format!("unknown lifecycle `{other}`"));
                None
            }
        }
    }

    fn callable_default(&mut self, value: &Value) -> Option<CallableDefault> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "default_symbol" => self.symbol(inner).map(CallableDefault::Symbol),
                "default_parameter" => self.symbol(inner).map(CallableDefault::Parameter),
                other => {
                    self.error(value.span, format!("unknown callable default `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "default_if" => {
                let condition = self.req(value, fields, "condition", Self::ct_expr)?;
                let then_value =
                    Box::new(self.req(value, fields, "then_value", Self::callable_default)?);
                let else_value =
                    Box::new(self.req(value, fields, "else_value", Self::callable_default)?);
                self.unknown(fields, &["condition", "then_value", "else_value"]);
                Some(CallableDefault::If {
                    condition,
                    then_value,
                    else_value,
                })
            }
            _ => {
                self.error(value.span, "expected callable default");
                None
            }
        }
    }

    fn ct_expr(&mut self, value: &Value) -> Option<CtExpr> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "ct_value" => self.ct_value(inner).map(CtExpr::Value),
                "ct_param" => self.symbol(inner).map(CtExpr::Param),
                "ct_neg" => Some(CtExpr::Neg(Box::new(self.ct_expr(inner)?))),
                other => {
                    self.error(
                        value.span,
                        format!("unknown compile-time expression `{other}`"),
                    );
                    None
                }
            },
            ValueKind::Record(tag, fields) => {
                let make = match tag.as_str() {
                    "ct_add" => CtExpr::Add,
                    "ct_sub" => CtExpr::Sub,
                    "ct_mul" => CtExpr::Mul,
                    "ct_floor_div" => CtExpr::FloorDiv,
                    "ct_mod" => CtExpr::Mod,
                    "ct_pow" => CtExpr::Pow,
                    other => {
                        self.error(
                            value.span,
                            format!("unknown compile-time expression `{other}`"),
                        );
                        return None;
                    }
                };
                let left = Box::new(self.req(value, fields, "left", Self::ct_expr)?);
                let right = Box::new(self.req(value, fields, "right", Self::ct_expr)?);
                self.unknown(fields, &["left", "right"]);
                Some(make(left, right))
            }
            _ => {
                self.error(value.span, "expected compile-time expression");
                None
            }
        }
    }

    fn ct_values(&mut self, value: &Value) -> Vec<CtValue> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.ct_value(v)).collect())
            .unwrap_or_default()
    }

    fn ct_value(&mut self, value: &Value) -> Option<CtValue> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "ct_int" => self.int64(inner).map(CtValue::Int),
                "ct_uint" => self.uint64(inner).map(CtValue::UInt),
                "ct_float_bits" => self.float_bits(inner).map(CtValue::Float),
                "ct_int_literal" => self.int_literal(inner).map(CtValue::IntLiteral),
                "ct_float_literal" => self.float_literal(inner).map(CtValue::FloatLiteral),
                "ct_bool" => self.boolean(inner).map(CtValue::Bool),
                "ct_string" => self.string(inner).map(CtValue::Str),
                "ct_tuple" => Some(CtValue::Tuple(self.ct_values(inner))),
                "ct_list" => Some(CtValue::List(self.ct_values(inner))),
                "ct_dtype" => self.dtype(inner).map(CtValue::Dtype),
                "ct_type" => self.ty(inner).map(|ty| CtValue::Type(Box::new(ty))),
                "ct_reflected" => self.ty(inner).map(|ty| CtValue::Reflected(Box::new(ty))),
                "ct_param" => self.symbol(inner).map(CtValue::Param),
                other => {
                    self.error(value.span, format!("unknown compile-time value `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "ct_struct" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let fields_value = self.required(value, fields, "fields")?;
                let mut entries = Vec::new();
                if let Ok(values) = self.list(fields_value) {
                    for entry in values {
                        let Ok(entry_fields) = self.record(entry, "ct_field") else {
                            continue;
                        };
                        let field_name = self
                            .required(entry, entry_fields, "name")
                            .and_then(|v| self.symbol(v));
                        let field_value = self
                            .required(entry, entry_fields, "value")
                            .and_then(|v| self.ct_value(v));
                        self.unknown(entry_fields, &["name", "value"]);
                        if let (Some(field_name), Some(field_value)) = (field_name, field_value) {
                            entries.push((field_name, field_value));
                        }
                    }
                }
                self.unknown(fields, &["name", "fields"]);
                Some(CtValue::Struct {
                    name,
                    fields: entries,
                })
            }
            _ => {
                self.error(value.span, "expected compile-time value");
                None
            }
        }
    }

    fn origin_path(&mut self, value: &Value) -> Vec<OriginSeg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.origin_seg(v)).collect())
            .unwrap_or_default()
    }

    fn origin_seg(&mut self, value: &Value) -> Option<OriginSeg> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "any_index" => Some(OriginSeg::AnyIndex),
            ValueKind::Atom(tag) if tag == "subtree" => Some(OriginSeg::Subtree),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "field" => self.symbol(inner).map(OriginSeg::Field),
                "interior" => self.symbol(inner).map(OriginSeg::Interior),
                other => {
                    self.error(value.span, format!("unknown origin segment `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected origin segment");
                None
            }
        }
    }

    fn origin(&mut self, value: &Value) -> Option<Origin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "origin_self" => Some(Origin::SelfParam),
            ValueKind::Atom(tag) if tag == "origin_static" => Some(Origin::Static),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "origin_param" => self.uint32(inner).map(|v| Origin::Param(OriginParamId(v))),
                "origin_union" => {
                    let members = self
                        .list(inner)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|v| self.origin(v))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(Origin::union(members))
                }
                other => {
                    self.error(value.span, format!("unknown origin `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, _) if tag == "origin_place" => {
                self.origin_place(value).map(Origin::Place)
            }
            ValueKind::Record(tag, fields) if tag == "origin_untracked" => {
                let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                self.unknown(fields, &["mutable"]);
                Some(Origin::Untracked { mutable })
            }
            _ => {
                self.error(value.span, "expected origin");
                None
            }
        }
    }

    fn origin_place(&mut self, value: &Value) -> Option<OriginPlace> {
        let fields = self.record(value, "origin_place").ok()?;
        let root = self.req(value, fields, "root", |d, v| d.identity(Some(v), "$v"))?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        self.unknown(fields, &["root", "path"]);
        Some(OriginPlace {
            root: OwnerId(root as u32),
            path,
        })
    }

    fn pointer_origin(&mut self, value: &Value) -> Option<PointerOrigin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "pointer_static" => Some(PointerOrigin::Static),
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "pointer_place" => {
                    let place = self.req(value, fields, "place", Self::origin_place)?;
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["place", "mutable"]);
                    Some(PointerOrigin::Place { place, mutable })
                }
                "pointer_param" => {
                    let id = OriginParamId(self.req(value, fields, "id", Self::uint32)?);
                    let mutability = self.req(value, fields, "mutability", Self::mutability)?;
                    let interior =
                        self.req(value, fields, "interior", |d, v| Some(d.strings(v)))?;
                    let subtree = self.req(value, fields, "subtree", Self::boolean)?;
                    self.unknown(fields, &["id", "mutability", "interior", "subtree"]);
                    Some(PointerOrigin::Param {
                        id,
                        mutability,
                        interior,
                        subtree,
                    })
                }
                "pointer_self" => {
                    let mutability = self.req(value, fields, "mutability", Self::mutability)?;
                    let interior =
                        self.req(value, fields, "interior", |d, v| Some(d.strings(v)))?;
                    let subtree = self.req(value, fields, "subtree", Self::boolean)?;
                    self.unknown(fields, &["mutability", "interior", "subtree"]);
                    Some(PointerOrigin::SelfPlace {
                        mutability,
                        interior,
                        subtree,
                    })
                }
                "pointer_untracked" | "pointer_unsafe_any" => {
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["mutable"]);
                    Some(if tag == "pointer_untracked" {
                        PointerOrigin::Untracked { mutable }
                    } else {
                        PointerOrigin::UnsafeAny { mutable }
                    })
                }
                other => {
                    self.error(value.span, format!("unknown pointer origin `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected pointer origin");
                None
            }
        }
    }

    fn ref_ty(&mut self, value: &Value) -> Option<RefTy> {
        let fields = self.record(value, "ref").ok()?;
        let referent = Box::new(self.req(value, fields, "referent", Self::ty)?);
        let origin = self.req(value, fields, "origin", Self::origin)?;
        let mutability = self.req(value, fields, "mutability", Self::mutability)?;
        self.unknown(fields, &["referent", "origin", "mutability"]);
        Some(RefTy {
            referent,
            origin,
            mutability,
        })
    }

    fn mutability(&mut self, value: &Value) -> Option<Mutability> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "immutable" => Some(Mutability::Immutable),
            ValueKind::Atom(tag) if tag == "mutable" => Some(Mutability::Mutable),
            ValueKind::Positional(tag, inner) if tag == "mutability_param" => self
                .uint32(inner)
                .map(|v| Mutability::Param(OriginParamId(v))),
            _ => {
                self.error(value.span, "expected mutability");
                None
            }
        }
    }

    fn ref_sigs(&mut self, value: &Value) -> Vec<Option<RefSig>> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .map(|v| self.option_value(Some(v)).and_then(|v| self.ref_sig(v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ref_sig(&mut self, value: &Value) -> Option<RefSig> {
        let fields = self.record(value, "ref_sig").ok()?;
        let origin = self.req(value, fields, "origin", Self::sig_origin)?;
        let mutability = self.req(value, fields, "mutability", Self::sig_mutability)?;
        self.unknown(fields, &["origin", "mutability"]);
        Some(RefSig { origin, mutability })
    }

    fn sig_origin(&mut self, value: &Value) -> Option<SigOrigin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "sig_self" => Some(SigOrigin::Self_),
            ValueKind::Atom(tag) if tag == "sig_static" => Some(SigOrigin::Static),
            ValueKind::Atom(tag) if tag == "sig_infer" => Some(SigOrigin::Infer),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "sig_param" => self.uint(inner).map(SigOrigin::Param),
                "sig_bound" => self.origin(inner).map(SigOrigin::Bound),
                "sig_union" => {
                    let members = self
                        .list(inner)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|v| self.sig_origin(v))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(SigOrigin::Union(members))
                }
                other => {
                    self.error(value.span, format!("unknown signature origin `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "sig_untracked" => {
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["mutable"]);
                    Some(SigOrigin::Untracked { mutable })
                }
                "sig_projected" => {
                    let base = Box::new(self.req(value, fields, "base", Self::sig_origin)?);
                    let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
                    self.unknown(fields, &["base", "path"]);
                    Some(SigOrigin::Projected(base, path))
                }
                other => {
                    self.error(value.span, format!("unknown signature origin `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected signature origin");
                None
            }
        }
    }

    fn sig_mutability(&mut self, value: &Value) -> Option<SigMutability> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "sig_immutable" => Some(SigMutability::Immutable),
            ValueKind::Atom(tag) if tag == "sig_mutable" => Some(SigMutability::Mutable),
            ValueKind::Atom(tag) if tag == "sig_infer" => Some(SigMutability::Infer),
            ValueKind::Positional(tag, inner) if tag == "sig_bool_param" => {
                self.uint(inner).map(SigMutability::BoolParam)
            }
            _ => {
                self.error(value.span, "expected signature mutability");
                None
            }
        }
    }

    fn environment(&mut self, value: &Value) -> Option<CallableEnvironment> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "default" => Some(CallableEnvironment::Default),
            ValueKind::Atom(tag) if tag == "thin" => Some(CallableEnvironment::Thin),
            ValueKind::Positional(tag, inner) if tag == "capturing" => {
                self.capture_set(inner).map(CallableEnvironment::Capturing)
            }
            _ => {
                self.error(value.span, "expected callable environment");
                None
            }
        }
    }

    fn capture_set(&mut self, value: &Value) -> Option<CaptureOriginSet> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "capture_set_infer" => Some(CaptureOriginSet::Infer),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "capture_set_param" => self
                    .uint32(inner)
                    .map(|v| CaptureOriginSet::Param(CaptureSetParamId(v))),
                "capture_set" => {
                    let mut captures = Vec::new();
                    if let Ok(values) = self.list(inner) {
                        for entry in values {
                            let Ok(fields) = self.record(entry, "capture_origin") else {
                                continue;
                            };
                            let origin = self
                                .required(entry, fields, "origin")
                                .and_then(|v| self.origin(v));
                            let access = self
                                .required(entry, fields, "access")
                                .and_then(|v| self.capture_access_kind(v));
                            self.unknown(fields, &["origin", "access"]);
                            if let (Some(origin), Some(access)) = (origin, access) {
                                captures.push(CaptureOrigin { origin, access });
                            }
                        }
                    }
                    Some(CaptureOriginSet::concrete(captures))
                }
                other => {
                    self.error(value.span, format!("unknown capture set `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected capture set");
                None
            }
        }
    }

    fn capture_access_kind(&mut self, value: &Value) -> Option<CaptureAccess> {
        match self.atom(value)? {
            "read" => Some(CaptureAccess::Read),
            "write" => Some(CaptureAccess::Write),
            other => {
                self.error(value.span, format!("unknown capture access `{other}`"));
                None
            }
        }
    }

    fn convention(&mut self, value: &Value) -> Option<ArgConvention> {
        match self.atom(value)? {
            "imm" => Some(ArgConvention::Imm),
            "var" => Some(ArgConvention::Var),
            "mut" => Some(ArgConvention::Mut),
            "out" => Some(ArgConvention::Out),
            "ref" => Some(ArgConvention::Ref),
            "deinit" => Some(ArgConvention::Deinit),
            other => {
                self.error(value.span, format!("unknown convention `{other}`"));
                None
            }
        }
    }

    fn conventions(&mut self, value: &Value) -> Vec<Option<ArgConvention>> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .map(|v| self.option_value(Some(v)).and_then(|v| self.convention(v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn dtype(&mut self, value: &Value) -> Option<Dtype> {
        let atom = self.atom(value)?;
        let parsed = Dtype::from_name(atom);
        if parsed.is_none() {
            self.error(value.span, format!("unknown dtype `{atom}`"));
        }
        parsed
    }

    fn locations(&mut self, value: &Value) -> SpanTable {
        let mut output = HashMap::new();
        let Ok(values) = self.list(value) else {
            return SpanTable(output);
        };
        for value in values {
            let Ok(fields) = self.record(value, "reg_loc") else {
                continue;
            };
            let Some(reg) = self.identity(self.field(fields, "reg").ok(), "%r") else {
                continue;
            };
            let Some(location) = self.option_value(self.field(fields, "location").ok()) else {
                continue;
            };
            let Ok(fields) = self.record(location, "loc") else {
                continue;
            };
            let Some(file) = self.identity(self.field(fields, "file").ok(), "file") else {
                continue;
            };
            let Some(start) = self.uint(self.field(fields, "start").ok().unwrap_or(location))
            else {
                continue;
            };
            let Some(end) = self.uint(self.field(fields, "end").ok().unwrap_or(location)) else {
                continue;
            };
            if start > end {
                self.error(location.span, "source span start exceeds end");
                continue;
            }
            let origin = self
                .option_identity(self.field(fields, "origin").ok(), "$v")
                .map(|v| v as u32);
            let source = self.files.get(&file).cloned().flatten();
            if !self.files.contains_key(&file) {
                self.error(location.span, format!("unknown file{file}"));
                continue;
            }
            output.insert(reg as u32, (SourceSpan::new(source, (start, end)), origin));
        }
        SpanTable(output)
    }

    fn type_map(
        &mut self,
        value: &Value,
        tag: &str,
        id_field: &str,
        prefix: &str,
    ) -> BTreeMap<usize, Ty> {
        let mut out = BTreeMap::new();
        if let Ok(values) = self.list(value) {
            for value in values {
                if let Ok(fields) = self.record(value, tag)
                    && let (Some(id), Some(ty)) = (
                        self.identity(self.field(fields, id_field).ok(), prefix),
                        self.field(fields, "type").ok().and_then(|v| self.ty(v)),
                    )
                    && out.insert(id, ty).is_some()
                {
                    self.error(value.span, format!("duplicate {prefix}{id}"));
                }
            }
        }
        out
    }
    fn types(&mut self, value: &Value) -> Vec<Ty> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.ty(v)).collect())
            .unwrap_or_default()
    }
    fn strings(&mut self, value: &Value) -> Vec<String> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.symbol(v)).collect())
            .unwrap_or_default()
    }
    fn bools(&mut self, value: &Value) -> Vec<bool> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.boolean(v)).collect())
            .unwrap_or_default()
    }
    fn option_ty(&mut self, value: Option<&Value>) -> Option<Ty> {
        self.option_value(value).and_then(|v| self.ty(v))
    }
    fn option_string(&mut self, value: Option<&Value>) -> Option<String> {
        self.option_value(value).and_then(|v| self.string(v))
    }
    fn option_identity(&mut self, value: Option<&Value>, prefix: &str) -> Option<usize> {
        self.option_value(value)
            .and_then(|v| self.identity(Some(v), prefix))
    }
    fn option_value<'a>(&mut self, value: Option<&'a Value>) -> Option<&'a Value> {
        let value = value?;
        match &value.kind {
            ValueKind::Atom(v) if v == "absent" => None,
            ValueKind::Positional(tag, inner) if tag == "present" => Some(inner),
            _ => {
                self.error(value.span, "expected `absent` or `present(...)`");
                None
            }
        }
    }
    fn list<'a>(&mut self, value: &'a Value) -> Result<&'a [Value], ()> {
        match &value.kind {
            ValueKind::List(v) => Ok(v),
            _ => {
                self.error(value.span, "expected list");
                Err(())
            }
        }
    }
    fn record<'a>(&mut self, value: &'a Value, expected: &str) -> Result<&'a [Field], ()> {
        match &value.kind {
            ValueKind::Record(tag, fields) if tag == expected => Ok(fields),
            ValueKind::Record(tag, _) => {
                self.error(
                    value.span,
                    format!("expected `{expected}` record, found `{tag}`"),
                );
                Err(())
            }
            _ => {
                self.error(value.span, format!("expected `{expected}` record"));
                Err(())
            }
        }
    }
    fn any_record<'a>(&mut self, value: &'a Value) -> Option<(&'a str, &'a [Field])> {
        match &value.kind {
            ValueKind::Record(tag, fields) => Some((tag, fields)),
            _ => {
                self.error(value.span, "expected record");
                None
            }
        }
    }
    fn field<'a>(&self, fields: &'a [Field], name: &str) -> Result<&'a Value, ()> {
        let mut found = fields.iter().filter(|f| f.name == name);
        let Some(first) = found.next() else {
            return Err(());
        };
        Ok(&first.value)
    }
    fn unknown(&mut self, fields: &[Field], known: &[&str]) {
        let mut seen = std::collections::BTreeSet::new();
        for field in fields {
            if !seen.insert(field.name.as_str()) {
                self.error(field.name_span, format!("duplicate field `{}`", field.name));
            }
            if !known.contains(&field.name.as_str()) {
                self.error(field.name_span, format!("unknown field `{}`", field.name));
            }
        }
    }
    fn atom<'a>(&mut self, value: &'a Value) -> Option<&'a str> {
        match &value.kind {
            ValueKind::Atom(v) => Some(v),
            _ => {
                self.error(value.span, "expected word or number");
                None
            }
        }
    }
    fn string(&mut self, value: &Value) -> Option<String> {
        match &value.kind {
            ValueKind::String(v) => Some(v.clone()),
            _ => {
                self.error(value.span, "expected string");
                None
            }
        }
    }
    fn symbol(&mut self, value: &Value) -> Option<String> {
        match &value.kind {
            ValueKind::Atom(v) | ValueKind::String(v) => Some(v.clone()),
            _ => {
                self.error(value.span, "expected symbol");
                None
            }
        }
    }
    fn uint(&mut self, value: &Value) -> Option<usize> {
        self.atom(value)?.parse().ok().or_else(|| {
            self.error(value.span, "expected unsigned integer");
            None
        })
    }
    fn boolean(&mut self, value: &Value) -> Option<bool> {
        match self.atom(value)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => {
                self.error(value.span, "expected boolean");
                None
            }
        }
    }
    fn identity(&mut self, value: Option<&Value>, prefix: &str) -> Option<usize> {
        let value = value?;
        let atom = self.atom(value)?;
        atom.strip_prefix(prefix)
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                self.error(value.span, format!("expected {prefix} identity"));
                None
            })
    }
    /// A field the canonical emitter always writes; absence is a diagnostic,
    /// never a silent default.
    fn required<'a>(
        &mut self,
        value: &Value,
        fields: &'a [Field],
        name: &str,
    ) -> Option<&'a Value> {
        match self.field(fields, name) {
            Ok(found) => Some(found),
            Err(()) => {
                self.error(value.span, format!("missing required field `{name}`"));
                None
            }
        }
    }
    /// Look up a required field and decode it in one step — the nesting
    /// `self.decode(self.required(..)?)` would borrow the decoder twice.
    fn req<T>(
        &mut self,
        value: &Value,
        fields: &[Field],
        name: &str,
        decode: impl FnOnce(&mut Self, &Value) -> Option<T>,
    ) -> Option<T> {
        match self.field(fields, name) {
            Ok(found) => decode(self, found),
            Err(()) => {
                self.error(value.span, format!("missing required field `{name}`"));
                None
            }
        }
    }
    fn positional_value<'a>(&mut self, value: &'a Value) -> Option<(&'a str, &'a Value)> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => Some((tag, inner)),
            _ => {
                self.error(value.span, "expected tagged value");
                None
            }
        }
    }
    fn option_uint(&mut self, value: &Value) -> Option<usize> {
        self.option_value(Some(value)).and_then(|v| self.uint(v))
    }
    fn uint32(&mut self, value: &Value) -> Option<u32> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected unsigned 32-bit integer");
        }
        parsed
    }
    fn int64(&mut self, value: &Value) -> Option<i64> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected integer");
        }
        parsed
    }
    fn uint64(&mut self, value: &Value) -> Option<u64> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected unsigned integer");
        }
        parsed
    }
    fn float_bits(&mut self, value: &Value) -> Option<u64> {
        let atom = self.atom(value)?;
        let parsed = (atom.len() == 16)
            .then(|| u64::from_str_radix(atom, 16).ok())
            .flatten();
        if parsed.is_none() {
            self.error(value.span, "expected 16 lowercase hex digits");
        }
        parsed
    }
    fn int_literal(&mut self, value: &Value) -> Option<IntLiteral> {
        let parsed = parse_int_literal(self.atom(value)?);
        if parsed.is_none() {
            self.error(value.span, "expected integer literal");
        }
        parsed
    }
    fn float_literal(&mut self, value: &Value) -> Option<FloatLiteral> {
        let parsed = FloatLiteral::parse_exact(self.atom(value)?);
        if parsed.is_none() {
            self.error(value.span, "expected exact float literal");
        }
        parsed
    }
    fn mark(&mut self, path: impl Into<String>, span: (usize, usize)) {
        self.source_map.entries.insert(path.into(), span);
    }
    fn error(&mut self, span: (usize, usize), message: impl Into<String>) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(diagnostic(span, message));
        }
    }
}

fn parse_int_literal(value: &str) -> Option<IntLiteral> {
    value.strip_prefix('-').map_or_else(
        || IntLiteral::parse_radix(value, 10),
        |digits| IntLiteral::parse_radix(digits, 10).map(|value| value.neg()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::text::write;

    fn program_with(functions: Vec<(String, MirFunction)>) -> MirProgram {
        MirProgram {
            functions,
            declarations: MirDeclarations::default(),
            invariant_errors: Vec::new(),
        }
    }

    fn function_with(reg_types: Vec<Ty>, instrs: Vec<MirInstr>) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::FallOff,
            }],
            n_regs: reg_types.len() as u32,
            n_vars: 0,
            var_names: Vec::new(),
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: Some(Ty::None),
            raises: false,
            error_ty: None,
            spans: SpanTable(HashMap::new()),
            reg_types: reg_types
                .into_iter()
                .enumerate()
                .map(|(index, ty)| (index as u32, ty))
                .collect(),
        }
    }

    /// Print → parse → print must reproduce the canonical text byte-for-byte.
    fn assert_reprints(program: &MirProgram) {
        let text = write::program(program);
        let parsed =
            artifact(text.as_bytes(), "unit.mir".to_string()).expect("parse canonical artifact");
        assert_eq!(write::program(&parsed.program), text);
    }

    fn diagnostics(text: &str) -> Vec<String> {
        artifact(text.as_bytes(), "unit.mir".to_string())
            .expect_err("expected artifact diagnostics")
            .diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn artifact_with_register_type(ty_text: &str) -> String {
        format!(
            "mojito-mir 1.0\nartifact {{\n  features: [],\n  files: [],\n  structs: [],\n  \
             decls: [],\n  functions: [\n    fn {{\n      name: main,\n      registers: 1,\n      \
             vars: 0,\n      var_names: [],\n      params: 0,\n      param_types: [],\n      \
             owned_params: [],\n      deinit_params: [],\n      ref_params: [],\n      \
             returns_reference: false,\n      var_types: [],\n      return_type: present(None),\n      \
             raises: false,\n      error_type: absent,\n      \
             register_types: [reg_type {{ reg: %r0, type: {ty_text} }}],\n      locations: [],\n      \
             blocks: []\n    }}\n  ]\n}}\n"
        )
    }

    #[test]
    fn type_families_reprint_byte_identically() {
        let capturing = CallableEnvironment::Capturing(CaptureOriginSet::concrete([
            CaptureOrigin {
                origin: Origin::Place(OriginPlace {
                    root: OwnerId(1),
                    path: vec![OriginSeg::Field("data".into()), OriginSeg::AnyIndex],
                }),
                access: CaptureAccess::Write,
            },
            CaptureOrigin {
                origin: Origin::Param(OriginParamId(4)),
                access: CaptureAccess::Read,
            },
        ]));
        let func_ty = Ty::Func {
            environment: capturing,
            params: vec![Ty::Int],
            names: vec!["value".into()],
            ret: Box::new(Ty::Bool),
            required: vec![true],
            variadic: Some(Box::new(Ty::Int)),
            kw_variadic: None,
            positional_only: Some(0),
            keyword_only: None,
            raises: true,
            error: Some(Box::new(Ty::Error)),
            conventions: vec![Some(crate::ast::ArgConvention::Mut)],
            ref_params: Box::new(vec![Some(RefSig {
                origin: SigOrigin::Projected(
                    Box::new(SigOrigin::Param(0)),
                    vec![OriginSeg::Interior("element".into())],
                ),
                mutability: SigMutability::BoolParam(1),
            })]),
            ref_return: Some(Box::new(RefSig {
                origin: SigOrigin::Union(vec![SigOrigin::Self_, SigOrigin::Static]),
                mutability: SigMutability::Infer,
            })),
            transfers: TransferSet(vec![TransferEffect {
                dest: SigOrigin::Param(1),
                src: SigOrigin::Bound(Origin::Static),
                src_is_place: true,
                mutable: false,
            }]),
        };
        let generic_ty = Ty::GenericFunc {
            environment: CallableEnvironment::Thin,
            decls: vec![
                ParamDecl::Type {
                    name: "T".into(),
                    bounds: vec!["Copyable".into()],
                    callable_bound: None,
                    default: Some(Box::new(Ty::Int)),
                    infer_only: true,
                    variadic: false,
                    constraints: vec![
                        GenericConstraint::WithMessage(
                            Box::new(GenericConstraint::And(
                                Box::new(GenericConstraint::Conforms {
                                    param: "T".into(),
                                    trait_name: "Movable".into(),
                                }),
                                Box::new(GenericConstraint::Trivial(
                                    TrivialLifecycle::Copyable,
                                    ConstraintOperand::Param("T".into()),
                                )),
                            )),
                            "T must copy".into(),
                        ),
                        GenericConstraint::Or(
                            Box::new(GenericConstraint::Not(Box::new(GenericConstraint::Bool(
                                false,
                            )))),
                            Box::new(GenericConstraint::ConformsPack {
                                param: "Ts".into(),
                                trait_name: "Movable".into(),
                            }),
                        ),
                        GenericConstraint::PackPredicate {
                            param: "Ts".into(),
                            predicate: PackPredicateRef::Alias("IsNice".into()),
                            all: true,
                        },
                        GenericConstraint::PackPredicate {
                            param: "Ts".into(),
                            predicate: PackPredicateRef::Trivial(TrivialLifecycle::Deinitable),
                            all: false,
                        },
                        GenericConstraint::PackContains {
                            param: "Ts".into(),
                            element: ConstraintOperand::Type(Ty::Int),
                        },
                        GenericConstraint::Le(
                            ConstraintOperand::Value(CtValue::Int(3)),
                            ConstraintOperand::PackLength("Ts".into()),
                        ),
                        GenericConstraint::Ne(
                            ConstraintOperand::Param("T".into()),
                            ConstraintOperand::Type(Ty::Bool),
                        ),
                    ],
                },
                ParamDecl::Value {
                    name: "width".into(),
                    ty: Box::new(Ty::Int),
                    default: Some(CtExpr::Add(
                        Box::new(CtExpr::Param("n".into())),
                        Box::new(CtExpr::Value(CtValue::Int(1))),
                    )),
                    callable_default: Some(CallableDefault::If {
                        condition: CtExpr::Value(CtValue::Bool(true)),
                        then_value: Box::new(CallableDefault::Symbol("default_fn".into())),
                        else_value: Box::new(CallableDefault::Parameter("F".into())),
                    }),
                    infer_only: false,
                    variadic: true,
                    constraints: Vec::new(),
                },
            ],
            params: Vec::new(),
            names: Vec::new(),
            ret: Box::new(Ty::None),
            required: Vec::new(),
            variadic: None,
            kw_variadic: Some(Box::new(Ty::Int)),
            positional_only: None,
            keyword_only: Some(0),
            raises: false,
            error: None,
            conventions: Vec::new(),
            ref_params: Box::new(vec![None]),
            ref_return: None,
            transfers: TransferSet(Vec::new()),
        };
        let struct_ty = Ty::Struct(
            "Fancy".into(),
            vec![
                TyArg::Ty(Ty::Tuple(vec![Ty::Int, Ty::Bool])),
                TyArg::Val(CtValue::Struct {
                    name: "Layout".into(),
                    fields: vec![
                        (
                            "shape".into(),
                            CtValue::Tuple(vec![CtValue::Int(2), CtValue::UInt(3)]),
                        ),
                        ("tag".into(), CtValue::Str("row".into())),
                    ],
                }),
                TyArg::Val(CtValue::List(vec![
                    CtValue::Float(0x3ff0000000000000),
                    CtValue::IntLiteral(parse_int_literal("-12345678901234567890").unwrap()),
                    CtValue::FloatLiteral(FloatLiteral::parse_exact("157/50").unwrap()),
                    CtValue::Dtype(Dtype::Float32),
                    CtValue::Type(Box::new(Ty::Int)),
                    CtValue::Reflected(Box::new(Ty::Bool)),
                    CtValue::Param("N".into()),
                ])),
                TyArg::Origin(Origin::union([
                    Origin::Param(OriginParamId(0)),
                    Origin::SelfParam,
                ])),
            ],
        );
        let program = program_with(vec![(
            "types".into(),
            function_with(
                vec![
                    Ty::Overload(vec![Ty::Int, Ty::Bool]),
                    Ty::Param {
                        name: "T".into(),
                        bounds: vec!["Copyable".into(), "Movable".into()],
                        callable_bound: Some(Box::new(func_ty.clone())),
                    },
                    Ty::Assoc {
                        base: Box::new(Ty::Param {
                            name: "C".into(),
                            bounds: vec!["Iterable".into()],
                            callable_bound: None,
                        }),
                        name: "IteratorType".into(),
                        args: vec![TyArg::Origin(Origin::SelfParam)],
                    },
                    Ty::Dependent(DependentType::Indexed {
                        elements: vec![Ty::Int, Ty::Bool],
                        index: CtExpr::FloorDiv(
                            Box::new(CtExpr::Neg(Box::new(CtExpr::Param("i".into())))),
                            Box::new(CtExpr::Value(CtValue::Int(2))),
                        ),
                    }),
                    func_ty,
                    generic_ty,
                    struct_ty,
                    Ty::Simd {
                        dtype: Dtype::Float32,
                        width: 4,
                    },
                    Ty::ComptimeList(Box::new(Ty::Int)),
                    Ty::RuntimePack(vec![Ty::Int, Ty::Bool]),
                    Ty::VariadicPack(Box::new(Ty::Int)),
                    Ty::Variant(vec![Ty::Int, Ty::None]),
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::Place {
                            place: OriginPlace {
                                root: OwnerId(0),
                                path: vec![OriginSeg::Subtree],
                            },
                            mutable: true,
                        },
                    },
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::Param {
                            id: OriginParamId(2),
                            mutability: Mutability::Param(OriginParamId(3)),
                            interior: vec!["element".into()],
                            subtree: true,
                        },
                    },
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::SelfPlace {
                            mutability: Mutability::Immutable,
                            interior: Vec::new(),
                            subtree: false,
                        },
                    },
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::Static,
                    },
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::Untracked { mutable: false },
                    },
                    Ty::Pointer {
                        element: Box::new(Ty::Int),
                        origin: PointerOrigin::UnsafeAny { mutable: true },
                    },
                    Ty::Ref(RefTy {
                        referent: Box::new(Ty::Struct("List".into(), vec![TyArg::Ty(Ty::Int)])),
                        origin: Origin::Untracked { mutable: true },
                        mutability: Mutability::Mutable,
                    }),
                ],
                Vec::new(),
            ),
        )]);
        assert_reprints(&program);
    }

    #[test]
    fn constant_families_reprint_byte_identically() {
        let constants = vec![
            Const::Int(i64::MIN),
            Const::Float(-0.0),
            Const::IntLiteral(parse_int_literal("-123456789012345678901234567890").unwrap()),
            Const::FloatLiteral(FloatLiteral::parse_exact("-1/3").unwrap()),
            Const::FloatLiteral(FloatLiteral::parse_exact("-0.0").unwrap()),
            Const::FloatLiteral(FloatLiteral::parse_exact("42.0").unwrap()),
            Const::Bool(true),
            Const::Str("line\n\"quoted\"\t\u{7}".into()),
            Const::Function("needs quoting!".into()),
            Const::None,
        ];
        let reg_types = vec![Ty::Int; constants.len()];
        let instrs = constants
            .into_iter()
            .enumerate()
            .map(|(index, k)| MirInstr::Const {
                dest: Reg(index as u32),
                k,
            })
            .collect();
        let program = program_with(vec![("consts".into(), function_with(reg_types, instrs))]);
        assert_reprints(&program);
    }

    #[test]
    fn unknown_value_grammar_tags_are_diagnosed() {
        assert!(
            diagnostics(&artifact_with_register_type("Frob"))
                .iter()
                .any(|message| message.contains("unknown type `Frob`"))
        );
        assert!(
            diagnostics(&artifact_with_register_type(
                "simd { dtype: int99, width: 4 }"
            ))
            .iter()
            .any(|message| message.contains("unknown dtype `int99`"))
        );
        assert!(
            diagnostics(&artifact_with_register_type("simd { dtype: int }"))
                .iter()
                .any(|message| message.contains("missing required field `width`"))
        );
        assert!(
            diagnostics(&artifact_with_register_type(
                "struct_type { name: Box, arguments: [value_arg(ct_float_literal(1/0))] }"
            ))
            .iter()
            .any(|message| message.contains("expected exact float literal"))
        );
        assert!(
            diagnostics(&artifact_with_register_type(
                "param { name: T, bounds: [], callable_bound: absent, extra: 1 }"
            ))
            .iter()
            .any(|message| message.contains("unknown field `extra`"))
        );
    }

    fn sample_place(root: u32) -> MirPlace {
        MirPlace {
            root,
            root_ty: Some(Ty::Struct("List".into(), vec![TyArg::Ty(Ty::Int)])),
            proj: vec![
                Proj::Field("data".into()),
                Proj::Index(Reg(7)),
                Proj::ConstIndex(1),
                Proj::Variant(0),
                Proj::UninitPayload,
            ],
            projection_tys: vec![Ty::Int, Ty::Int, Ty::Int, Ty::Int, Ty::Int],
            ty: Some(Ty::Int),
            through: Some(9),
        }
    }

    fn sample_subscript_call() -> MirSubscriptCall {
        MirSubscriptCall {
            target: "List::__getitem__".into(),
            raises: Some(Ty::Error),
            result_ty: Ty::Int,
            receiver_requires_place: true,
            receiver_convention: Some(ArgConvention::Mut),
            arguments: vec![
                CheckedCallArgument {
                    source: CheckedCallArgumentSource::Positional(0),
                    parameter_ty: Ty::Int,
                    requires_place: false,
                    convention: None,
                },
                CheckedCallArgument {
                    source: CheckedCallArgumentSource::Keyword(1),
                    parameter_ty: Ty::Bool,
                    requires_place: true,
                    convention: Some(ArgConvention::Ref),
                },
                CheckedCallArgument {
                    source: CheckedCallArgumentSource::Default,
                    parameter_ty: Ty::Int,
                    requires_place: false,
                    convention: Some(ArgConvention::Imm),
                },
            ],
            capture_accesses: vec![MirCaptureAccess {
                root: 3,
                path: vec![OriginSeg::Field("buffer".into()), OriginSeg::AnyIndex],
                access: CaptureAccess::Write,
            }],
            reference_result: Some(RefTy {
                referent: Box::new(Ty::Int),
                origin: Origin::Place(OriginPlace {
                    root: OwnerId(3),
                    path: vec![OriginSeg::Interior("element".into())],
                }),
                mutability: Mutability::Mutable,
            }),
            param_arg_regs: vec![
                MirParamArg {
                    name: Some("T".into()),
                    value: None,
                },
                MirParamArg {
                    name: None,
                    value: Some(Reg(5)),
                },
            ],
            param_decls: vec![ParamDecl::Type {
                name: "T".into(),
                bounds: vec!["Copyable".into()],
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
        }
    }

    fn sample_iterator_call() -> CheckedIteratorCall {
        CheckedIteratorCall {
            target: "_ListIter::__next__".into(),
            result_ty: Ty::Int,
            reference_result: Some(RefTy {
                referent: Box::new(Ty::Int),
                origin: Origin::SelfParam,
                mutability: Mutability::Immutable,
            }),
            raises: Some(Ty::Error),
            result_adapter: Some(CheckedResultAdapter::CopyIteratorReference),
        }
    }

    #[test]
    fn instruction_families_reprint_byte_identically() {
        let place = sample_place;
        let interior = |root: u32| MirInteriorOrigin {
            root,
            path: vec![OriginSeg::Interior("element".into()), OriginSeg::Subtree],
        };
        let instrs = vec![
            MirInstr::EstablishLoans {
                reference: 0,
                loans: vec![
                    MirLoan {
                        place: place(1),
                        mutable: true,
                        interior: Some(interior(1)),
                    },
                    MirLoan {
                        place: place(2),
                        mutable: false,
                        interior: None,
                    },
                ],
                marker: Reg(0),
                dest_interior: Some(interior(0)),
            },
            MirInstr::InvalidateInteriors {
                base: interior(2),
                except: Some(3),
                include_base_generation: true,
                marker: Reg(1),
            },
            MirInstr::MakeRef {
                dest: Reg(2),
                place: place(0),
            },
            MirInstr::ReadRef {
                dest: Reg(3),
                reference: Reg(2),
            },
            MirInstr::CopyValue {
                dest: Reg(4),
                value: Reg(3),
            },
            MirInstr::WriteRef {
                reference: Reg(2),
                value: Reg(4),
            },
            MirInstr::MakeClosure {
                dest: Reg(5),
                function: "outer::lambda#1".into(),
                captures: vec![
                    MirClosureCapture {
                        place: place(0),
                        mode: MirCaptureMode::Reference,
                    },
                    MirClosureCapture {
                        place: place(1),
                        mode: MirCaptureMode::Copy,
                    },
                    MirClosureCapture {
                        place: place(2),
                        mode: MirCaptureMode::Move,
                    },
                ],
            },
            MirInstr::KeepAlive { var: 1 },
            MirInstr::MovePlace {
                dest: Reg(6),
                place: place(1),
            },
            MirInstr::DefVar {
                var: 2,
                src: Reg(6),
                binding_ty: Some(Ty::Int),
            },
            MirInstr::DefVar {
                var: 2,
                src: Reg(6),
                binding_ty: None,
            },
            MirInstr::UnOp {
                op: PrefixOp::Neg,
                dest: Reg(7),
                a: Reg(6),
            },
            MirInstr::UnOp {
                op: PrefixOp::Not,
                dest: Reg(8),
                a: Reg(7),
            },
            MirInstr::BinOp {
                op: InfixOp::FloorDiv,
                dest: Reg(9),
                a: Reg(7),
                b: Reg(8),
                resolved: Some("Tuple::__contains__".into()),
            },
            MirInstr::Call {
                dest: Reg(10),
                func: FuncRef("std::print".into()),
                raises: Some(Ty::Error),
                args: vec![Reg(1), Reg(2)],
                kwargs: vec![("sep".into(), Reg(3))],
                arg_places: vec![None, Some(place(1))],
                kwarg_places: vec![Some(place(2))],
                capture_accesses: vec![MirCaptureAccess {
                    root: 0,
                    path: Vec::new(),
                    access: CaptureAccess::Read,
                }],
                param_arg_regs: vec![MirParamArg {
                    name: Some("T".into()),
                    value: Some(Reg(4)),
                }],
            },
            MirInstr::CallIndirect {
                dest: Reg(11),
                callee: Reg(5),
                resolved: Some("Adder::__call__".into()),
                raises: None,
                args: vec![Reg(1)],
                kwargs: Vec::new(),
                callee_place: Some(place(3)),
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: vec![ParamDecl::Value {
                    name: "n".into(),
                    ty: Box::new(Ty::Int),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                }],
                instantiated_contract: Some(Ty::Func {
                    environment: CallableEnvironment::Default,
                    params: vec![Ty::Int],
                    names: vec!["x".into()],
                    ret: Box::new(Ty::Int),
                    required: vec![true],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    error: None,
                    conventions: vec![None],
                    ref_params: Box::new(vec![None]),
                    ref_return: None,
                    transfers: TransferSet(Vec::new()),
                }),
                instantiated_args: vec![TyArg::Ty(Ty::Int)],
            },
            MirInstr::MethodCall {
                dest: Reg(12),
                recv: Reg(0),
                method: "append".into(),
                resolved: Some("List::append".into()),
                raises: None,
                reference_result: Some(RefTy {
                    referent: Box::new(Ty::Int),
                    origin: Origin::Static,
                    mutability: Mutability::Param(OriginParamId(0)),
                }),
                result_adapter: Some(CheckedResultAdapter::CopyIteratorReference),
                args: vec![Reg(4)],
                kwargs: vec![("count".into(), Reg(5))],
                recv_place: Some(place(0)),
                recv_writes: true,
                arg_places: vec![Some(place(1))],
                kwarg_places: vec![None],
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            },
            MirInstr::PointerStorageTake {
                dest: Reg(13),
                pointer: Reg(1),
                index: Reg(2),
                element: Ty::Int,
            },
            MirInstr::PointerStorageDestroy {
                dest: Reg(14),
                pointer: Reg(1),
                index: Reg(2),
                element: Ty::Int,
            },
            MirInstr::UninitStorage {
                dest: Reg(15),
                init: Some(Reg(3)),
            },
            MirInstr::UninitStorage {
                dest: Reg(16),
                init: None,
            },
            MirInstr::UninitStorageTake {
                dest: Reg(17),
                storage: Reg(15),
                element: Ty::Int,
            },
            MirInstr::UninitStorageDestroy {
                dest: Reg(18),
                storage: Reg(15),
                element: Ty::Int,
            },
            MirInstr::GetField {
                dest: Reg(19),
                base: Reg(0),
                field: "size".into(),
            },
            MirInstr::Index {
                dest: Reg(20),
                base: Reg(0),
                index: Reg(1),
                base_place: Some(place(0)),
                index_place: None,
                call: Some(sample_subscript_call()),
                intrinsic: None,
            },
            MirInstr::Index {
                dest: Reg(21),
                base: Reg(0),
                index: Reg(1),
                base_place: None,
                index_place: None,
                call: None,
                intrinsic: Some(MirIntrinsicSubscript::Pointer),
            },
            MirInstr::Slice {
                dest: Reg(22),
                object: Reg(0),
                kind: SliceKind::StridedSlice,
                lower: Some(Reg(1)),
                upper: None,
                step: Some(Reg(2)),
                object_place: Some(place(0)),
                arg_places: vec![None, Some(place(1))],
                call: Some(sample_subscript_call()),
                intrinsic: Some(MirIntrinsicSubscript::Simd),
            },
            MirInstr::MultiIndex {
                dest: Reg(23),
                object: Reg(0),
                args: vec![
                    MirSubscriptArg::Index(Reg(1)),
                    MirSubscriptArg::Slice {
                        kind: SliceKind::ContiguousSlice,
                        lower: Some(Reg(2)),
                        upper: Some(Reg(3)),
                        step: None,
                    },
                ],
                object_place: Some(place(0)),
                arg_places: vec![None],
                kwargs: vec![("byte".into(), MirSubscriptArg::Index(Reg(4)))],
                kwarg_places: vec![Some(place(1))],
                call: Some(sample_subscript_call()),
            },
            MirInstr::MultiSet {
                receiver: Reg(0),
                receiver_place: Some(place(0)),
                args: vec![MirSubscriptArg::Index(Reg(1))],
                arg_places: vec![None],
                value: Reg(2),
                value_place: Some(place(2)),
                value_keyword: true,
                call: sample_subscript_call(),
            },
            MirInstr::Store {
                place: place(0),
                src: Reg(1),
            },
            MirInstr::StoreRef {
                place: place(0),
                reference: Reg(2),
            },
            MirInstr::LoadPlace {
                dest: Reg(24),
                place: place(0),
            },
            MirInstr::MakeTuple {
                dest: Reg(25),
                elems: vec![Reg(1), Reg(2)],
                element_types: Some(vec![Ty::Int, Ty::Bool]),
            },
            MirInstr::MakeTuple {
                dest: Reg(26),
                elems: Vec::new(),
                element_types: None,
            },
            MirInstr::MakeVariant {
                dest: Reg(27),
                alternatives: vec![Ty::Int, Ty::None],
                index: 1,
                value: Reg(1),
            },
            MirInstr::VariantIs {
                dest: Reg(28),
                variant: Reg(27),
                index: 0,
            },
            MirInstr::VariantGet {
                dest: Reg(29),
                variant: Reg(27),
                index: 1,
            },
            MirInstr::VariantSet {
                dest: Reg(30),
                place: place(0),
                index: 0,
                value: Reg(1),
            },
            MirInstr::VariantTake {
                dest: Reg(31),
                variant: Reg(27),
                index: 1,
                checked: true,
            },
            MirInstr::VariantSetInitWith {
                dest: Reg(32),
                place: place(0),
                index: 0,
                factory: Reg(5),
            },
            MirInstr::VariantDeinitWith {
                dest: Reg(33),
                variant: Reg(27),
                handler: Reg(5),
                index: 1,
            },
            MirInstr::VariantReplace {
                dest: Reg(34),
                place: place(0),
                input_index: 0,
                output_index: 1,
                value: Reg(1),
                checked: false,
            },
            MirInstr::MakeSimd {
                dest: Reg(35),
                dtype: Dtype::Float32,
                width: 4,
                elems: vec![Reg(1), Reg(2), Reg(3), Reg(4)],
            },
            MirInstr::SimdCast {
                dest: Reg(36),
                value: Reg(35),
                dtype: Dtype::Int32,
                width: 4,
            },
            MirInstr::SimdShuffle {
                dest: Reg(37),
                value: Reg(35),
                mask: vec![3, 1, 2, 0],
            },
            MirInstr::Raise { src: Reg(1) },
            MirInstr::Drop { reg: Reg(2) },
            MirInstr::DropVar { var: 1 },
            MirInstr::ConsumeVar { var: 2 },
            MirInstr::ConsumePlace {
                place: place(0),
                marker: Reg(3),
            },
            MirInstr::Unsupported("no lowering for frobnication".into()),
            MirInstr::GetIter {
                source: 0,
                dest: 1,
                mode: IterationMode::Borrowed,
                prepare: vec!["__iter__".into()],
            },
            MirInstr::GetIter {
                source: 0,
                dest: 2,
                mode: IterationMode::Owned,
                prepare: Vec::new(),
            },
            MirInstr::HasNext {
                dest: Reg(38),
                iter: 1,
                method: Some("__has_next__".into()),
            },
            MirInstr::Next {
                dest: Reg(39),
                iter: 1,
                call: Some(sample_iterator_call()),
            },
            MirInstr::Next {
                dest: Reg(40),
                iter: 1,
                call: None,
            },
            MirInstr::TryNext {
                dest: Reg(41),
                yielded: Reg(42),
                iter: 1,
                call: sample_iterator_call(),
                exhaustion: Ty::Struct("StopIteration".into(), Vec::new()),
            },
        ];
        let mut function = function_with(vec![Ty::Int; 43], instrs);
        function.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::ReturnWithCleanup {
                value: Some(Reg(0)),
                cleanup: vec![1, 2],
            },
        });
        function.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Branch {
                cond: Reg(0),
                then_b: 0,
                else_b: 1,
            },
        });
        function.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Jump(0),
        });
        function.blocks.push(MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Return(None),
        });
        let program = program_with(vec![("instructions".into(), function)]);
        assert_reprints(&program);
    }

    #[test]
    fn nested_try_regions_reprint_without_region_source_marks() {
        let region_block = |term: MirTerm| MirBlock {
            instrs: vec![MirInstr::KeepAlive { var: 0 }],
            term,
        };
        let inner_try = MirInstr::Try {
            body: vec![region_block(MirTerm::EscapeJump {
                target: 0,
                cleanup: vec![1],
            })],
            handler: None,
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        };
        let outer_try = MirInstr::Try {
            body: vec![
                MirBlock {
                    instrs: vec![inner_try],
                    term: MirTerm::Jump(1),
                },
                region_block(MirTerm::FallOff),
            ],
            handler: Some((Some(3), vec![region_block(MirTerm::FallOff)])),
            orelse: Some(vec![region_block(MirTerm::FallOff)]),
            finalbody: Some(vec![region_block(MirTerm::FallOff)]),
            cleanup: vec![4, 5],
        };
        let program = program_with(vec![(
            "regions".into(),
            function_with(Vec::new(), vec![outer_try]),
        )]);
        let text = write::program(&program);
        let parsed =
            artifact(text.as_bytes(), "unit.mir".to_string()).expect("parse canonical artifact");
        assert_eq!(write::program(&parsed.program), text);
        // Region-local blocks stay out of the source map: the enclosing
        // instruction path brackets them and the canonical verifier resolves
        // only function-level block paths.
        let paths: Vec<&str> = parsed.source_map.iter().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            [
                "artifact",
                "function/regions",
                "function/regions/bb0",
                "function/regions/bb0/instruction/0",
                "function/regions/bb0/terminator",
            ]
        );
    }

    fn artifact_with_instruction(instruction_text: &str) -> String {
        format!(
            "mojito-mir 1.0\nartifact {{\n  features: [],\n  files: [],\n  structs: [],\n  \
             decls: [],\n  functions: [\n    fn {{\n      name: main,\n      registers: 1,\n      \
             vars: 0,\n      var_names: [],\n      params: 0,\n      param_types: [],\n      \
             owned_params: [],\n      deinit_params: [],\n      ref_params: [],\n      \
             returns_reference: false,\n      var_types: [],\n      return_type: present(None),\n      \
             raises: false,\n      error_type: absent,\n      register_types: [],\n      \
             locations: [],\n      blocks: [bb0 {{ instructions: [{instruction_text}], \
             terminator: falloff {{}} }}]\n    }}\n  ]\n}}\n"
        )
    }

    #[test]
    fn malformed_instruction_payloads_are_diagnosed() {
        assert!(
            diagnostics(&artifact_with_instruction("ref.make { dest: %r0 }"))
                .iter()
                .any(|message| message.contains("missing required field `place`"))
        );
        assert!(
            diagnostics(&artifact_with_instruction("frob.nicate { dest: %r0 }"))
                .iter()
                .any(|message| message.contains("unknown instruction `frob.nicate`"))
        );
        assert!(
            diagnostics(&artifact_with_instruction(
                "try { body: [bb1 { instructions: [], terminator: falloff {} }], \
                 handler: absent, orelse: absent, finalbody: absent, cleanup: [] }"
            ))
            .iter()
            .any(|message| message.contains("expected `bb0` record, found `bb1`"))
        );
        assert!(
            diagnostics(&artifact_with_instruction(
                "index.multi { dest: %r0, object: %r0, args: [slice_arg { kind: sideways, \
                 lower: absent, upper: absent, step: absent }], object_place: absent, \
                 arg_places: [], kwargs: [], kwarg_places: [], call: absent }"
            ))
            .iter()
            .any(|message| message.contains("unknown slice kind `sideways`"))
        );
    }

    #[test]
    fn declaration_metadata_reprints_byte_identically() {
        let boxed = MirStructDeclaration {
            name: "Box".into(),
            fields: vec![("value".into(), Ty::Int), ("tag".into(), Ty::Bool)],
            mut_self_methods: HashSet::from(["append".into(), "clear".into()]),
            fieldwise_init: true,
            param_decls: vec![ParamDecl::Type {
                name: "T".into(),
                bounds: vec!["Copyable".into()],
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            explicit_destroy_message: Some("explicit destroy required".into()),
            explicit_destructors: HashMap::from([
                ("_finish".into(), true),
                ("__del__".into(), false),
            ]),
        };
        let zebra = MirStructDeclaration {
            name: "Zebra needs quoting!".into(),
            fields: Vec::new(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: Vec::new(),
            explicit_destroy_message: None,
            explicit_destructors: HashMap::new(),
        };
        let add = MirFunctionDeclaration {
            lowered_name: "add".into(),
            param_names: vec!["lhs".into(), "rhs".into(), "scale".into()],
            param_types: vec![Ty::Int, Ty::Int, Ty::Float64],
            defaults: vec![
                None,
                Some(CheckedConst::Int(parse_int_literal("-7").unwrap())),
                Some(CheckedConst::Float(
                    FloatLiteral::parse_exact("157/50").unwrap(),
                )),
            ],
            required: vec![true, false, false],
            variadic: Some(Ty::Int),
            variadic_convention: Some(ArgConvention::Var),
            variadic_index: Some(3),
            kw_variadic: Some(Ty::Bool),
            kw_variadic_convention: Some(ArgConvention::Imm),
            kw_variadic_index: Some(4),
            positional_only: Some(1),
            keyword_only: Some(2),
            param_decls: vec![ParamDecl::Value {
                name: "n".into(),
                ty: Box::new(Ty::Int),
                default: Some(CtExpr::Value(CtValue::Int(2))),
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            has_receiver: true,
            receiver_convention: Some(ArgConvention::Mut),
            param_conventions: vec![None, Some(ArgConvention::Imm), Some(ArgConvention::Ref)],
            ret_ty: Ty::Int,
            returns_reference: true,
            raises: true,
            error_ty: Some(Ty::Error),
            ref_params: vec![false, false, true],
        };
        let other = MirFunctionDeclaration {
            lowered_name: "aaa_first".into(),
            param_names: vec!["flag".into(), "nothing".into()],
            param_types: vec![Ty::Bool, Ty::None],
            defaults: vec![Some(CheckedConst::Bool(true)), Some(CheckedConst::None)],
            required: vec![false, false],
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            param_decls: Vec::new(),
            has_receiver: false,
            receiver_convention: None,
            param_conventions: vec![None, None],
            ret_ty: Ty::None,
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: vec![false, false],
        };
        let mut program =
            program_with(vec![("main".into(), function_with(Vec::new(), Vec::new()))]);
        // Deliberately unsorted: the canonical writer sorts by name, so the
        // reprint equality also proves parse keeps the sorted order.
        program.declarations = MirDeclarations {
            structs: vec![zebra, boxed],
            functions: vec![add, other],
        };
        assert_reprints(&program);
    }

    #[test]
    fn malformed_declaration_metadata_is_diagnosed() {
        let text = "mojito-mir 1.0\nartifact { features: [], files: [], structs: [struct { \
                    name: Box, fields: [], mut_self_methods: [], fieldwise_init: false, \
                    param_decls: [], explicit_destroy_message: absent, explicit_destructors: \
                    [destructor { name: f, raises: true }, destructor { name: f, raises: \
                    false }] }], decls: [decl { lowered_name: add }], functions: [] }\n";
        let messages = diagnostics(text);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("duplicate destructor `f`"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("missing required field `param_names`"))
        );
    }

    #[test]
    fn func_types_reject_param_decls() {
        let text = artifact_with_register_type(
            "func { environment: default, param_decls: [type_param { name: T, bounds: [], \
             callable_bound: absent, default: absent, infer_only: false, variadic: false, \
             constraints: [] }], params: [], names: [], return_type: None, required: [], \
             variadic: absent, kw_variadic: absent, positional_only: absent, keyword_only: \
             absent, raises: false, error_type: absent, conventions: [], ref_params: [], \
             ref_return: absent, transfers: [] }",
        );
        assert!(
            diagnostics(&text)
                .iter()
                .any(|message| message.contains("`func` types take no param_decls"))
        );
    }
}
