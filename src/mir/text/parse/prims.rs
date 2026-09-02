//! Primitive field decoding: scalars, lists, records, options, and
//! required-field plumbing.

use super::*;

impl Decoder {
    pub(super) fn dtype(&mut self, value: &Value) -> Option<Dtype> {
        let atom = self.atom(value)?;
        let parsed = Dtype::from_name(atom);
        if parsed.is_none() {
            self.error(value.span, format!("unknown dtype `{atom}`"));
        }
        parsed
    }

    pub(super) fn locations(&mut self, value: &Value) -> SpanTable {
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

    pub(super) fn type_map(
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
    pub(super) fn types(&mut self, value: &Value) -> Vec<Ty> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.ty(v)).collect())
            .unwrap_or_default()
    }
    pub(super) fn strings(&mut self, value: &Value) -> Vec<String> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.symbol(v)).collect())
            .unwrap_or_default()
    }
    pub(super) fn bools(&mut self, value: &Value) -> Vec<bool> {
        self.list(value)
            .map(|v| v.iter().filter_map(|v| self.boolean(v)).collect())
            .unwrap_or_default()
    }
    pub(super) fn option_ty(&mut self, value: Option<&Value>) -> Option<Ty> {
        self.option_value(value).and_then(|v| self.ty(v))
    }
    pub(super) fn option_string(&mut self, value: Option<&Value>) -> Option<String> {
        self.option_value(value).and_then(|v| self.string(v))
    }
    pub(super) fn option_identity(&mut self, value: Option<&Value>, prefix: &str) -> Option<usize> {
        self.option_value(value)
            .and_then(|v| self.identity(Some(v), prefix))
    }
    pub(super) fn option_value<'a>(&mut self, value: Option<&'a Value>) -> Option<&'a Value> {
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
    pub(super) fn list<'a>(&mut self, value: &'a Value) -> Result<&'a [Value], ()> {
        match &value.kind {
            ValueKind::List(v) => Ok(v),
            _ => {
                self.error(value.span, "expected list");
                Err(())
            }
        }
    }
    pub(super) fn record<'a>(
        &mut self,
        value: &'a Value,
        expected: &str,
    ) -> Result<&'a [Field], ()> {
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
    pub(super) fn any_record<'a>(&mut self, value: &'a Value) -> Option<(&'a str, &'a [Field])> {
        match &value.kind {
            ValueKind::Record(tag, fields) => Some((tag, fields)),
            _ => {
                self.error(value.span, "expected record");
                None
            }
        }
    }
    pub(super) fn field<'a>(&self, fields: &'a [Field], name: &str) -> Result<&'a Value, ()> {
        let mut found = fields.iter().filter(|f| f.name == name);
        let Some(first) = found.next() else {
            return Err(());
        };
        Ok(&first.value)
    }
    pub(super) fn unknown(&mut self, fields: &[Field], known: &[&str]) {
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
    pub(super) fn atom<'a>(&mut self, value: &'a Value) -> Option<&'a str> {
        match &value.kind {
            ValueKind::Atom(v) => Some(v),
            _ => {
                self.error(value.span, "expected word or number");
                None
            }
        }
    }
    pub(super) fn string(&mut self, value: &Value) -> Option<String> {
        match &value.kind {
            ValueKind::String(v) => Some(v.clone()),
            _ => {
                self.error(value.span, "expected string");
                None
            }
        }
    }
    pub(super) fn symbol(&mut self, value: &Value) -> Option<String> {
        match &value.kind {
            ValueKind::Atom(v) | ValueKind::String(v) => Some(v.clone()),
            _ => {
                self.error(value.span, "expected symbol");
                None
            }
        }
    }
    pub(super) fn uint(&mut self, value: &Value) -> Option<usize> {
        self.atom(value)?.parse().ok().or_else(|| {
            self.error(value.span, "expected unsigned integer");
            None
        })
    }
    pub(super) fn boolean(&mut self, value: &Value) -> Option<bool> {
        match self.atom(value)? {
            "true" => Some(true),
            "false" => Some(false),
            _ => {
                self.error(value.span, "expected boolean");
                None
            }
        }
    }
    pub(super) fn identity(&mut self, value: Option<&Value>, prefix: &str) -> Option<usize> {
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
    pub(super) fn required<'a>(
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
    pub(super) fn req<T>(
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
    pub(super) fn positional_value<'a>(
        &mut self,
        value: &'a Value,
    ) -> Option<(&'a str, &'a Value)> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => Some((tag, inner)),
            _ => {
                self.error(value.span, "expected tagged value");
                None
            }
        }
    }
    pub(super) fn option_uint(&mut self, value: &Value) -> Option<usize> {
        self.option_value(Some(value)).and_then(|v| self.uint(v))
    }
    pub(super) fn uint32(&mut self, value: &Value) -> Option<u32> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected unsigned 32-bit integer");
        }
        parsed
    }
    pub(super) fn int64(&mut self, value: &Value) -> Option<i64> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected integer");
        }
        parsed
    }
    pub(super) fn uint64(&mut self, value: &Value) -> Option<u64> {
        let parsed = self.atom(value)?.parse().ok();
        if parsed.is_none() {
            self.error(value.span, "expected unsigned integer");
        }
        parsed
    }
    pub(super) fn float_bits(&mut self, value: &Value) -> Option<u64> {
        let atom = self.atom(value)?;
        let parsed = (atom.len() == 16)
            .then(|| u64::from_str_radix(atom, 16).ok())
            .flatten();
        if parsed.is_none() {
            self.error(value.span, "expected 16 lowercase hex digits");
        }
        parsed
    }
    pub(super) fn int_literal(&mut self, value: &Value) -> Option<IntLiteral> {
        let parsed = parse_int_literal(self.atom(value)?);
        if parsed.is_none() {
            self.error(value.span, "expected integer literal");
        }
        parsed
    }
    pub(super) fn float_literal(&mut self, value: &Value) -> Option<FloatLiteral> {
        let parsed = FloatLiteral::parse_exact(self.atom(value)?);
        if parsed.is_none() {
            self.error(value.span, "expected exact float literal");
        }
        parsed
    }
    pub(super) fn mark(&mut self, path: impl Into<String>, span: (usize, usize)) {
        self.source_map.entries.insert(path.into(), span);
    }
    pub(super) fn error(&mut self, span: (usize, usize), message: impl Into<String>) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(diagnostic(span, message));
        }
    }
}
