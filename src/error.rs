use crate::lexer::Token;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum LexError {
    IndentationError(usize),
    UnmatchedParenthesis(usize),
    UnexpectedCharacter(char, usize),
    InvalidInteger(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    LexerError(LexError),
    UnexpectedToken(Token, String),
    UnexpectedEof(String),
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::IndentationError(pos) => write!(f, "Indentation error at byte {}", pos),
            LexError::UnmatchedParenthesis(pos) => write!(f, "Unmatched closing parenthesis at byte {}", pos    ),
            LexError::UnexpectedCharacter(c, pos) => write!(f, "Unexpected character '{}' at byte {}", c, pos),
            LexError::InvalidInteger(pos) => write!(f, "Invalid integer literal starting at byte {}", pos),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::LexerError(err) => write!(f, "Lexer error: {}", err),
            ParseError::UnexpectedToken(token, msg) => write!(f, "Unexpected token {:?}: {}", token, msg),
            ParseError::UnexpectedEof(msg) => write!(f, "Unexpected EOF: {}", msg),
        }
    }
}

impl std::error::Error for LexError {}
impl std::error::Error for ParseError {}
