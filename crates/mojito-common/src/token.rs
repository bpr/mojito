/// A source byte range `(start, end)` — a half-open `[start, end)` slice of the
/// original source. This is the single, canonical span type shared by the lexer
/// (which stamps each token), the parser (which propagates spans onto AST nodes),
/// and the MIR (whose `SpanTable` maps each temporary back to its origin span).
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
    pub fn new(source: Option<String>, span: Span) -> Self {
        Self {
            source,
            span,
            syntax: None,
        }
    }

    pub fn syntax(source: Option<String>, span: Span, syntax: SyntaxId) -> Self {
        Self {
            source,
            span,
            syntax: Some(syntax),
        }
    }

    /// Return diagnostic provenance only. Semantic occurrence identity is a
    /// phase-local lookup discriminator and must not leak into displayed spans,
    /// MIR source maps, or source-only comparisons.
    pub fn without_syntax(mut self) -> Self {
        self.syntax = None;
        self
    }

    /// Compare only the source file and byte range, ignoring which elaborated
    /// syntax occurrence supplied the location.
    pub fn same_provenance(&self, other: &Self) -> bool {
        self.source == other.source && self.span == other.span
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
#[derive(Debug, Clone, PartialEq)]
pub enum TStringChunk {
    Text(String),
    Interp(String),
}

#[derive(Debug, Clone, PartialEq)]
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

    // Identifiers (includes type names like `Int`, `String`, `Bool`)
    Identifier(String),

    // Literals
    IntLiteral(crate::literal::IntLiteral),
    FloatLiteral(crate::literal::FloatLiteral),
    BoolLiteral(bool),
    StringLiteral(String),
    /// A triple-quoted string. Kept distinct through parsing so a standalone
    /// literal can be recognized as a Mojo docstring.
    TripleStringLiteral(String),
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
    pub fn keyword(text: &str) -> Option<Token> {
        Some(match text {
            // Mojo-only
            "var" => Token::Var,
            "struct" => Token::Struct,
            "trait" => Token::Trait,
            "comptime" => Token::Comptime,
            "raises" => Token::Raises,
            // Shared with Python
            "def" => Token::Def,
            "lambda" => Token::Lambda,
            "return" => Token::Return,
            "pass" => Token::Pass,
            "None" => Token::None,
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            "if" => Token::If,
            "elif" => Token::Elif,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "in" => Token::In,
            "is" => Token::Is,
            "with" => Token::With,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "raise" => Token::Raise,
            "try" => Token::Try,
            "except" => Token::Except,
            "finally" => Token::Finally,
            "import" => Token::Import,
            "from" => Token::From,
            "as" => Token::As,
            // Boolean literals lex as values, not keywords.
            "True" => Token::BoolLiteral(true),
            "False" => Token::BoolLiteral(false),
            _ => return None,
        })
    }
}
