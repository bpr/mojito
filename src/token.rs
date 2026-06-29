
/// The minimal token set required to parse our pseudo-Python language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords & Identifiers
    Var,
    Identifier(String),
    
    // Literals
    IntLiteral(i64),
    BoolLiteral(bool),
    
    // Operators & Punctuation
    Assign,
    Colon,
    LParen,
    RParen,
    
    // Structural (Offside Rule) Tokens
    Newline,
    Indent,
    Dedent,
    Eof,
}
