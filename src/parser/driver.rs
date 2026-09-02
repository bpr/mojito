//! Token cursor plumbing and the `parse_program` entry points.

use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    pub fn new(tokens: I) -> Self {
        Self {
            tokens: tokens.peekable(),
            last_span: (0, 0),
            last_significant_end: 0,
            last_stmt_ended_with_semicolon: false,
        }
    }

    /// Helper to get the next token, propagating errors. Records the consumed
    /// token's span in `self.last_span` (and its end in `last_significant_end`
    /// unless it is a layout token).
    pub(super) fn next_token(&mut self) -> Result<Token, ParseError> {
        match self.tokens.next() {
            Some(Ok((token, span))) => {
                self.last_span = span;
                if !matches!(
                    token,
                    Token::Newline | Token::Indent | Token::Dedent | Token::Eof
                ) {
                    self.last_significant_end = span.1;
                }
                Ok(token)
            }
            Some(Err(err)) => Err(ParseError::LexerError(err)),
            None => Err(ParseError::UnexpectedEof(
                "Expected a token, found EOF".into(),
            )),
        }
    }

    /// Helper to peek at the next token without consuming it
    pub(super) fn peek_token(&mut self) -> Result<Option<&Token>, ParseError> {
        match self.tokens.peek() {
            Some(Ok((token, _))) => Ok(Some(token)),
            Some(Err(err)) => Err(ParseError::LexerError(err.clone())),
            None => Ok(None),
        }
    }

    /// The start byte offset of the next (unconsumed) token, or the end of the
    /// last consumed token at end of input. Used as a node's span start.
    pub(super) fn peek_start(&mut self) -> usize {
        match self.tokens.peek() {
            Some(Ok((_, span))) => span.0,
            _ => self.last_span.1,
        }
    }

    /// Build a spanned expression: `kind` spanning from `start` (its first token's
    /// start offset) to the end of the most recently consumed token.
    /// Rebuild a `Self.o`-rooted binder/projection chain the type parser
    /// consumed (`Self.o`, `Self.o._get_owned_interior["element"]`) as the
    /// origin expression it spells in an origin slot.
    pub(super) fn rematerialize_self_param_chain(&self, ty: &Type, start: usize) -> Option<Expr> {
        match ty {
            Type::SelfParam(param) => {
                let object = self.node(ExprKind::Identifier("Self".to_string()), start);
                Some(self.node(
                    ExprKind::Member {
                        object: Box::new(object),
                        field: param.clone(),
                    },
                    start,
                ))
            }
            Type::Assoc { base, name, args } => {
                let base = self.rematerialize_self_param_chain(base, start)?;
                let member = self.node(
                    ExprKind::Member {
                        object: Box::new(base),
                        field: name.clone(),
                    },
                    start,
                );
                match args.as_slice() {
                    [] => Some(member),
                    [crate::ast::ParamArg::Value(index)] => Some(self.node(
                        ExprKind::Index {
                            object: Box::new(member),
                            index: Box::new(index.clone()),
                        },
                        start,
                    )),
                    _ => None,
                }
            }
            Type::IndexedProjection { base, index } => {
                let base = self.rematerialize_self_param_chain(base, start)?;
                Some(self.node(
                    ExprKind::Index {
                        object: Box::new(base),
                        index: index.clone(),
                    },
                    start,
                ))
            }
            _ => None,
        }
    }

    pub(super) fn node(&self, kind: ExprKind, start: usize) -> Expr {
        Expr {
            kind,
            span: (start, self.last_span.1),
            source: None,
            syntax_id: crate::token::SyntaxId::fresh(),
        }
    }

    /// Consumes a token and ensures it matches the expected one
    pub(super) fn expect(&mut self, expected: Token, context_msg: &str) -> Result<(), ParseError> {
        let token = self.next_token()?;
        if token == expected {
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken(token, context_msg.to_string()))
        }
    }

    /// Consumes the next token, requiring it to be an identifier, and returns its text.
    pub(super) fn expect_identifier(&mut self, context_msg: &str) -> Result<String, ParseError> {
        match self.next_token()? {
            Token::Identifier(id) => Ok(id),
            token => Err(ParseError::UnexpectedToken(token, context_msg.to_string())),
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

    /// Parse for diagnostics, recovering at layout-level statement boundaries.
    /// Normal compilation continues to use `parse_program` and remains fail-fast.
    pub fn parse_program_diagnostic(&mut self, max_errors: usize) -> ParseReport {
        let mut program = Vec::new();
        let mut errors = Vec::new();
        let mut truncated = false;

        loop {
            let token = match self.peek_token() {
                Ok(Some(token)) => token.clone(),
                Ok(None) => break,
                Err(err) => {
                    errors.push(err.at(self.last_span));
                    self.discard_one();
                    if errors.len() >= max_errors {
                        truncated = true;
                        break;
                    }
                    continue;
                }
            };
            match token {
                Token::Eof => break,
                Token::Newline | Token::Indent | Token::Dedent => self.discard_one(),
                _ => match self.parse_statement() {
                    Ok(stmt) => program.push(stmt),
                    Err(err) => {
                        errors.push(err);
                        if errors.len() >= max_errors {
                            truncated = true;
                            break;
                        }
                        self.synchronize_statement();
                    }
                },
            }
        }
        ParseReport {
            program,
            errors,
            truncated,
        }
    }
}
