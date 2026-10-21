/// A source byte range `(start, end)` — a half-open `[start, end)` slice of
/// the original source.
///
/// This is the single, canonical span type shared by the lexer (which stamps
/// each token), the parser (which propagates spans onto AST nodes), and the
/// MIR (whose `SpanTable` maps each temporary back to its origin span).
pub type Span = (usize, usize);

/// Identity of one concrete syntax occurrence in a checked program.
///
/// Byte spans are provenance, not identity: compile-time elaboration can clone
/// the same source node more than once. The checker validates uniqueness on the
/// final elaborated tree and re-keys every repeated clone before recording facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyntaxId(pub u64);

impl SyntaxId {
    /// Allocate an identity for syntax synthesized outside the final checker
    /// re-keying pass. The high bit keeps these process-local IDs disjoint from
    /// traversal-ordered replacement IDs used for final-tree clones.
    pub fn fresh() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self((1_u64 << 63) | NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Whether this identity came from [`Self::fresh`].
    pub const fn is_fresh(self) -> bool {
        self.0 >> 63 == 1
    }

    /// The identity of the `ordinal`-th node synthesized from the node
    /// `parent`: the same pair always yields the same identity, so a desugar
    /// rebuilt for one statement names the same occurrences each time. The
    /// second-highest bit keeps these disjoint from fresh and re-keyed IDs.
    pub fn derived(parent: Self, ordinal: u32) -> Self {
        let mut derivations = derivations();
        if let Some(id) = derivations.forward.get(&(parent, ordinal)) {
            return *id;
        }
        let id = Self(DERIVED_BIT | derivations.forward.len() as u64);
        derivations.forward.insert((parent, ordinal), id);
        derivations.backward.insert(id, (parent, ordinal));
        id
    }

    /// The parent and ordinal a [`Self::derived`] identity was made from.
    pub fn derivation(self) -> Option<(Self, u32)> {
        if self.0 & (1_u64 << 63 | DERIVED_BIT) != DERIVED_BIT {
            return None;
        }
        derivations().backward.get(&self).copied()
    }
}

/// The bit that marks a [`SyntaxId::derived`] identity.
const DERIVED_BIT: u64 = 1_u64 << 62;

/// Every derived identity handed out, both ways.
#[derive(Default)]
struct Derivations {
    forward: std::collections::HashMap<(SyntaxId, u32), SyntaxId>,
    backward: std::collections::HashMap<SyntaxId, (SyntaxId, u32)>,
}

fn derivations() -> std::sync::MutexGuard<'static, Derivations> {
    static DERIVATIONS: std::sync::OnceLock<std::sync::Mutex<Derivations>> =
        std::sync::OnceLock::new();
    DERIVATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A byte range together with the linked source file it belongs to. Parser-only
/// programs use `None`; the module linker stamps every nested AST node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub source: Option<String>,
    pub span: Span,
    /// Present for an AST node and absent for source-only diagnostic locations.
    /// This makes semantic maps occurrence-safe without changing displayed
    /// source provenance.
    pub syntax: Option<SyntaxId>,
}

impl SourceSpan {
    pub const fn new(source: Option<String>, span: Span) -> Self {
        Self {
            source,
            span,
            syntax: None,
        }
    }

    pub const fn syntax(source: Option<String>, span: Span, syntax: SyntaxId) -> Self {
        Self {
            source,
            span,
            syntax: Some(syntax),
        }
    }

    /// Return diagnostic provenance only. Semantic occurrence identity is a
    /// phase-local lookup discriminator and must not leak into displayed spans,
    /// MIR source maps, or source-only comparisons.
    #[must_use]
    pub const fn without_syntax(mut self) -> Self {
        self.syntax = None;
        self
    }

    /// Compare only the source file and byte range, ignoring which elaborated
    /// syntax occurrence supplied the location.
    pub fn same_provenance(&self, other: &Self) -> bool {
        self.source == other.source && self.span == other.span
    }
}

/// Run `encode` with syntax occurrences encodable, and give back its result
/// with the file table its occurrences name.
///
/// Inside it a [`SyntaxId`] encodes as its derivation, so a decoded one
/// keeps it, over a number local to the encoding: the encoding renumbers
/// each underived identity densely, in the order it first meets them. A
/// source path encodes as its index in the file table. The encoding of a
/// checked body thus keeps which occurrence each of its facts is keyed by.
/// Outside it a [`SourceSpan`] or a [`SyntaxId`] does not encode, since
/// both are local to one compilation.
pub fn encoding_occurrences<R>(encode: impl FnOnce() -> R) -> (R, Vec<Option<String>>) {
    let previous = OCCURRENCES.replace(Some(Occurrences::Encoding {
        files: std::collections::HashMap::new(),
        numbers: std::collections::HashMap::new(),
    }));
    let result = encode();
    let Some(Occurrences::Encoding { files, .. }) = OCCURRENCES.replace(previous) else {
        unreachable!("the occurrence encoding scope was replaced")
    };
    let mut table = vec![None; files.len()];
    for (file, index) in files {
        table[index as usize] = file;
    }
    (result, table)
}

/// Run `decode` over occurrences [`encoding_occurrences`] encoded against
/// `files`.
///
/// Each encoded identity decodes to one fresh identity, the same for every
/// occurrence of it, and a derived one to the derivation of its decoded
/// parent.
pub fn decoding_occurrences<R>(files: Vec<Option<String>>, decode: impl FnOnce() -> R) -> R {
    let previous = OCCURRENCES.replace(Some(Occurrences::Decoding {
        files,
        identities: std::collections::HashMap::new(),
    }));
    let result = decode();
    OCCURRENCES.set(previous);
    result
}

/// Whether occurrences encode or decode here ([`encoding_occurrences`],
/// [`decoding_occurrences`]).
pub fn occurrences_encode() -> bool {
    OCCURRENCES.with_borrow(Option::is_some)
}

/// The index of the source `file` in the file table being encoded.
///
/// # Errors
///
/// When no occurrence encoding is in progress.
pub fn occurrence_file(file: Option<&str>) -> Result<u32, String> {
    OCCURRENCES.with_borrow_mut(|occurrences| match occurrences {
        Some(Occurrences::Encoding { files, .. }) => {
            let next = files.len() as u32;
            Ok(*files.entry(file.map(str::to_string)).or_insert(next))
        }
        _ => Err("a source location is local to one compilation".to_string()),
    })
}

/// The source file at `index` of the file table being decoded.
///
/// # Errors
///
/// When no occurrence decoding is in progress, or the table holds no such
/// file.
pub fn occurrence_source(index: u32) -> Result<Option<String>, String> {
    OCCURRENCES.with_borrow(|occurrences| match occurrences {
        Some(Occurrences::Decoding { files, .. }) => files
            .get(index as usize)
            .cloned()
            .ok_or_else(|| format!("a location names no file {index}")),
        _ => Err("a source location is local to one compilation".to_string()),
    })
}

impl serde::Serialize for SyntaxId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !occurrences_encode() {
            return Err(serde::ser::Error::custom(
                "a syntax identity is local to one compilation",
            ));
        }
        EncodedSyntax::of(*self)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SyntaxId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        EncodedSyntax::deserialize(deserializer)?
            .decode()
            .map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for SourceSpan {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let file = occurrence_file(self.source.as_deref()).map_err(serde::ser::Error::custom)?;
        (file, self.span, self.syntax).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SourceSpan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (file, span, syntax) = <(u32, Span, Option<SyntaxId>)>::deserialize(deserializer)?;
        Ok(Self {
            source: occurrence_source(file).map_err(serde::de::Error::custom)?,
            span,
            syntax,
        })
    }
}

/// An AST node's own [`SyntaxId`] as its encoding carries it.
///
/// Inside an occurrence encoding ([`encoding_occurrences`]) it is the
/// identity; outside one it is nothing, and a decoded node draws a fresh
/// identity, as a parsed one does.
pub mod node_syntax {
    use super::{SyntaxId, occurrences_encode};
    use serde::{Deserialize, Serialize};

    /// # Errors
    ///
    /// As the serializer's.
    pub fn serialize<S: serde::Serializer>(
        id: &SyntaxId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if occurrences_encode() {
            id.serialize(serializer)
        } else {
            serializer.serialize_unit()
        }
    }

    /// # Errors
    ///
    /// As the deserializer's.
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SyntaxId, D::Error> {
        if occurrences_encode() {
            SyntaxId::deserialize(deserializer)
        } else {
            <()>::deserialize(deserializer).map(|()| SyntaxId::fresh())
        }
    }
}

/// The zero-width, position-`0` span used for synthetic nodes that have no source
/// text (e.g. compiler-generated program-entry operations).
pub const DUMMY_SPAN: Span = (0, 0);

/// The token set for the implemented subset of Mojo.
///
/// Keywords are split the way the reference tree-sitter Mojo grammar splits them
/// — into the ones Mojo **shares with Python** and the **Mojo-only** ones — since
/// mojito is a strict subset of Mojo, which is itself (largely) a superset of
/// Python's surface syntax. `Token::keyword` is the single lookup table.
/// One piece of a lexed t-string: either literal text or the raw source text of
/// an interpolation `{…}` (which the parser later parses into an `Expr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TStringChunk {
    Text(String),
    Interp(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    // --- Mojo-only keywords ---
    // Not present in Python: `var`, `struct`, `trait`, `comptime` (Mojo's
    // replacement for `alias`), and the `raises` function-effect keyword.
    // (Further Mojo keywords — `ref`, `out`, `where`, `capturing`, … — are not
    // reserved here; the features that need them aren't implemented, and `mut`
    // stays a soft/contextual word so `mut self` doesn't reserve the name.)
    Var,
    Struct,
    Trait,
    Comptime,
    Raises,

    // --- Keywords shared with Python ---
    Def,
    Lambda,
    Return,
    Pass,
    None,
    And,
    Or,
    Not,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    /// `is` — identity comparison; dispatches to `__is__`/`__isnot__`.
    Is,
    With,
    Break,
    Continue,
    Raise,
    Try,
    Except,
    Finally,
    Import,
    From,
    As,

    // --- Keywords of syntax Mojito does not implement ---
    // `match`/`case` (pattern matching), `class`, `del`, and `yield` are
    // keywords in current Mojo, so none can name a binding or a free function
    // without backticks; a method or member may still be named by one.
    Class,
    Del,
    Match,
    Yield,
    Case,

    // Identifiers (includes type names like `Int`, `String`, `Bool`)
    Identifier(String),

    // Literals: a number or string carries its value, then its source
    // spelling (separators, radix prefix, and quotes intact), which a
    // literal pattern's coverage keys on, as upstream's literal nodes keep
    // their token text. The exact float is boxed, keeping tokens small.
    IntLiteral(crate::literal::IntLiteral, Box<str>),
    FloatLiteral(Box<crate::literal::FloatLiteral>, Box<str>),
    BoolLiteral(bool),
    StringLiteral(String, Box<str>),
    /// A triple-quoted string. Kept distinct through parsing so a standalone
    /// literal can be recognized as a Mojo docstring.
    TripleStringLiteral(String, Box<str>),
    /// A t-string (`t"…{expr}…"`) or raw t-string (`rt"…"`), lexed into
    /// alternating literal text and raw interpolation-expression text; the parser
    /// re-parses each interpolation into a real `Expr`. `raw` is true for `rt`.
    TString {
        chunks: Vec<TStringChunk>,
        raw: bool,
    },

    // Operators & Punctuation
    Assign,
    // Augmented assignment: `+= -= *= /= //= %= **= &= |= ^=`
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    DoubleSlashEq,
    PercentEq,
    DoubleStarEq,
    AmpEq,
    PipeEq,
    CaretEq,
    Colon,
    ColonEq, // `:=` (the walrus / named-expression operator)
    Comma,
    Semicolon,
    Dot,      // `.`
    Ellipsis, // `...` (an unimplemented trait-method requirement)
    At,       // `@`
    Amp,      // `&` (trait-bound conjunction)
    Pipe,     // `|`
    Caret,    // `^` (transfer sigil)
    Tilde,    // `~` (bitwise inversion)
    Arrow,    // `->`
    LParen,
    RParen,
    LBracket, // `[`  (type-parameter / type-argument list)
    RBracket, // `]`
    LBrace,   // `{`  (effect sets and set/dictionary displays)
    RBrace,   // `}`

    // Arithmetic operators
    Plus,        // `+`
    Minus,       // `-`
    Star,        // `*`
    DoubleStar,  // `**`
    Slash,       // `/`
    DoubleSlash, // `//`
    Percent,     // `%`

    // Comparison operators
    EqEq,  // `==`
    NotEq, // `!=`
    Lt,    // `<`
    Gt,    // `>`
    Le,    // `<=`
    Ge,    // `>=`
    Shl,   // `<<`
    Shr,   // `>>`

    // Structural (Offside Rule) Tokens
    Newline,
    Indent,
    Dedent,
    Eof,
}

impl Token {
    /// The keyword token for `text`, or `None` if it is an ordinary identifier.
    /// The single source of truth for the reserved-word set (the lexer calls this
    /// after scanning a word). `True`/`False` map to `BoolLiteral`s, not keywords.
    pub fn keyword(text: &str) -> Option<Self> {
        Some(match text {
            // Mojo-only
            "var" => Self::Var,
            "struct" => Self::Struct,
            "trait" => Self::Trait,
            "comptime" => Self::Comptime,
            "raises" => Self::Raises,
            // Shared with Python
            "def" => Self::Def,
            "lambda" => Self::Lambda,
            "return" => Self::Return,
            "pass" => Self::Pass,
            "None" => Self::None,
            "and" => Self::And,
            "or" => Self::Or,
            "not" => Self::Not,
            "if" => Self::If,
            "elif" => Self::Elif,
            "else" => Self::Else,
            "while" => Self::While,
            "for" => Self::For,
            "in" => Self::In,
            "is" => Self::Is,
            "with" => Self::With,
            "break" => Self::Break,
            "continue" => Self::Continue,
            "raise" => Self::Raise,
            "try" => Self::Try,
            "except" => Self::Except,
            "finally" => Self::Finally,
            "import" => Self::Import,
            "from" => Self::From,
            "as" => Self::As,
            // Unimplemented syntax
            "class" => Self::Class,
            "del" => Self::Del,
            "match" => Self::Match,
            "yield" => Self::Yield,
            "case" => Self::Case,
            // Boolean literals lex as values, not keywords.
            "True" => Self::BoolLiteral(true),
            "False" => Self::BoolLiteral(false),
            _ => return None,
        })
    }

    /// The spelling of a keyword that may still name a method or a member
    /// (`class`, `del`, `match`, `yield`, `case`), or `None` for any other token.
    pub const fn member_name_keyword(&self) -> Option<&'static str> {
        Some(match self {
            Self::Class => "class",
            Self::Del => "del",
            Self::Match => "match",
            Self::Yield => "yield",
            Self::Case => "case",
            _ => return None,
        })
    }
}

/// The encoding of a [`SyntaxId`]: its number in the encoding, or how it
/// was derived.
#[derive(serde::Serialize, serde::Deserialize)]
enum EncodedSyntax {
    Plain(u64),
    Derived(Box<Self>, u32),
}

impl EncodedSyntax {
    fn of(id: SyntaxId) -> Result<Self, String> {
        match id.derivation() {
            Some((parent, ordinal)) => Ok(Self::Derived(Box::new(Self::of(parent)?), ordinal)),
            None => OCCURRENCES.with_borrow_mut(|occurrences| match occurrences {
                Some(Occurrences::Encoding { numbers, .. }) => {
                    let next = numbers.len() as u64;
                    Ok(Self::Plain(*numbers.entry(id.0).or_insert(next)))
                }
                _ => Err("a syntax identity is local to one compilation".to_string()),
            }),
        }
    }

    fn decode(self) -> Result<SyntaxId, String> {
        match self {
            Self::Plain(number) => OCCURRENCES.with_borrow_mut(|occurrences| match occurrences {
                Some(Occurrences::Decoding { identities, .. }) => {
                    Ok(*identities.entry(number).or_insert_with(SyntaxId::fresh))
                }
                _ => Err("a syntax identity is local to one compilation".to_string()),
            }),
            Self::Derived(parent, ordinal) => Ok(SyntaxId::derived(parent.decode()?, ordinal)),
        }
    }
}

/// The occurrence encoding in progress on this thread.
enum Occurrences {
    Encoding {
        files: std::collections::HashMap<Option<String>, u32>,
        /// Each underived identity's number in the encoding.
        numbers: std::collections::HashMap<u64, u64>,
    },
    Decoding {
        files: Vec<Option<String>>,
        identities: std::collections::HashMap<u64, SyntaxId>,
    },
}

thread_local! {
    static OCCURRENCES: std::cell::RefCell<Option<Occurrences>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::SyntaxId;

    #[test]
    fn derived_identities_are_stable_and_invertible() {
        let parent = SyntaxId::fresh();
        let first = SyntaxId::derived(parent, 0);
        assert_eq!(SyntaxId::derived(parent, 0), first);
        assert_ne!(SyntaxId::derived(parent, 1), first);
        assert_ne!(SyntaxId::derived(SyntaxId::fresh(), 0), first);
        assert_eq!(first.derivation(), Some((parent, 0)));
        assert_eq!(parent.derivation(), None);
        assert_eq!(SyntaxId(7).derivation(), None);
    }
}
