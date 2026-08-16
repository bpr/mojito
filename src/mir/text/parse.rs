use super::{ArtifactDiagnostic, ArtifactReport, ArtifactSourceMap, ParsedArtifact};
use crate::literal::IntLiteral;
use crate::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirInstr, MirProgram, MirTerm, Reg, SpanTable,
    UseMode,
};
use crate::token::SourceSpan;
use crate::types::Ty;
use std::collections::{BTreeMap, HashMap};

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
        self.empty_list(features, "features");
        let Ok(files) = self.field(fields, "files") else {
            self.error(value.span, "missing required field `files`");
            return Err(self.diagnostics);
        };
        self.decode_files(files);
        if let Ok(structs) = self.field(fields, "structs") {
            self.empty_list(structs, "structs");
        }
        if let Ok(decls) = self.field(fields, "decls") {
            self.empty_list(decls, "decls");
        }
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
                    declarations: MirDeclarations::default(),
                    invariant_errors: Vec::new(),
                },
                self.source_map,
            ))
        } else {
            Err(self.diagnostics)
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
        let get = |name| {
            fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| &field.value)
        };
        match tag {
            "var.use" => Some(MirInstr::UseVar {
                dest: Reg(self.identity(get("dest"), "%r")? as u32),
                var: self.identity(get("var"), "$v")? as u32,
                mode: match self.atom(get("mode")?)? {
                    "copy" => UseMode::Copy,
                    "move" => UseMode::Move,
                    "borrow_shared" => UseMode::BorrowShared,
                    "borrow_mut" => UseMode::BorrowMut,
                    other => {
                        self.error(value.span, format!("unknown use mode `{other}`"));
                        return None;
                    }
                },
            }),
            "const" => Some(MirInstr::Const {
                dest: Reg(self.identity(get("dest"), "%r")? as u32),
                k: self.constant(get("value")?)?,
            }),
            "literal.materialize" => Some(MirInstr::MaterializeLiteral {
                dest: Reg(self.identity(get("dest"), "%r")? as u32),
                value: Reg(self.identity(get("value"), "%r")? as u32),
                target: self.ty(get("target")?)?,
            }),
            other => {
                self.error(value.span, format!("unsupported instruction `{other}`"));
                None
            }
        }
    }

    fn term(&mut self, value: &Value) -> Option<MirTerm> {
        let (tag, fields) = self.any_record(value)?;
        match tag {
            "jump" => Some(MirTerm::Jump(
                self.identity(self.field(fields, "target").ok(), "bb")?,
            )),
            "branch" => Some(MirTerm::Branch {
                cond: Reg(self.identity(self.field(fields, "condition").ok(), "%r")? as u32),
                then_b: self.identity(self.field(fields, "then").ok(), "bb")?,
                else_b: self.identity(self.field(fields, "else").ok(), "bb")?,
            }),
            "return" => Some(MirTerm::Return(
                self.option_identity(self.field(fields, "value").ok(), "%r")
                    .map(|v| Reg(v as u32)),
            )),
            "falloff" => Some(MirTerm::FallOff),
            other => {
                self.error(value.span, format!("unsupported terminator `{other}`"));
                None
            }
        }
    }

    fn constant(&mut self, value: &Value) -> Option<Const> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "none" => Some(Const::None),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "int" => self.atom(inner)?.parse().ok().map(Const::Int),
                "int_literal" => parse_int_literal(self.atom(inner)?).map(Const::IntLiteral),
                "float" => u64::from_str_radix(self.atom(inner)?, 16)
                    .ok()
                    .map(|bits| Const::Float(f64::from_bits(bits))),
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
        let atom = self.atom(value)?;
        Some(match atom {
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
                self.error(value.span, format!("unsupported type `{other}`"));
                return None;
            }
        })
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
    fn empty_list(&mut self, value: &Value, name: &str) {
        if self.list(value).is_ok_and(|v| !v.is_empty()) {
            self.error(value.span, format!("{name} decoding is not available"));
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
