//! The value-tree reader: tokenizes the textual artifact into nested
//! `Value` records for the decoder.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn header(&mut self) {
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

    pub(super) fn value(&mut self) -> Option<Value> {
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

    pub(super) fn string(&mut self) -> Option<String> {
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

    pub(super) fn atom(&mut self) -> Option<String> {
        self.space();
        let value =
            self.take_while(|byte| !byte.is_ascii_whitespace() && !b"[]{}():,#".contains(&byte));
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    }

    pub(super) fn space(&mut self) {
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
    pub(super) fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }
    pub(super) fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    pub(super) fn take_while(&mut self, predicate: impl Fn(u8) -> bool) -> &'a str {
        let start = self.pos;
        while self.peek().is_some_and(&predicate) {
            self.pos += 1;
        }
        &self.source[start..self.pos]
    }
    pub(super) fn error_here(&mut self, message: &str) {
        self.error((self.pos, (self.pos + 1).min(self.source.len())), message);
    }
    pub(super) fn error(&mut self, span: (usize, usize), message: &str) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(diagnostic(span, message));
        }
    }
    pub(super) fn sync(&mut self, closer: u8) {
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
