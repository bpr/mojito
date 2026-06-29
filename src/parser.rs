use std::iter::Peekable;
use crate::lexer::Token;
use crate::error::{LexError, ParseError};

// --- Abstract Syntax Tree (AST) ---

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    VarDecl { name: String, value: Expr },
    Block { name: String, body: Vec<Stmt> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Bool(bool),
    Identifier(String),
}

// --- Recursive Descent Parser ---

pub struct Parser<I: Iterator<Item = Result<Token, LexError>>> {
    tokens: Peekable<I>,
}

impl<I: Iterator<Item = Result<Token, LexError>>> Parser<I> {
    pub fn new(tokens: I) -> Self {
        Self {
            tokens: tokens.peekable(),
        }
    }

    /// Helper to get the next token, propagating errors
    fn next_token(&mut self) -> Result<Token, ParseError> {
        match self.tokens.next() {
            Some(Ok(token)) => Ok(token),
            Some(Err(err)) => Err(ParseError::LexerError(err)),
            None => Err(ParseError::UnexpectedEof("Expected a token, found EOF".into())),
        }
    }

    /// Helper to peek at the next token without consuming it
    fn peek_token(&mut self) -> Result<Option<&Token>, ParseError> {
        match self.tokens.peek() {
            Some(Ok(token)) => Ok(Some(token)),
            Some(Err(err)) => Err(ParseError::LexerError(err.clone())),
            None => Ok(None),
        }
    }

    /// Consumes a token and ensures it matches the expected one
    fn expect(&mut self, expected: Token, context_msg: &str) -> Result<(), ParseError> {
        let token = self.next_token()?;
        if token == expected {
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken(token, context_msg.to_string()))
        }
    }

    /// Parses the top-level program
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        
        while let Some(token) = self.peek_token()? {
            match token {
                Token::Eof => break,
                Token::Newline => {
                    self.next_token()?; // Ignore empty lines at the top level
                }
                _ => {
                    stmts.push(self.parse_statement()?);
                }
            }
        }
        
        Ok(stmts)
    }

    /// Parses a single statement (VarDecl or Block)
    fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Var, "Statements must begin with 'var'")?;

        let name = match self.next_token()? {
            Token::Identifier(id) => id,
            token => return Err(ParseError::UnexpectedToken(token, "Expected identifier after 'var'".into())),
        };

        let next = self.next_token()?;
        match next {
            Token::Assign => {
                let value = self.parse_expression()?;
                self.expect_stmt_end()?;
                Ok(Stmt::VarDecl { name, value })
            }
            Token::Colon => {
                self.expect_stmt_end()?;
                
                // Block must immediately be followed by an indentation
                self.expect(Token::Indent, "Expected indented block after ':'")?;
                
                let mut body = Vec::new();
                while let Some(token) = self.peek_token()? {
                    if token == &Token::Dedent {
                        self.next_token()?; // Consume the dedent to end the block
                        break;
                    }
                    if token == &Token::Newline {
                        self.next_token()?; // Skip empty lines inside the block
                        continue;
                    }
                    body.push(self.parse_statement()?);
                }
                
                Ok(Stmt::Block { name, body })
            }
            token => Err(ParseError::UnexpectedToken(token, "Expected '=' or ':' after identifier".into())),
        }
    }

    /// Parses a primary expression
    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let token = self.next_token()?;
        match token {
            Token::IntLiteral(val) => Ok(Expr::Int(val)),
            Token::BoolLiteral(val) => Ok(Expr::Bool(val)),
            Token::Identifier(id) => Ok(Expr::Identifier(id)),
            Token::LParen => {
                let expr = self.parse_expression()?;
                self.expect(Token::RParen, "Expected closing ')' after expression")?;
                Ok(expr)
            }
            token => Err(ParseError::UnexpectedToken(token, "Expected an expression".into())),
        }
    }

    /// Ensures a statement is cleanly terminated by a Newline or EOF
    fn expect_stmt_end(&mut self) -> Result<(), ParseError> {
        let token = self.next_token()?;
        match token {
            Token::Newline | Token::Eof => Ok(()),
            _ => Err(ParseError::UnexpectedToken(token, "Expected newline or EOF at the end of statement".into())),
        }
    }
}
